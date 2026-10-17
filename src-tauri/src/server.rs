//! Supervises the `ace-server` sidecar.
//!
//! Adapted from Atomic-Chat's pattern (`core/atomic_core/launch.rs`): spawn
//! detached, poll `/health` for readiness, reap finished children so a stale
//! PID cannot block a restart, and allow a dev override via env var.

use serde::Serialize;

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Dev escape hatch: run the server from a build tree instead of the bundle.
const SERVER_ENV: &str = "ACEFORGE_ACE_CMD";
const PORT_ENV: &str = "ACEFORGE_PORT";

const READY_TIMEOUT: Duration = Duration::from_secs(90);
const POLL: Duration = Duration::from_millis(250);

/// Reaped children are parked here. Without this a finished `Child` stays in
/// the table and its PID keeps blocking restarts.
static SPAWNED: Mutex<Vec<Child>> = Mutex::new(Vec::new());

/// Port of the live sidecar. The UI needs this for every /job call, and it is
/// only known once the readiness handshake has picked a free port.
static RUNNING_PORT: Mutex<Option<u16>> = Mutex::new(None);

pub fn running_port() -> Option<u16> {
    *RUNNING_PORT.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Debug, Clone, Serialize)]
pub struct ServerStatus {
    pub running: bool,
    pub pid: Option<u32>,
    pub port: Option<u16>,
    pub ready: bool,
    pub log_tail: Vec<String>,
}

fn lock_children() -> std::sync::MutexGuard<'static, Vec<Child>> {
    SPAWNED.lock().unwrap_or_else(|e| e.into_inner())
}

fn reap_finished() {
    let mut children = lock_children();
    children.retain_mut(|c| matches!(c.try_wait(), Ok(None)));
}

/// Locate the bundled sidecar. The `resource_dir` is handed in by the command
/// layer because Tauri v2 has no ambient app handle to query here.
pub fn bundled_server_path(
    resource_dir: Option<&std::path::Path>,
    hot: bool,
) -> Option<PathBuf> {
    let dir = resource_dir?;
    // Two runtimes are bundled, both shipping a file called `ace-server`. Picking
    // the wrong one loads the wrong ggml against the wrong libraries.
    let name = if hot {
        "hotstep-runtime"
    } else {
        "aceforge-runtime"
    };
    let bundled = dir.join(name).join("ace-server");
    bundled.is_file().then_some(bundled)
}

/// Dev fallback: look for an `ace-server*` near the executable or in a build
/// tree. Ignored once the app is packaged.
fn dev_server_path(_hot: bool) -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let mut dir = exe.parent()?.to_path_buf();
    for _ in 0..3 {
        for sub in ["", "resources/runtime", "bin"] {
            let scan = dir.join(sub);
            if !scan.is_dir() {
                continue;
            }
            let mut hits: Vec<PathBuf> = std::fs::read_dir(&scan)
                .ok()?
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with("ace-server"))
                        && p.is_file()
                })
                .collect();
            if let Some(found) = hits.pop() {
                return Some(found);
            }
        }
        dir = dir.parent()?.to_path_buf();
    }
    None
}

fn http_get(port: u16, path: &str) -> Option<String> {
    let url = format!("http://127.0.0.1:{port}{path}");
    ureq::get(&url)
        .timeout(Duration::from_secs(5))
        .call()
        .ok()?
        .into_string()
        .ok()
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .unwrap_or(8085)
}

/// Start `engine` against `models_dir`, returning once `/health` answers.
///
/// The two engines share a binary name and an HTTP surface, so which one is
/// being launched is the only thing that distinguishes a run. Getting it wrong
/// shows up as unknown request fields being ignored, which looks like the
/// parameter having no effect rather than like the wrong engine.
pub fn start(
    engine: crate::engines::EngineId,
    models_dir: &std::path::Path,
    resource_dir: Option<&std::path::Path>,
) -> Result<ServerStatus, String> {
    reap_finished();
    let hot = engine == crate::engines::EngineId::HotStep;

    // Resolution order: explicit override (the ACE-Step engine only, since it
    // is the one a developer is likely to have built by hand), then the engine
    // registry, then the bundled resource, then a dev build tree.
    let program = if hot {
        None
    } else {
        std::env::var(SERVER_ENV).map(PathBuf::from).ok()
    }
        .or_else(|| crate::engines::binary_path(engine))
        .or_else(|| bundled_server_path(resource_dir, hot))
        .or_else(|| dev_server_path(hot))
        .ok_or_else(|| {
            format!("ace-server not found. Set {SERVER_ENV}=/path/to/ace-server")
        })?;

    if !program.is_file() {
        return Err(format!("ace-server not executable: {}", program.display()));
    }
    if !models_dir.is_dir() {
        return Err(format!(
            "models dir missing: {} (create it, add weights, then start)",
            models_dir.display()
        ));
    }

    let port: u16 = std::env::var(PORT_ENV)
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or_else(free_port);

    let log_path = models_dir.join("ace-server.log");

    // Tile size is the memory knob. 1024 is the upstream default and OOMs a
    // 4 GB card during VAE decode; 256 was measured working.
    let config = crate::settings::load();
    let vae_chunk = config.vae_chunk.clamp(64, 1024);
    // Both of these default upstream to values that do not fit 4 GB.
    let max_seq = config.max_seq.clamp(1024, 32768);

        let child = Command::new(&program)
        .arg("--models")
        .arg(models_dir)
        .arg("--host")
        .arg("127.0.0.1")
        .arg("--port")
        .arg(port.to_string())
        .arg("--vae-chunk")
        .arg(vae_chunk.to_string())
        .arg("--max-seq")
        .arg(max_seq.to_string())
        // LoRA registry. The flag is only passed when the directory exists,
        // because `ace-server` scans it eagerly and would otherwise log a
        // missing directory on every start. Layout matches `registry_scan_adapters`:
        // `*.safetensors` files, or subdirectories holding
        // `adapter_model.safetensors`.
        .args(adapter_args(models_dir, hot))
        .stdout(Stdio::null())
        // Keep the diagnostics. The backend explains model-not-found and OOM
        // failures on stderr, and discarding them turns every failure into an
        // unexplained "job failed".
        .stderr(Stdio::from(
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(stderr_log_path(models_dir))
                .map_err(|e| format!("cannot open server log: {e}"))?,
        ))
        .spawn()
        .map_err(|e| format!("spawn failed: {e}"))?;

    let pid = child.id();
    lock_children().push(child);

    let _ = std::fs::write(&log_path, format!("started pid {pid} on port {port}\n"));

    // Readiness handshake.
    let deadline = Instant::now() + READY_TIMEOUT;
    while Instant::now() < deadline {
        reap_finished();
        if http_get(port, "/health").is_some() {
            *RUNNING_PORT.lock().unwrap_or_else(|e| e.into_inner()) = Some(port);
            return Ok(ServerStatus {
                running: true,
                pid: Some(pid),
                port: Some(port),
                ready: true,
                log_tail: tail_log(&log_path),
            });
        }
        std::thread::sleep(POLL);
    }

    Ok(ServerStatus {
        running: false,
        pid: Some(pid),
        port: Some(port),
        ready: false,
        log_tail: tail_log(&log_path),
    })
}

/// Where LoRA adapters live, by convention under the models directory.
pub fn adapters_dir(models_dir: &Path) -> PathBuf {
    models_dir.join("adapters")
}

/// `--adapters <dir>`, but only when there is something in it.
fn adapter_args(models_dir: &Path, hot: bool) -> Vec<std::ffi::OsString> {
    // HOT-Step keeps adapters in its own setting, because it also accepts
    // rank-suffixed LoKr files the ACE-Step layout does not describe.
    let dir = if hot {
        crate::settings::resolve_hotstep_adapters_dir()
            .unwrap_or_else(|| adapters_dir(models_dir))
    } else {
        adapters_dir(models_dir)
    };
    let has_entries = std::fs::read_dir(&dir)
        .map(|mut e| e.any(|entry| {
            entry
                .map(|e| {
                    e.path()
                        .file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n != ".gitkeep")
                })
                .unwrap_or(false)
        }))
        .unwrap_or(false);
    if !has_entries {
        return Vec::new();
    }
    vec![
        std::ffi::OsString::from("--adapters"),
        dir.into_os_string(),
    ]
}

fn tail_log(path: &std::path::Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let lines: Vec<String> = text.lines().map(|s| s.to_string()).collect();
    let start = lines.len().saturating_sub(20);
    lines[start..].to_vec()
}

/// Where the sidecar's stderr is kept. `ace-server.log` with its extension
/// swapped, so the two logs sit side by side.
pub fn stderr_log_path(models_dir: &Path) -> PathBuf {
    models_dir.join("ace-server.log").with_extension("stderr.log")
}

/// Byte length of the stderr log right now. Take this before submitting a job
/// and hand it to `stderr_since` when the job fails.
///
/// A failed job carries no error field -- `Job` in `ace-server.cpp` has nowhere
/// to put one, and `/job` answers a bare `{"status":"failed"}` -- so this log is
/// the only place the real explanation exists. Marking the offset first is what
/// stops the previous run's progress lines from being reported as this run's
/// failure.
pub fn stderr_mark(models_dir: &Path) -> u64 {
    std::fs::metadata(stderr_log_path(models_dir))
        .map(|m| m.len())
        .unwrap_or(0)
}

/// Lines the backend appended to stderr after `mark`, oldest first.
///
/// Everything after the mark belongs to the job that just ran, so the caller
/// can filter aggressively without dragging in unrelated history.
pub fn stderr_since(models_dir: &Path, mark: u64, max_lines: usize) -> Vec<String> {
    stderr_after(models_dir, mark, max_lines)
}

/// The last `n` lines the backend wrote, oldest first. For the log viewer,
/// which deliberately reads from the top rather than only recent output.
pub fn stderr_tail(models_dir: &Path, max_lines: usize) -> Vec<String> {
    stderr_after(models_dir, 0, max_lines)
}

/// Current length of the log: where the next incremental poll resumes from.
pub fn stderr_len(models_dir: &Path) -> u64 {
    stderr_mark(models_dir)
}

/// Read the stderr log from `mark` to the end.
///
/// `mark` of 0 means "the whole file". Split out so the log viewer (which reads
/// from the top on purpose) and the failure path (which reads only what this
// job wrote) share one implementation.
fn stderr_after(models_dir: &Path, mark: u64, max_lines: usize) -> Vec<String> {
    use std::io::{Read, Seek, SeekFrom};

    let path = stderr_log_path(models_dir);
    let Ok(len) = std::fs::metadata(&path).map(|m| m.len()) else {
        return Vec::new();
    };
    // The log was truncated or replaced underneath us; there is nothing to read.
    if mark > len {
        return Vec::new();
    }
    let Ok(mut file) = std::fs::File::open(&path) else {
        return Vec::new();
    };
    if file.seek(SeekFrom::Start(mark)).is_err() {
        return Vec::new();
    }
    let mut buf = Vec::new();
    if file.read_to_end(&mut buf).is_err() {
        return Vec::new();
    }

    let text = String::from_utf8_lossy(&buf);
    let lines: Vec<String> = text.lines().map(|s| s.to_string()).collect();
    let start = lines.len().saturating_sub(max_lines);
    lines[start..].to_vec()
}

/// Stop every tracked child. Returns the number signalled.
pub fn stop() -> Result<usize, String> {
    let mut children = lock_children();
    let mut killed = 0;
    for child in children.iter_mut() {
        if matches!(child.try_wait(), Ok(None)) {
            let _ = child.kill();
            killed += 1;
        }
    }
    children.clear();
    *RUNNING_PORT.lock().unwrap_or_else(|e| e.into_inner()) = None;
    Ok(killed)
}

/// `/props` gives the frontend its model list and the server's default
/// request, so the UI does not hardcode either.
pub fn props(port: u16) -> Result<serde_json::Value, String> {
    let body = http_get(port, "/props").ok_or_else(|| "server not reachable".to_string())?;
    serde_json::from_str(&body).map_err(|e| format!("bad /props payload: {e}"))
}
#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch(PathBuf);

    impl Scratch {
        /// Unique per test: these run in parallel in one process, so a shared
        /// directory would have them trampling each other's logs.
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "aceforge-server-test-{tag}-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("scratch dir");
            Self(dir)
        }

        /// Append to the stderr log the way the sidecar does.
        fn write(&self, text: &str) {
            use std::io::Write;
            let path = stderr_log_path(&self.0);
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .expect("open log");
            f.write_all(text.as_bytes()).expect("write log");
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// The whole point of the mark: a failure must report its own lines, not
    /// the previous render's progress output.
    #[test]
    fn reads_only_what_came_after_the_mark() {
        let dir = Scratch::new("mark");
        dir.write("[Server] previous run\n[MP3] Encoding 30.0s\n");

        let mark = stderr_mark(&dir.0);
        dir.write("[Server] LM not found: x.gguf\n");

        let since = stderr_since(&dir.0, mark, 80);
        assert_eq!(since, vec!["[Server] LM not found: x.gguf".to_string()]);
    }

    #[test]
    fn caps_the_number_of_lines_returned() {
        let dir = Scratch::new("cap");
        let mark = stderr_mark(&dir.0);
        dir.write("a\nb\nc\nd\ne\n");
        let since = stderr_since(&dir.0, mark, 2);
        assert_eq!(since, vec!["d".to_string(), "e".to_string()]);
    }

    /// A mark past the end means the log was rotated or truncated. Returning
    /// stale bytes there would be worse than returning nothing.
    #[test]
    fn tolerates_a_log_shorter_than_the_mark() {
        let dir = Scratch::new("short");
        dir.write("only line\n");
        assert!(stderr_since(&dir.0, 9_999_999, 80).is_empty());
    }

    #[test]
    fn missing_log_is_not_an_error() {
        let dir = Scratch::new("absent");
        assert_eq!(stderr_mark(&dir.0), 0);
        assert!(stderr_since(&dir.0, 0, 80).is_empty());
    }
}
