//! Durable generation queue.
//!
//! Generation used to be one blocking call per click: the UI awaited it, so a
//! second click was impossible, a restart lost the work, and a wedged backend
//! looked identical to a slow one. This module makes submission instant and
//! non-blocking, keeps jobs across restarts, and turns a hang into a clear
//! failure (the watchdog lives in `generate::wait_for_job`).
//!
//! Jobs run one at a time on a background thread. That is deliberate: both
//! backends hold the whole model resident, and overlapping renders on a 4 GB
//! card would OOM rather than finish sooner.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};

use serde::{Deserialize, Serialize};

use crate::generate::GenerateParams;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobState {
    /// Waiting for the engine.
    Queued,
    /// Submitted to the backend and polling.
    Running,
    /// Finished with at least one track.
    Done,
    /// Failed, stalled, or timed out. `error` says which.
    Failed,
    /// Cancelled by the user.
    Cancelled,
    /// Was `Running` when the app exited. Needs a decision; nothing resumes on
    /// its own, so a restart can never double-run work.
    Interrupted,
}

impl JobState {
    /// Active states occupy the single engine slot.
    fn is_active(self) -> bool {
        matches!(self, JobState::Running)
    }

    /// Terminal states no longer need engine time.
    pub fn is_finished(self) -> bool {
        matches!(
            self,
            JobState::Done | JobState::Failed | JobState::Cancelled
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueuedJob {
    pub id: String,
    pub params: GenerateParams,
    pub state: JobState,
    pub error: Option<String>,
    pub track_ids: Vec<String>,
    pub created_at: u64,
    pub started_at: Option<u64>,
    pub finished_at: Option<u64>,
    pub audio_secs: f32,
    /// Wall-clock seconds the engine was busy, across any retries of this job.
    pub elapsed_secs: f64,
}

impl QueuedJob {
    /// Audio seconds per second of generation: 3.0 means a minute of music
    /// took twenty seconds. The single most useful number for judging whether a
    /// model or setting is worth keeping, and the reason to record both clocks
    /// rather than just the result.
    pub fn render_ratio(&self) -> Option<f32> {
        match self.state {
            JobState::Done
                if self.audio_secs > 0.1 && self.elapsed_secs > 0.1 =>
            {
                Some((self.audio_secs as f64 / self.elapsed_secs) as f32)
            }
            _ => None,
        }
    }

    /// One-line summary for the queue list.
    pub fn label(&self) -> String {
        let caption = self.params.caption.trim();
        let base = if caption.is_empty() {
            "(no caption)"
        } else {
            caption
        };
        if base.chars().count() > 60 {
            let head: String = base.chars().take(59).collect();
            format!("{head}…")
        } else {
            base.to_string()
        }
    }
}

fn queue_path() -> PathBuf {
    crate::paths::app_data_dir().join("queue.json")
}

fn read_jobs() -> Vec<QueuedJob> {
    let path = queue_path();
    let Ok(bytes) = std::fs::read(&path) else {
        return Vec::new();
    };
    match serde_json::from_slice::<Vec<QueuedJob>>(&bytes) {
        Ok(jobs) => jobs,
        // A corrupt queue must not stop the app opening. The user loses
        // history but can still generate, which is the right trade.
        Err(_) => Vec::new(),
    }
}

fn write_jobs(jobs: &[QueuedJob]) {
    let path = queue_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(bytes) = serde_json::to_vec_pretty(jobs) {
        write_atomic(&path, &bytes);
    }
}

/// Replace a file without leaving a truncated copy behind. The queue is the
/// only record of pending work, so a crash mid-write must not destroy it.
fn write_atomic(path: &Path, bytes: &[u8]) {
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, bytes).is_err() {
        return;
    }
    if std::fs::rename(&tmp, path).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

/// Shared state plus the wakeup that lets the runner idle without spinning.
struct Inner {
    jobs: Mutex<Vec<QueuedJob>>,
    /// Bumped on every mutation so the runner wakes.
    wake: Condvar,
    /// Held so queued work can be emitted to the UI. Set once during startup.
    app: Mutex<Option<tauri::AppHandle>>,
    /// Engine-side job id of the render in flight, for cancel.
    backend_job: Mutex<Option<String>>,
}

impl Inner {
    /// Apply `f` to the job list, persist, and notify the runner.
    fn update<F: FnOnce(&mut Vec<QueuedJob>)>(&self, f: F) {
        let mut jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut jobs);
        write_jobs(&jobs);
        self.wake.notify_all();
    }
}

/// The queue handle. `OnceLock` because the runner thread needs the same state
/// and Tauri commands each receive their own handle.
fn state() -> &'static Arc<Inner> {
    static STATE: std::sync::OnceLock<Arc<Inner>> = std::sync::OnceLock::new();
    STATE.get_or_init(|| {
        let mut jobs = read_jobs();
        // A job recorded as Running cannot still be running: the process that
        // was running it is gone. Mark it Interrupted so the UI can ask.
        let mut changed = false;
        for job in jobs.iter_mut() {
            if job.state == JobState::Running {
                job.state = JobState::Interrupted;
                changed = true;
            }
        }
        if changed {
            write_jobs(&jobs);
        }
        Arc::new(Inner {
            jobs: Mutex::new(jobs),
            wake: Condvar::new(),
            app: Mutex::new(None),
            backend_job: Mutex::new(None),
        })
    })
}

pub fn list() -> Vec<QueuedJob> {
    state()
        .jobs
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

fn next_id() -> String {
    // Monotonic suffix rather than a random id: a queue that survives a
    // restart is easier to reason about when ids are ordered.
    let n = state()
        .jobs
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .len();
    format!("q{}", now_secs() * 1000 + n as u64)
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Enqueue a job and return it. Submission never blocks on the engine.
pub fn enqueue(params: GenerateParams) -> QueuedJob {
    // A negative seed is the UI's "random" sentinel, and the backend resolves it
    // internally with `random_device` without ever reporting the value back.
    // Pinning it now is what makes a job reproducible: the row shows the seed,
    // wildcards expand against it, and Retry reuses it.
    let params = GenerateParams {
        seed: crate::wildcards::resolve_seed(params.seed),
        ..params
    };
    let job = QueuedJob {
        id: next_id(),
        params,
        state: JobState::Queued,
        error: None,
        track_ids: Vec::new(),
        created_at: now_secs(),
        started_at: None,
        finished_at: None,
        audio_secs: 0.0,
        elapsed_secs: 0.0,
    };
    state().update(|jobs| jobs.push(job.clone()));
    emit_change();
    job
}

/// Cancel a queued job, or the running job when `job_id` names it. The running
/// job is cancelled through the backend's own cancel endpoint so it stops at the
/// next diffusion step rather than being killed mid-write.
pub fn cancel(job_id: String) -> Result<(), String> {
    let inner = state();
    let is_running = {
        let jobs = inner.jobs.lock().unwrap_or_else(|e| e.into_inner());
        match jobs.iter().find(|j| j.id == job_id) {
            None => return Err(format!("no such job {job_id}")),
            Some(job) => job.state == JobState::Running,
        }
    };

    if is_running {
        // Ask the backend to stop, then let the runner observe the result. A
        // queued job needs no backend round trip at all.
        if let (Some(port), Some(backend_job)) =
            (crate::server::running_port(), current_running_backend_job())
        {
            let _ = crate::generate::cancel(port, &backend_job);
        }
    }

    inner.update(|jobs| {
        if let Some(job) = jobs.iter_mut().find(|j| j.id == job_id) {
            match job.state {
                JobState::Queued => {
                    job.state = JobState::Cancelled;
                    job.finished_at = Some(now_secs());
                }
                // Running is left alone: generate() returns and the runner
                // records the outcome, so elapsed time and ratio stay correct.
                JobState::Running | JobState::Done => {}
                JobState::Failed | JobState::Cancelled => job.state = JobState::Cancelled,
                JobState::Interrupted => job.state = JobState::Cancelled,
            }
        }
    });
    emit_change();
    Ok(())
}

/// Put a failed or interrupted job back at the front of the queue with its
/// original parameters, so a retry is the same request, not a reconstruction.
pub fn retry(job_id: String) -> Result<QueuedJob, String> {
    let inner = state();
    let mut retried: Option<QueuedJob> = None;
    inner.update(|jobs| {
        let Some(job) = jobs.iter_mut().find(|j| j.id == job_id) else {
            return;
        };
        if job.state.is_active() || job.state == JobState::Queued {
            return;
        }
        let fresh = QueuedJob {
            id: next_id(),
            params: job.params.clone(),
            state: JobState::Queued,
            error: None,
            track_ids: Vec::new(),
            created_at: now_secs(),
            started_at: None,
            finished_at: None,
            audio_secs: 0.0,
            elapsed_secs: 0.0,
        };
        retried = Some(fresh.clone());
        jobs.push(fresh);
    });
    emit_change();
    retried.ok_or_else(|| format!("job {job_id} cannot be retried"))
}

/// Clear finished jobs. Active and queued work is untouched.
pub fn clear_done() {
    state().update(|jobs| jobs.retain(|j| !j.state.is_finished()));
    emit_change();
}

/// Cancel everything: queued jobs drop out and the running one is asked to stop.
pub fn reset() {
    let ids: Vec<String> = state()
        .jobs
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .filter(|j| !j.state.is_finished())
        .map(|j| j.id.clone())
        .collect();
    for id in ids {
        let _ = cancel(id);
    }
    emit_change();
}

/// Drop an interrupted job the user does not want to resume.
pub fn discard(job_id: String) -> Result<(), String> {
    let mut found = false;
    state().update(|jobs| {
        if let Some(pos) = jobs
            .iter()
            .position(|j| j.id == job_id && j.state == JobState::Interrupted)
        {
            jobs.remove(pos);
            found = true;
        }
    });
    emit_change();
    if found {
        Ok(())
    } else {
        Err(format!("job {job_id} is not interrupted"))
    }
}

/// The backend's own job id for whatever is rendering now. This is not the
/// queue id: cancel has to be sent to the engine's `/job?id=` namespace, and the
/// engine only reveals its id once a stage has been submitted.
fn current_running_backend_job() -> Option<String> {
    state()
        .backend_job
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// Remember the app handle so queue events can reach the UI. Called once during
/// startup, before the runner thread begins.
pub fn set_app(app: tauri::AppHandle) {
    *state().app.lock().unwrap_or_else(|e| e.into_inner()) = Some(app);
}

fn app() -> Option<tauri::AppHandle> {
    state()
        .app
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// Emit `queue://changed` so the list re-renders. Failure is not fatal: the
/// queue is still correct on disk and refreshes on the next action.
fn emit_change() {
    use tauri::Emitter;
    if let Some(app) = app() {
        let _ = app.emit("queue://changed", ());
    }
}

fn set_running(id: &str) {
    state().update(|jobs| {
        if let Some(job) = jobs.iter_mut().find(|j| j.id == id) {
            job.state = JobState::Running;
            job.started_at = Some(now_secs());
        }
    });
    emit_change();
}

fn finish(id: &str, tracks: &[crate::generate::Track], cancelled: bool, error: Option<String>) {
    state().update(|jobs| {
        if let Some(job) = jobs.iter_mut().find(|j| j.id == id) {
            let now = now_secs();
            if let Some(start) = job.started_at {
                job.elapsed_secs += (now.saturating_sub(start)) as f64;
            }
            job.finished_at = Some(now);
            job.error = error;
            if !tracks.is_empty() {
                job.track_ids = tracks.iter().map(|t| t.id.clone()).collect();
                job.audio_secs = tracks.iter().map(|t| t.duration_secs).sum();
                job.state = JobState::Done;
            } else if cancelled {
                job.state = JobState::Cancelled;
            } else {
                job.state = JobState::Failed;
            }
        }
    });
    emit_change();
}

/// Background worker. Sleeps on the condvar rather than polling, so an idle
/// queue costs nothing.
pub fn spawn_runner() {
    let inner = state();
    std::thread::spawn(move || {
        loop {
            let job_id = {
                let jobs = inner.jobs.lock().unwrap_or_else(|e| e.into_inner());
                jobs.iter()
                    .find(|j| j.state == JobState::Queued)
                    .map(|j| j.id.clone())
            };

            let Some(job_id) = job_id else {
                let guard = inner.jobs.lock().unwrap_or_else(|e| e.into_inner());
                let _guard = inner.wake.wait(guard).unwrap_or_else(|e| e.into_inner());
                continue;
            };

            // Wait for the engine rather than failing a queued job submitted
            // before the server finished starting.
            let Some(port) = wait_for_engine() else {
                finish(
                    &job_id,
                    &[],
                    false,
                    Some("engine stopped before this job could start".to_string()),
                );
                continue;
            };

            let params = {
                let jobs = inner.jobs.lock().unwrap_or_else(|e| e.into_inner());
                jobs.iter()
                    .find(|j| j.id == job_id)
                    .map(|j| j.params.clone())
            };
            let Some(params) = params else { continue };

            set_running(&job_id);

            let params_for_run = params.clone();
            let id_for_run = job_id.clone();
            let result = std::thread::spawn(move || {
                use tauri::Emitter;
                let stage_id = id_for_run.clone();
                let mut stage = move |stage: &str, detail: &str| {
                    if let Some(app) = app() {
                        let _ = app.emit("queue://stage", (stage_id.clone(), stage, detail));
                    }
                };
                let job_id_for_cb = id_for_run.clone();
                let mut job = move |id: &str| {
                    *state()
                        .backend_job
                        .lock()
                        .unwrap_or_else(|e| e.into_inner()) = Some(id.to_string());
                    if let Some(app) = app() {
                        let _ = app.emit("queue://stage", (job_id_for_cb.clone(), "running", id));
                    }
                };
                crate::generate::generate(port, &params_for_run, &mut stage, &mut job)
            })
            .join();

            *state()
                .backend_job
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = None;

            match result {
                Ok(Ok(outcome)) => finish(
                    &job_id,
                    &outcome.tracks,
                    outcome.cancelled,
                    None,
                ),
                Ok(Err(err)) => finish(&job_id, &[], false, Some(err)),
                Err(_) => finish(
                    &job_id,
                    &[],
                    false,
                    Some("the generation thread panicked".to_string()),
                ),
            }
        }
    });
}

/// Block until the engine is up, giving up after a minute so a queued job does
/// not sit in Running forever with nothing running.
fn wait_for_engine() -> Option<u16> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while std::time::Instant::now() < deadline {
        if let Some(port) = crate::server::running_port() {
            return Some(port);
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every other field has a serde default, so a bare caption is a valid
    /// `GenerateParams`. Building it this way keeps the test from having to name
    /// fields that the struct may later add.
    fn params_with_caption(caption: &str) -> GenerateParams {
        serde_json::from_value(serde_json::json!({ "caption": caption }))
            .expect("GenerateParams accepts a caption alone")
    }

    fn job(state: JobState, audio: f32, elapsed: f64) -> QueuedJob {
        QueuedJob {
            id: "q1".into(),
            params: params_with_caption(""),
            state,
            error: None,
            track_ids: Vec::new(),
            created_at: 0,
            started_at: None,
            finished_at: None,
            audio_secs: audio,
            elapsed_secs: elapsed,
        }
    }

    #[test]
    fn render_ratio_is_audio_seconds_per_generation_second() {
        let j = job(JobState::Done, 60.0, 20.0);
        let r = j.render_ratio().expect("a finished job with clocks has a ratio");
        assert!((r - 3.0).abs() < 0.001, "expected 3.0, got {r}");
    }

    #[test]
    fn render_ratio_absent_for_unfinished_or_clockless_jobs() {
        assert!(job(JobState::Running, 60.0, 20.0).render_ratio().is_none());
        assert!(job(JobState::Failed, 0.0, 20.0).render_ratio().is_none());
        // A generation fast enough to finish before the clock ticked must not
        // divide by zero.
        assert!(job(JobState::Done, 60.0, 0.0).render_ratio().is_none());
    }

    #[test]
    fn interrupted_is_terminal_but_still_needs_a_decision() {
        assert!(!JobState::Interrupted.is_finished());
        assert!(!JobState::Interrupted.is_active());
        assert!(JobState::Running.is_active());
        assert!(JobState::Done.is_finished());
    }

    #[test]
    fn label_truncates_long_captions_without_splitting_a_char() {
        let mut j = job(JobState::Queued, 0.0, 0.0);
        j.params.caption = "é".repeat(80);
        let label = j.label();
        assert_eq!(label.chars().count(), 60, "60 chars plus ellipsis");
        assert!(label.ends_with('…'));
        // The truncation must be on a char boundary, not a byte.
        assert!(j.params.caption.starts_with(&label[..label.len() - '…'.len_utf8()]));
    }

    #[test]
    fn states_serialise_as_lowercase_for_the_frontend() {
        assert_eq!(
            serde_json::to_string(&JobState::Interrupted).unwrap(),
            "\"interrupted\""
        );
    }
}