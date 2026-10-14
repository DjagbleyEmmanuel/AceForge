//! Backfilling lyric timings for songs rendered before the engine could produce
//! them.
//!
//! The alignment the engine runs measures lyric tokens against the audio
//! generated in the *same* pass, so re-rendering a song to time it would
//! produce timings for a different performance. The only correct approach is to
//! feed the existing audio back in and discard the new audio, keeping just the
//! `.lrc` that comes out.
//!
//! That is cheap: the DiT runs for a handful of steps rather than a full
//! render, and the audio it produces is thrown away. Measured at roughly 13 s
//! per track on a GTX 1050, most of which is the DiT pass itself.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::generate::Track;

/// Steps for the throwaway DiT pass.
///
/// Low on purpose -- the audio is discarded, only the cross-attention maps are
/// read. Measured alignment at 4 steps was already accurate; 8 is the turbo
/// model's native count and leaves a margin without costing much.
const ALIGN_STEPS: i32 = 8;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncReport {
    pub path: String,
    pub lines: usize,
    /// Wall-clock seconds this track took.
    pub took_secs: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncProgress {
    pub running: bool,
    pub done: usize,
    pub total: usize,
    pub current: String,
    /// Set when the batch stopped early, e.g. the engine was not running.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Default for SyncProgress {
    fn default() -> Self {
        SyncProgress {
            running: false,
            done: 0,
            total: 0,
            current: String::new(),
            error: None,
        }
    }
}

/// Idempotence lives in the file itself: a track that already has a sidecar is
/// considered done unless `force` says otherwise.
fn lrc_path(audio: &Path) -> PathBuf {
    audio.with_extension("lrc")
}

/// Count of timestamped lines, so the UI can say something more useful than
/// "done" and so an empty alignment is reported as a failure rather than a
/// silent success.
///
/// A lyric line is `[mm:ss.xx]words` and a metadata tag is `[ti:Title]`; both
/// open with a bracket, so the discriminator is what sits inside it -- a
/// timestamp starts with a digit, a tag key never does. Testing for `]:`
/// instead is wrong: the character after the bracket is the first letter of the
/// lyric, not a colon.
fn count_timed_lines(lrc: &str) -> usize {
    lrc.lines()
        .filter(|l| {
            let l = l.trim();
            let Some(close) = l.strip_prefix('[').and_then(|r| r.find(']')) else {
                return false;
            };
            close > 0 && l[1..=close].starts_with(|c: char| c.is_ascii_digit())
        })
        .count()
}

/// Whether this track is worth trying: it needs lyrics that are not just an
/// instrumental marker, because there is nothing to align otherwise.
fn has_singable_lyrics(track: &Track) -> bool {
    let lyrics = track.lyrics.trim();
    !lyrics.is_empty() && !lyrics.eq_ignore_ascii_case("[instrumental]")
}

/// Sync one track. Returns the number of timed lines written, or an error.
pub fn sync_track(port: u16, track: &Track, force: bool) -> Result<SyncReport, String> {
    let started = std::time::Instant::now();
    let audio = PathBuf::from(&track.path);
    if !audio.is_file() {
        return Err(format!("{} is missing", audio.display()));
    }
    if !has_singable_lyrics(track) {
        return Err("instrumental — nothing to align".to_string());
    }
    if !force && lrc_path(&audio).is_file() {
        return Ok(SyncReport {
            path: track.path.clone(),
            lines: count_timed_lines(
                &std::fs::read_to_string(lrc_path(&audio)).unwrap_or_default(),
            ),
            took_secs: 0.0,
            error: None,
        });
    }

    // The track's own stored duration, not a guess. An under-reported duration
    // silently truncates the alignment: a 32 s track asked for 20 s produces
    // timings that stop two thirds of the way in.
    let duration = if track.duration_secs > 1.0 {
        track.duration_secs
    } else {
        // Fall back to the metadata sidecar, then to a generous ceiling so the
        // caller gets full coverage rather than a short answer.
        duration_from_sidecar(&audio).unwrap_or(600.0)
    };

    let request = serde_json::json!([{
        "caption": if track.caption.trim().is_empty() { "instrumental" } else { track.caption.trim() },
        "lyrics": track.lyrics,
        "duration": duration,
        "inference_steps": ALIGN_STEPS,
        "synth_batch_size": 1,
        "guidance_scale": 0,
        "shift": 3,
        "seed": 0,
        "get_lrc": true,
    }]);

    let id = crate::generate::submit_synth_with_audio(port, &request, Some(&audio), None)?;
    let state = crate::generate::wait_for_job(port, &id, 0, &mut |_| {})?;
    if state == crate::generate::JobState::Cancelled {
        return Err("cancelled".to_string());
    }

    let fetched = crate::generate::fetch_result(port, &id)?;
    let lrc = fetched
        .lrc
        .ok_or_else(|| "the engine returned no lyric timings".to_string())?;
    let lines = count_timed_lines(&lrc);
    if lines == 0 {
        return Err("the alignment came back empty".to_string());
    }

    let out = lrc_path(&audio);
    std::fs::write(&out, &lrc).map_err(|e| format!("cannot write {}: {e}", out.display()))?;

    Ok(SyncReport {
        path: track.path.clone(),
        lines,
        took_secs: started.elapsed().as_secs_f32(),
        error: None,
    })
}

fn duration_from_sidecar(audio: &Path) -> Option<f32> {
    let json = audio.with_extension("json");
    let text = std::fs::read_to_string(json).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    value
        .get("durationSecs")
        .and_then(|v| v.as_f64())
        .map(|v| v as f32)
        .filter(|v| *v > 0.0)
}

// --- batch runner -------------------------------------------------------

static CANCEL: AtomicBool = AtomicBool::new(false);
static PROGRESS: Mutex<Option<SyncProgress>> = Mutex::new(None);

pub fn progress() -> SyncProgress {
    PROGRESS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .unwrap_or_default()
}

fn set_progress(p: SyncProgress) {
    *PROGRESS.lock().unwrap_or_else(|e| e.into_inner()) = Some(p.clone());
    emit(&p);
}

fn emit(p: &SyncProgress) {
    if let Some(app) = crate::app_handle() {
        use tauri::Emitter;
        let _ = app.emit("lyricsync://progress", p);
    }
}

/// Tracks that could be synced: has lyrics, and has no sidecar yet.
pub fn pending() -> Vec<Track> {
    crate::generate::list_tracks()
        .into_iter()
        .filter(|t| has_singable_lyrics(t))
        .filter(|t| !lrc_path(Path::new(&t.path)).is_file())
        .collect()
}

pub fn cancel() {
    CANCEL.store(true, Ordering::SeqCst);
}

/// Start a background sync. Runs one track at a time: each pass acquires the
/// DiT, and overlapping passes would fight over VRAM on a 4 GB card.
pub fn start(app: tauri::AppHandle, paths: Vec<String>) {
    crate::set_app_handle(app);
    if progress().running {
        return;
    }
    CANCEL.store(false, Ordering::SeqCst);

    let total = paths.len();
    std::thread::spawn(move || {
        let Some(port) = crate::server::running_port() else {
            set_progress(SyncProgress {
                running: false,
                done: 0,
                total,
                current: String::new(),
                error: Some("start the engine first".to_string()),
            });
            return;
        };

        let all = crate::generate::list_tracks();
        let mut done = 0usize;

        for path in paths {
            if CANCEL.load(Ordering::SeqCst) {
                break;
            }
            let Some(track) = all.iter().find(|t| t.path == path) else {
                done += 1;
                continue;
            };
            let name = Path::new(&track.path)
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| track.id.clone());

            set_progress(SyncProgress {
                running: true,
                done,
                total,
                current: name.clone(),
                error: None,
            });

            // A failure on one track must not abandon the rest: the batch is
            // long enough that one bad file should not cost the user the other
            // thirty-nine.
            let mut last_error = None;
            if let Err(e) = sync_track(port, track, false) {
                last_error = Some(e);
            }

            done += 1;
            set_progress(SyncProgress {
                running: done < total,
                done,
                total,
                current: name,
                error: last_error,
            });
        }

        set_progress(SyncProgress {
            running: false,
            done,
            total,
            current: String::new(),
            error: None,
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timed_lines_are_counted_and_metadata_ignored() {
        let lrc = "[ti:Title]\n[ar:Someone]\n[00:01.00]first\n[00:04.50]second\n\n";
        assert_eq!(count_timed_lines(lrc), 2);
    }

    #[test]
    fn an_empty_alignment_counts_as_zero() {
        // The point of counting is to refuse to write an empty sidecar that the
        // UI would then present as "synced".
        assert_eq!(count_timed_lines(""), 0);
        assert_eq!(count_timed_lines("[ti:Only metadata]\n"), 0);
    }

    #[test]
    fn instrumental_tracks_are_skipped() {
        let mk = |lyrics: &str| Track {
            id: "x".into(),
            path: "/library/x.mp3".into(),
            format: "mpeg".into(),
            bytes: 0,
            caption: String::new(),
            lyrics: lyrics.into(),
            bpm: 0,
            keyscale: String::new(),
            timesignature: String::new(),
            vocal_language: String::new(),
            duration_secs: 30.0,
            created_at: 0,
            cover: None,
        };
        assert!(!has_singable_lyrics(&mk("")));
        assert!(!has_singable_lyrics(&mk("  [Instrumental]  ")));
        assert!(!has_singable_lyrics(&mk("[instrumental]")));
        assert!(has_singable_lyrics(&mk("[verse]\nhello")));
    }

    #[test]
    fn the_sidecar_sits_beside_the_audio_with_an_lrc_extension() {
        let p = lrc_path(Path::new("/library/song.mp3"));
        assert_eq!(p, PathBuf::from("/library/song.lrc"));
        // A WAV must not collide with the audio's own extension.
        let w = lrc_path(Path::new("/library/song.wav"));
        assert_eq!(w, PathBuf::from("/library/song.lrc"));
    }

    #[test]
    fn progress_starts_idle_and_zeroed() {
        // The UI reads this before any batch has run; it must not panic and
        // must not claim to be busy.
        let p = SyncProgress::default();
        assert!(!p.running);
        assert_eq!(p.done, 0);
        assert!(p.error.is_none());
    }

    #[test]
    fn align_steps_stays_low_because_the_audio_is_discarded() {
        // A regression guard: this is a throwaway pass, and a full render per
        // track would turn a 13 s job into a 13 minute one.
        assert!(
            ALIGN_STEPS <= 16,
            "alignment pass is running {ALIGN_STEPS} steps; the audio is discarded"
        );
    }
}
