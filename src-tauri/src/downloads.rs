//! Resumable model downloads.
//!
//! Design notes:
//!
//! * Each file is split into fixed segments, and every segment gets its own
//!   sidecar file. Resuming is then per-segment and needs no bookkeeping: a
//!   segment resumes at whatever its own file length already is. Segments are
//!   concatenated and size-checked once every one has finished, so a partial
//!   file is never mistaken for a complete one.
//! * Range requests mean an interrupted transfer picks up where it stopped
//!   instead of starting over. Retries use exponential backoff so a flaky link
//!   does not turn into a hot loop.
//! * The final rename only happens after the byte count matches what the Hub
//!   reports, so a truncated file can never be presented as a good one.

use serde::Serialize;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Concurrent range requests per file. Four keeps a single file from
/// saturating or starving everything else without over-fragmenting the parts.
const SEGMENTS: usize = 4;
const CHUNK: usize = 256 * 1024;
const MAX_RETRIES: u32 = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DownloadState {
    Running,
    Paused,
    Done,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadStatus {
    pub id: String,
    pub filename: String,
    pub received: u64,
    pub total: u64,
    pub state: DownloadState,
    pub error: Option<String>,
    pub rate_bps: f64,
    /// Seconds left, or None when it cannot be estimated yet.
    pub eta_secs: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Progress {
    id: String,
    filename: String,
    received: u64,
    total: u64,
    state: DownloadState,
    error: Option<String>,
    rate_bps: f64,
}

struct Job {
    filename: String,
    url: String,
    total: u64,
    dir: PathBuf,
    cancel: Arc<AtomicBool>,
}

static JOBS: Mutex<Option<HashMap<String, Job>>> = Mutex::new(None);
static STATUS: Mutex<Vec<DownloadStatus>> = Mutex::new(Vec::new());

impl DownloadStatus {
    /// Compact state token, used by the emitter to spot transitions.
    pub fn state_key(&self) -> String {
        format!(
            "{}|{}",
            self.received,
            match self.state {
                DownloadState::Running => "running",
                DownloadState::Paused => "paused",
                DownloadState::Done => "done",
                DownloadState::Failed => "failed",
                DownloadState::Cancelled => "cancelled",
            }
        )
    }
}

fn jobs() -> std::sync::MutexGuard<'static, Option<HashMap<String, Job>>> {
    JOBS.lock().unwrap_or_else(|e| e.into_inner())
}

fn status_map() -> std::sync::MutexGuard<'static, Vec<DownloadStatus>> {
    STATUS.lock().unwrap_or_else(|e| e.into_inner())
}

fn part_path(dir: &Path, filename: &str, index: usize) -> PathBuf {
    dir.join(format!("{filename}.part{index}"))
}

/// Best-effort free space on the filesystem holding `dir`.
pub fn free_bytes(dir: &Path) -> u64 {
    let probe = dir.to_path_buf();
    let out = std::process::Command::new("df")
        .arg("--output=avail")
        .arg("-B1")
        .arg(&probe)
        .output();
    match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .lines()
            .nth(1)
            .and_then(|l| l.trim().parse::<u64>().ok())
            .unwrap_or(0),
        _ => 0,
    }
}

fn publish(progress: Progress) {
    let mut list = status_map();
    let entry = DownloadStatus {
        id: progress.id.clone(),
        filename: progress.filename.clone(),
        received: progress.received,
        total: progress.total,
        state: progress.state,
        error: progress.error.clone(),
        rate_bps: progress.rate_bps,
        eta_secs: if progress.rate_bps > 1.0 && progress.total > progress.received {
            Some((progress.total - progress.received) as f64 / progress.rate_bps)
        } else {
            None
        },
    };
    match list.iter_mut().find(|s| s.id == progress.id) {
        Some(slot) => *slot = entry,
        None => list.push(entry),
    }
}

/// Total bytes already on disk across every segment of a file.
fn received_total(dir: &Path, filename: &str, total: u64) -> u64 {
    let per = total.div_ceil(SEGMENTS as u64);
    let mut sum = 0u64;
    for i in 0..SEGMENTS {
        let path = part_path(dir, filename, i);
        let len = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        // Clamp: a segment can never exceed its slice.
        let expected = if i == SEGMENTS - 1 {
            total.saturating_sub(per * (SEGMENTS as u64 - 1))
        } else {
            per
        };
        sum += len.min(expected);
    }
    sum
}

/// Download one segment, resuming from whatever is already on disk.
fn fetch_segment(
    url: &str,
    path: &Path,
    start: u64,
    end: u64,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let want = end.saturating_sub(start) + 1;

    let mut have = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    if have >= want {
        return Ok(());
    }

    let mut attempt = 0u32;
    let mut last_error: Option<String> = None;
    while have < want {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".to_string());
        }
        attempt += 1;
        if attempt > MAX_RETRIES {
            return Err(last_error
                .unwrap_or_else(|| format!("gave up after {MAX_RETRIES} attempts")));
        }

        let from = start + have;
        let response = ureq::get(url)
            .set("Range", &format!("bytes={from}-"))
            .timeout(std::time::Duration::from_secs(120))
            .call();

        let response = match response {
            Ok(r) => r,
            Err(e) => {
                backoff(attempt, cancel);
                let _ = e;
                continue;
            }
        };

        // 206 is a real range response. 200 means the server ignored Range
        // and is sending the whole file, which would corrupt this segment.
        if response.status() == 200 {
            return Err("server ignored the Range header".to_string());
        }

        let mut reader = response.into_reader();
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|e| format!("cannot open {}: {e}", path.display()))?;

        let mut buf = vec![0u8; CHUNK];
        let mut interrupted = false;
        while have < want {
            if cancel.load(Ordering::Relaxed) {
                return Err("cancelled".to_string());
            }
            let n = match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) => {
                    if attempt > MAX_RETRIES {
                        return Err(format!("read failed: {e}"));
                    }
                    last_error = Some(format!("read failed: {e}"));
                    interrupted = true;
                    break;
                }
            };
            // Never write past this segment's slice.
            let room = want - have;
            let take = (n as u64).min(room) as usize;
            file.write_all(&buf[..take])
                .map_err(|e| format!("write failed: {e}"))?;
            have += take as u64;
        }
        drop(file);

        let on_disk = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        if on_disk >= want {
            return Ok(());
        }
        if interrupted {
            last_error = None;
        }
        let on_disk = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        if on_disk >= want {
            return Ok(());
        }
        backoff(attempt, cancel);
    }
    Ok(())
}

fn backoff(attempt: u32, cancel: &AtomicBool) {
    let secs = 1u64 << attempt.min(5);
    let mut waited = 0;
    while waited < secs * 10 {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
        waited += 1;
    }
}

/// Kick off a download on a background thread. Returns immediately.
pub fn start(id: String, filename: String, url: String, total: u64, dir: PathBuf) -> Result<(), String> {
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;

    let target = dir.join(&filename);
    if target.is_file() {
        let len = std::fs::metadata(&target).map(|m| m.len()).unwrap_or(0);
        if total == 0 || len == total {
            publish(Progress {
                id,
                filename,
                received: len,
                total: len.max(total),
                state: DownloadState::Done,
                error: None,
                rate_bps: 0.0,
            });
            return Ok(());
        }
    }

    let available = free_bytes(&dir);
    let already = received_total(&dir, &filename, total);
    if available > 0 && total > 0 && available < (total - already) + 64 * 1024 * 1024 {
        return Err(format!(
            "not enough free space: need {} MiB, have {} MiB",
            (total - already) / 1024 / 1024,
            available / 1024 / 1024
        ));
    }

    {
        let mut map = jobs();
        map.get_or_insert_with(HashMap::new)
            .insert(id.clone(), Job { filename: filename.clone(), url, total, dir, cancel: Arc::new(AtomicBool::new(false)) });
    }

    let thread_id = id.clone();
    std::thread::spawn(move || run(thread_id));
    Ok(())
}

fn run(id: String) {
    let job = {
        let map = jobs();
        let j = map.as_ref().and_then(|m| m.get(&id)).map(|j| (j.filename.clone(), j.url.clone(), j.total, j.dir.clone(), j.cancel.clone()));
        j
    };
    let Some((filename, url, total, dir, cancel)) = job else {
        return;
    };

    let start_time = std::time::Instant::now();
    let start_bytes = received_total(&dir, &filename, total);

    let emit = |state: DownloadState, error: Option<String>| {
        let received = received_total(&dir, &filename, total);
        let elapsed = start_time.elapsed().as_secs_f64().max(0.001);
        // Average rate, not instantaneous: a rate computed over a 100 ms
        // window reads as zero between chunks and makes the bar jitter.
        let rate = received.saturating_sub(start_bytes) as f64 / elapsed;
        publish(Progress {
            id: id.clone(),
            filename: filename.clone(),
            received,
            total,
            state,
            error,
            rate_bps: rate,
        });
    };

    emit(DownloadState::Running, None);

    let per = total.div_ceil(SEGMENTS as u64);
    let range_for = |i: usize| -> (u64, u64) {
        let start = per * i as u64;
        let end = (start + per - 1).min(total.saturating_sub(1));
        (start, end)
    };

    let mut handles = Vec::new();
    for i in 0..SEGMENTS {
        if total == 0 {
            break;
        }
        let (start, end) = range_for(i);
        let url = url.clone();
        let path = part_path(&dir, &filename, i);
        let cancel = cancel.clone();
        handles.push(std::thread::spawn(move || fetch_segment(&url, &path, start, end, &cancel)));
    }

    // Progress pump: the workers only return at the end, so poll alongside.
    let pump_cancel = cancel.clone();
    let pump_dir = dir.clone();
    let pump_file = filename.clone();
    let pump_id = id.clone();
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let pump = std::thread::spawn(move || {
        while rx.recv_timeout(std::time::Duration::from_millis(400)).is_err() {
            let received = received_total(&pump_dir, &pump_file, total);
            let elapsed = start_time.elapsed().as_secs_f64().max(0.001);
            let rate = received.saturating_sub(start_bytes) as f64 / elapsed;
            publish(Progress {
                id: pump_id.clone(),
                filename: pump_file.clone(),
                received,
                total,
                state: DownloadState::Running,
                error: None,
                rate_bps: rate,
            });
        }
        let _ = &pump_cancel;
    });

    let mut failure: Option<String> = None;
    let mut cancelled = false;
    for handle in handles {
        match handle.join() {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                if e == "cancelled" {
                    cancelled = true;
                } else {
                    failure = Some(e);
                }
            }
            Err(_) => failure = Some("segment thread panicked".to_string()),
        }
    }
    let _ = tx.send(());
    let _ = pump.join();

    if let Some(err) = failure {
        emit(DownloadState::Failed, Some(err));
        cleanup(&id);
        return;
    }
    if cancelled || cancel.load(Ordering::Relaxed) {
        emit(DownloadState::Cancelled, None);
        cleanup(&id);
        return;
    }

    // Concatenate, then verify before publishing.
    let assembling = dir.join(format!("{filename}.part"));
    let mut assembled = match std::fs::File::create(&assembling) {
        Ok(f) => f,
        Err(e) => {
            emit(DownloadState::Failed, Some(format!("cannot assemble: {e}")));
            cleanup(&id);
            return;
        }
    };
    for i in 0..SEGMENTS {
        let path = part_path(&dir, &filename, i);
        match std::fs::File::open(&path) {
            Ok(mut seg) => {
                if let Err(e) = std::io::copy(&mut seg, &mut assembled) {
                    emit(DownloadState::Failed, Some(format!("assemble failed: {e}")));
                    cleanup(&id);
                    return;
                }
            }
            Err(e) => {
                emit(DownloadState::Failed, Some(format!("missing segment {i}: {e}")));
                cleanup(&id);
                return;
            }
        }
    }
    drop(assembled);

    let size = std::fs::metadata(&assembling).map(|m| m.len()).unwrap_or(0);
    if total > 0 && size != total {
        emit(
            DownloadState::Failed,
            Some(format!("size mismatch: got {size}, expected {total}")),
        );
        cleanup(&id);
        return;
    }

    let target = dir.join(&filename);
    if std::fs::rename(&assembling, &target).is_err() {
        // Cross-device or a leftover target: copy instead of giving up.
        if let Err(e) = std::fs::copy(&assembling, &target) {
            emit(DownloadState::Failed, Some(format!("cannot finalise: {e}")));
            cleanup(&id);
            return;
        }
        let _ = std::fs::remove_file(&assembling);
    }
    for i in 0..SEGMENTS {
        let _ = std::fs::remove_file(part_path(&dir, &filename, i));
    }

    emit(DownloadState::Done, None);
    cleanup(&id);
}

fn cleanup(id: &str) {
    let mut map = jobs();
    if let Some(m) = map.as_mut() {
        m.remove(id);
    }
}

pub fn cancel(id: &str) {
    let map = jobs();
    if let Some(job) = map.as_ref().and_then(|m| m.get(id)) {
        job.cancel.store(true, Ordering::Relaxed);
    }
}

pub fn statuses() -> Vec<DownloadStatus> {
    status_map().clone()
}

/// Drop finished rows so the UI does not accumulate dead entries forever.
pub fn clear_finished() {
    status_map().retain(|s| matches!(s.state, DownloadState::Running));
}