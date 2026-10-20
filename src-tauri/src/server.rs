//! Supervises the `ace-server` sidecar.
//!
//! Adapted from Atomic-Chat's pattern (`core/atomic_core/launch.rs`): spawn
//! detached, poll `/health` for readiness, reap finished children so a stale
//! PID cannot block a restart, and allow a dev override via env var.

use serde::Serialize;

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::os::unix::process::CommandExt;
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

/// Which child owns `RUNNING_PORT`. A port outlives the process that was
/// listening on it, and a stale port turns every later request into a bare
/// "connection refused" that names nothing.
static RUNNING_PID: Mutex<Option<u32>> = Mutex::new(None);

pub fn running_port() -> Option<u16> {
    *RUNNING_PORT.lock().unwrap_or_else(|e| e.into_inner())
}

/// Whether a spawned engine is still running.
///
/// Deliberately does not touch the recorded port: this is asked from the render
/// poll to tell "the engine exited" from "the engine is too busy to answer",
/// and clearing the port out from under a render that is still going would be
/// the wrong way to lose one.
pub fn engine_alive() -> bool {
    reap_finished();
    let mut children = lock_children();
    // Nothing tracked: this app did not start the engine, so it has no opinion.
    children.is_empty() || children.iter_mut().any(|c| matches!(c.try_wait(), Ok(None)))
}

/// Reap anything that has exited and report the port only while the engine
/// behind it is still alive.
///
/// `SPAWNED` was only swept during the startup handshake, so an engine that
/// died mid-session stayed an unreaped zombie and its port stayed on display.
/// The UI is told the runtime is live until it is stopped by hand, and the
/// first symptom is a connect error against a port nothing is listening on.
pub fn poll_liveness() -> Option<u16> {
    reap_finished();
    let pid = *RUNNING_PID.lock().unwrap_or_else(|e| e.into_inner());
    let port = running_port();
    let (Some(pid), Some(port)) = (pid, port) else {
        return port;
    };
    let alive = lock_children().iter().any(|c| c.id() == pid);
    if alive {
        return Some(port);
    }
    *RUNNING_PORT.lock().unwrap_or_else(|e| e.into_inner()) = None;
    *RUNNING_PID.lock().unwrap_or_else(|e| e.into_inner()) = None;
    None
}

#[derive(Debug, Clone, Serialize)]
pub struct ServerStatus {
    pub running: bool,
    pub pid: Option<u32>,
    pub port: Option<u16>,
    pub ready: bool,
    pub log_tail: Vec<String>,
    /// Weight files the engine had when it started.
    ///
    /// The backend scans its models folder once, in `main`, and exposes no
    /// rescan. So a weight that arrives later is on disk and visible to this
    /// app, and invisible to the running engine, until it is restarted. The
    /// UI compares this against the folder to say so, because "my model is
    /// there and the app lists it but rendering still uses the old one" has no
    /// other explanation on screen.
    #[serde(default)]
    pub loaded_weights: Vec<String>,
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
/// Ask the kernel to signal this process when its parent dies.
///
/// The sidecar is a long-lived child that loads gigabytes onto the GPU, so
/// letting it outlive the window is expensive: `stop()` kills tracked children,
/// but it only runs on a clean Stop, and closing the app or killing it outright
/// leaves an orphan holding a CUDA context and a listening port. Three
/// accumulated here over a day, each a wasted 46 MB on a 4 GB card -- and one of
/// them part of why a render could not get the memory it asked for.
///
/// PR_SET_PDEATHSIG is the kernel's own answer, and it closes the window a
/// shutdown hook leaves open: if the parent has already gone by the time the
/// child runs this, it exits immediately rather than becoming an orphan again.
///
/// Runs in the forked child between fork and exec, so only async-signal-safe
/// work belongs here. `prctl` is.
fn die_with_parent() -> std::io::Result<()> {
    use std::os::raw::{c_int, c_ulong};
    const PR_SET_PDEATHSIG: c_int = 1;
    const SIGTERM: c_int = 15;
    // Declared rather than imported: the C integer types are in std, so this
    // avoids a new dependency in order to avoid one.
    extern "C" {
        fn prctl(option: c_int, arg2: c_ulong, ...) -> c_int;
    }
    if unsafe { prctl(PR_SET_PDEATHSIG, SIGTERM as c_ulong, 0, 0, 0) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// Which engine the running sidecar is.
///
/// Set when a server starts. The two engines share an HTTP surface but not a
/// request format, and nothing else distinguishes them at runtime.
///
/// A lock rather than a `OnceLock`, and that distinction matters. The engine
/// can be switched in the sidebar and started again in the same session, and a
/// `OnceLock` accepts a value exactly once: the second start would have been
/// ignored and the app would have carried on shaping requests for the engine
/// that was no longer running. That is the same 400 this whole change is
/// about, coming back after a restart.
static RUNNING_ENGINE: std::sync::RwLock<Option<crate::engines::EngineId>> =
    std::sync::RwLock::new(None);

fn set_running_engine(engine: crate::engines::EngineId) {
    if let Ok(mut slot) = RUNNING_ENGINE.write() {
        *slot = Some(engine);
    }
}

/// The engine currently running, if any.
pub fn running_engine() -> Option<crate::engines::EngineId> {
    RUNNING_ENGINE.read().ok().and_then(|slot| *slot)
}

pub fn start(
    engine: crate::engines::EngineId,
    models_dir: &std::path::Path,
    resource_dir: Option<&std::path::Path>,
) -> Result<ServerStatus, String> {
    reap_finished();
    let hot = engine == crate::engines::EngineId::HotStep;
    // Remembered so a request can be shaped for the engine that is actually
    // running. The two engines disagree about the request format and the app
    // cannot tell them apart by port.
    set_running_engine(engine);

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

        let mut command = Command::new(&program);
    // Unsafe at the call site rather than around the whole chain: `pre_exec` is
    // an unsafe fn precisely because it runs in the forked child.
    unsafe {
        command.pre_exec(die_with_parent);
    }
    let child = command
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
            *RUNNING_PID.lock().unwrap_or_else(|e| e.into_inner()) = Some(pid);
            return Ok(ServerStatus {
                running: true,
                pid: Some(pid),
                port: Some(port),
                ready: true,
                log_tail: tail_log(&log_path),
                loaded_weights: record_loaded_weights(&models_dir),
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
        loaded_weights: record_loaded_weights(&models_dir),
    })
}

/// What was on disk at the moment the engine was started.
///
/// Read from the folder rather than from the engine, because the point is to
/// compare it against the folder *now* and report what has appeared since.
fn record_loaded_weights(models_dir: &Path) -> Vec<String> {
    crate::store::scan_local(models_dir)
        .into_iter()
        .filter(|f| f.present)
        .map(|f| f.filename)
        .collect()
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

/// The offset just past the last complete line.
///
/// The log viewer resumes from this rather than from the raw length. The engine
/// writes in whatever chunks it likes, so a poll routinely lands mid-line: the
/// raw length would then be treated as a line boundary, and the remainder of
/// that line would arrive at the next poll as a second, headless line. Long
/// paths and progress bars -- which are exactly what you go looking at -- split
/// across two entries.
///
/// Deliberately not folded into `stderr_len`, which feeds the stall watchdog.
/// That one wants to notice a log growing at all, and rounding to a line
/// boundary would make it slightly deaf to a partial write.
pub fn stderr_len_complete(models_dir: &Path) -> u64 {
    use std::io::{Read, Seek, SeekFrom};
    let path = stderr_log_path(models_dir);
    let Ok(len) = std::fs::metadata(&path).map(|m| m.len()) else {
        return 0;
    };
    if len == 0 {
        return 0;
    }
    // A long tail is enough to find the last newline in, and not reading the
    // whole file every two seconds is the point of having a cursor at all.
    const TAIL: u64 = 64 * 1024;
    let from = len.saturating_sub(TAIL);
    let Ok(mut file) = std::fs::File::open(&path) else {
        return len;
    };
    if file.seek(SeekFrom::Start(from)).is_err() {
        return len;
    }
    let mut buf = Vec::new();
    if file.read_to_end(&mut buf).is_err() {
        return len;
    }
    match buf.iter().rposition(|b| *b == b'\n') {
        // +1 for the newline itself.
        Some(i) => from + i as u64 + 1,
        // No newline in the tail at all: one enormous line, or a write that
        // has not landed yet. Fall back to the length rather than stalling.
        None => len,
    }
}

/// Empty the log.
///
/// Truncated in place rather than unlinked. The sidecar was handed this path as
/// its stderr when it was spawned, so it holds an open appending descriptor:
/// replacing the file would leave it writing into an unlinked inode and the log
/// would silently stop growing. `set_len(0)` keeps the same inode, so the
/// engine carries on appending to a file the app can still see.
pub fn stderr_clear(models_dir: &Path) -> std::io::Result<()> {
    let path = stderr_log_path(models_dir);
    if !path.exists() {
        return Ok(());
    }
    std::fs::OpenOptions::new().write(true).open(&path)?.set_len(0)
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
    *RUNNING_PID.lock().unwrap_or_else(|e| e.into_inner()) = None;
    Ok(killed)
}

/// `/props` gives the frontend its model list and the server's default
/// request, so the UI does not hardcode either.
pub fn props(port: u16) -> Result<serde_json::Value, String> {
    let body = http_get(port, "/props").ok_or_else(|| "server not reachable".to_string())?;
    serde_json::from_str(&body).map_err(|e| format!("bad /props payload: {e}"))
}
    /// The engine can be switched in the sidebar and started again in the same
    /// session. A `OnceLock` takes a value exactly once, so the second start
    /// would have been ignored and the app would have kept shaping requests
    /// for the engine that was no longer running -- which is the 400 this
    /// whole change is about, returning after a restart.
    #[test]
    fn the_running_engine_follows_a_switch() {
        set_running_engine(crate::engines::EngineId::AceStep);
        assert_eq!(running_engine(), Some(crate::engines::EngineId::AceStep));
        set_running_engine(crate::engines::EngineId::HotStep);
        assert_eq!(
            running_engine(),
            Some(crate::engines::EngineId::HotStep),
            "switching engines has to take effect, or requests keep their old shape"
        );
        // Put it back, so a later test does not inherit a HOT-Step engine.
        set_running_engine(crate::engines::EngineId::AceStep);
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

    /// Clearing has to empty the log and report a zero cursor, or the viewer's
    /// next incremental poll resumes from an offset past the end and the panel
    /// sits empty.
    #[test]
    fn clearing_empties_the_log() {
        let dir = Scratch::new("clear");
        dir.write("[Server] a\n[Server] b\n");
        assert!(!stderr_tail(&dir.0, 80).is_empty());
        stderr_clear(&dir.0).expect("clear succeeds");
        assert_eq!(stderr_len(&dir.0), 0);
        assert!(stderr_tail(&dir.0, 80).is_empty());
    }

    /// The reason this truncates instead of unlinking. The sidecar holds this
    /// path open as its stderr, so replacing the file would leave it appending
    /// to an unlinked inode: the log would stop growing and the viewer would go
    /// permanently blank with no error anywhere. Same inode in, same inode out.
    #[test]
    fn clearing_keeps_the_inode_the_engine_writes_to() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let dir = Scratch::new("inode");
            dir.write("before\n");
            let before = std::fs::metadata(stderr_log_path(&dir.0)).unwrap().ino();
            stderr_clear(&dir.0).expect("clear succeeds");
            let after = std::fs::metadata(stderr_log_path(&dir.0)).unwrap().ino();
            assert_eq!(before, after, "the engine's open descriptor now writes nowhere");
        }
    }

    /// Clearing a log that was never created is a no-op, not an error: the
    /// button is in the UI from the moment the app opens.
    #[test]
    fn clearing_a_missing_log_is_not_an_error() {
        let dir = Scratch::new("clear-absent");
        stderr_clear(&dir.0).expect("nothing to clear is still success");
    }

    /// A cursor that lands mid-line splits that line in two across polls. The
    /// engine writes in arbitrary chunks, so this is the normal case rather
    /// than an edge one.
    #[test]
    fn the_cursor_lands_on_a_line_boundary() {
        let dir = std::env::temp_dir().join(format!("aceforge-log-{}", std::process::id()));
        let yue2 = dir.join("yue2");
        std::fs::create_dir_all(&yue2).unwrap();
        std::fs::write(stderr_log_path(&dir), b"first\nsecond\npart").unwrap();
        let cursor = stderr_len_complete(&dir);
        assert_eq!(cursor, b"first\nsecond\n".len() as u64);

        // The half-written line has not been shown, and when it completes it
        // arrives whole. With the raw length as the cursor the viewer would
        // have shown "part" already and "ial" next, as two headless lines.
        std::fs::write(stderr_log_path(&dir), b"first\nsecond\npartial\n").unwrap();
        assert_eq!(stderr_since(&dir, cursor, 1000), vec!["partial".to_string()]);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A log with no trailing newline has no boundary to offer, and stalling
    /// the viewer forever would be worse than overshooting slightly.
    #[test]
    fn a_log_with_no_newline_does_not_stall_the_cursor() {
        let dir = std::env::temp_dir().join(format!("aceforge-log2-{}", std::process::id()));
        let yue2 = dir.join("yue2");
        std::fs::create_dir_all(&yue2).unwrap();
        std::fs::write(stderr_log_path(&dir), b"no newline anywhere").unwrap();
        assert_eq!(stderr_len_complete(&dir), b"no newline anywhere".len() as u64);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The watchdog still gets the raw length. Rounding to a line boundary
    /// would make it slightly deaf to a partial write, and that is the one
    /// signal it has.
    #[test]
    fn the_watchdog_still_gets_the_raw_length() {
        let dir = std::env::temp_dir().join(format!("aceforge-log3-{}", std::process::id()));
        let yue2 = dir.join("yue2");
        std::fs::create_dir_all(&yue2).unwrap();
        std::fs::write(stderr_log_path(&dir), b"line\npartial").unwrap();
        assert_eq!(stderr_len(&dir), b"line\npartial".len() as u64);
        assert!(stderr_len(&dir) > stderr_len_complete(&dir));
        std::fs::remove_dir_all(&dir).ok();
    }
}
