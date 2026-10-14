//! audio.cpp driver.
//!
//! Fundamentally unlike the ACE-Step sidecar:
//!
//! * Configuration is a JSON file written to disk, not command-line flags,
//!   and each model entry needs `family`, `task` and `path`. `task` is the
//!   *operation* (`gen`), not the domain — `music` is rejected.
//! * `POST /v1/tasks/run` blocks for the whole render. There is no job id, no
//!   polling and no cancellation: upstream states a wedged GPU call cannot be
//!   interrupted from userspace. The only way to stop a run is to kill the
//!   process, which is what `stop()` does.
//! * The result carries audio as Base64 WAV in `result.audio`.
//!
//! CPU rendering is slow but works: measured loading a 3B YuE2 model and
//! entering generation with no error, on a machine where the same weights do
//! not fit in 4 GB of VRAM.

use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

/// YuE2 on CPU takes many minutes. Upstream's default busy timeout is five
/// minutes, which is not enough, so it is raised here rather than at the call
/// site.
const BUSY_TIMEOUT_MS: u32 = 3_600_000;

static CHILD: Mutex<Option<Child>> = Mutex::new(None);
static PORT: Mutex<Option<u16>> = Mutex::new(None);
static CANCELLED: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelEntry {
    pub id: String,
    pub family: String,
    pub task: String,
    pub mode: String,
    pub path: String,
    /// Load-time options pinned to the exact files this view contains.
    ///
    /// audio.cpp resolves a family's components by hardcoded filename and
    /// throws when they are missing -- `yue2.model_gguf` defaults to
    /// `yue2-3b-q8_0.gguf` with no discovery fallback. A per-quant view that
    /// holds `yue2-3b-q4_0.gguf` therefore has to say so explicitly, or
    /// selecting that quant dies at load time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_options: Option<std::collections::BTreeMap<String, String>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeStatus {
    pub running: bool,
    pub port: Option<u16>,
    pub models: Vec<ModelEntry>,
    pub error: Option<String>,
}

fn running_port() -> Option<u16> {
    *PORT.lock().unwrap_or_else(|e| e.into_inner())
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .unwrap_or(8188)
}

fn get(path: &str) -> Option<String> {
    let port = running_port()?;
    ureq::get(&format!("http://127.0.0.1:{port}{path}"))
        .timeout(Duration::from_secs(10))
        .call()
        .ok()?
        .into_string()
        .ok()
}

fn config_dir() -> PathBuf {
    crate::paths::app_data_dir().join("audiocpp")
}

/// Write the server config.
///
/// `models` are directories, each holding the GGUF plus its sidecar configs.
/// audio.cpp reads them from disk, so a family with no embedded spec still
/// works — which is why pointing this at an LM Studio snapshot works.
fn write_config(port: u16, models: &[ModelEntry]) -> Result<PathBuf, String> {
    let dir = config_dir();
    let knobs = crate::settings::load();
    write_config_at(&dir, port, models, &knobs)
}

/// The config writer proper, with its output directory and knobs passed in so
/// the `threads` handling can be tested without touching real app state.
fn write_config_at(
    dir: &Path,
    port: u16,
    models: &[ModelEntry],
    knobs: &crate::settings::Settings,
) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let path = dir.join("server.json");

    let entries: Vec<serde_json::Value> = models
        .iter()
        .map(|m| {
            let mut obj = serde_json::json!({
                "id": m.id,
                "family": m.family,
                "path": m.path,
                // `task` is the operation, not the domain: music generation is
                // `gen`. Passing "music" is rejected by the server.
                "task": m.task,
                "mode": m.mode,
            });
            if let (Some(target), Some(opts)) = (obj.as_object_mut(), m.session_options.as_ref()) {
                target.insert("session_options".into(), serde_json::json!(opts));
            }
            obj
        })
        .collect();

    let mut config = serde_json::json!({
        "host": "127.0.0.1",
        "port": port,
        "lazy_load": true,
        "max_loaded_models": 1,
        "busy_timeout_ms": BUSY_TIMEOUT_MS,
        "models": entries,
    });

    // audio.cpp exposes backend, device and threads. It has no llama.cpp-style
    // layer offload, so "partial offload" is not a flag that exists to set.
    if let Some(obj) = config.as_object_mut() {
        obj.insert("backend".into(), serde_json::json!(knobs.audiocpp_backend));
        obj.insert("device".into(), serde_json::json!(knobs.audiocpp_device));
        // 0 means "let audio.cpp decide", which it signals by the key being
        // absent. Writing `threads.max(1)` instead made the UI show 0 while
        // the config said 1, and on a 4-core box that quietly cost 3 cores.
        if knobs.audiocpp_threads > 0 {
            obj.insert("threads".into(), serde_json::json!(knobs.audiocpp_threads));
        }
    }

    std::fs::write(&path, serde_json::to_string_pretty(&config).map_err(|e| e.to_string())?)
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(path)
}

/// Start the server and wait for it to answer `/health`.
pub fn start(models: &[ModelEntry]) -> Result<RuntimeStatus, String> {
    stop();

    let program = crate::engines::binary_path(crate::engines::EngineId::AudioCpp)
        .ok_or_else(|| "audio.cpp is not installed".to_string())?;

    if models.is_empty() {
        return Err("no model directories given".to_string());
    }

    let port = free_port();
    let config = write_config(port, models)?;

    let child = Command::new(&program)
        .arg("--config")
        .arg(&config)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("cannot start audio.cpp: {e}"))?;

    *CHILD.lock().unwrap_or_else(|e| e.into_inner()) = Some(child);
    *PORT.lock().unwrap_or_else(|e| e.into_inner()) = Some(port);
    CANCELLED.store(false, Ordering::Relaxed);

    let deadline = std::time::Instant::now() + Duration::from_secs(90);
    while std::time::Instant::now() < deadline {
        if let Some(body) = get("/health") {
            let parsed: serde_json::Value =
                serde_json::from_str(&body).map_err(|e| e.to_string())?;
            return Ok(RuntimeStatus {
                running: parsed.get("status").and_then(|s| s.as_str()) == Some("ok"),
                port: Some(port),
                models: models.to_vec(),
                error: None,
            });
        }
        std::thread::sleep(Duration::from_millis(300));
    }

    stop();
    Err("audio.cpp did not become ready".to_string())
}

pub fn stop() {
    // The documented behaviour: a run cannot be interrupted from userspace, so
    // killing the process is the only way to stop one.
    let mut slot = CHILD.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(child) = slot.as_mut() {
        if matches!(child.try_wait(), Ok(None)) {
            let _ = child.kill();
        }
    }
    *slot = None;
    *PORT.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// Ask the server what it can see. Used to prove a model directory is picked up.
pub fn list_models() -> Result<Vec<ModelEntry>, String> {
    let body = get("/v1/models").ok_or_else(|| "audio.cpp is not running".to_string())?;
    let parsed: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| format!("bad /v1/models: {e}"))?;

    Ok(parsed
        .get("data")
        .and_then(|d| d.as_array())
        .map(|items| {
            items
                .iter()
                .map(|m| ModelEntry {
                    id: m.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    family: m.get("family").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    task: m.get("task").and_then(|v| v.as_str()).unwrap_or("gen").to_string(),
                    mode: m.get("mode").and_then(|v| v.as_str()).unwrap_or("offline").to_string(),
                    path: m.get("path").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    session_options: None,
                })
                .collect()
        })
        .unwrap_or_default())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Rendered {
    pub path: String,
    pub bytes: u64,
    pub seconds: f32,
    /// Wall clock for the blocking request.
    pub elapsed_secs: f64,
    pub sample_rate: u32,
}

/// YuE2 request options.
///
/// Names and ranges are taken from `yue2_cli_interface()` in
/// `src/models/yue2/session.cpp`, not invented: audio.cpp passes the options
/// map straight through, so a key the model does not declare is silently
/// ignored at best. `tags` and `duration` were both being sent and both do not
/// exist for this family -- there is no length control in YuE2's contract.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct YuE2Params {
    /// Style text. YuE2 rejects an empty one outright.
    pub style: String,
    #[serde(default)]
    pub lyrics: String,
    #[serde(default)]
    pub seed: Option<i64>,
    /// Classifier-free guidance. YuE2 declares 0.0 - 20.0, default 1.0.
    #[serde(default)]
    pub guidance_scale: Option<f32>,
    /// NAR ODE steps. YuE2 declares a minimum of 1, default 8.
    #[serde(default)]
    pub num_inference_steps: Option<u32>,
    /// Planning mode: off, melody, full. YuE2 default is off.
    #[serde(default)]
    pub cot: Option<String>,
    /// Last stage to run: abc, semantic, audio. Default audio.
    ///
    /// `semantic` stops before the VAE decode, which is the cheap way to check
    /// that a model really loaded without paying for a full render.
    #[serde(default)]
    pub stop_after: Option<String>,
    /// Upper bound on semantic codec tokens, default 9000.
    ///
    /// This is YuE2's length control. `apply_sampling` in `yue2/request.cpp`
    /// reads `{prefix}_max_tokens` from the request even though it is absent
    /// from `yue2_cli_interface()`, so it never appears in a schema-driven UI --
    /// and the AR stage genuinely stops here (`pipeline.cpp` sets
    /// `truncated` when the stream reaches it).
    ///
    /// Deliberately in tokens, not seconds: audio.cpp documents no conversion
    /// rate between semantic tokens and wall-clock time, and inventing one
    /// would make the number a lie. Calibrating it needs one timed render.
    #[serde(default)]
    pub semantic_max_tokens: Option<i64>,
    /// Lower bound. Setting both to the same value renders exactly that many.
    #[serde(default)]
    pub semantic_min_tokens: Option<i64>,
}

/// Cancelling kills the runtime, so the in-flight request always fails with a
/// connection error. Reporting that verbatim as `render failed: connection
/// reset` is both alarming and wrong — the user asked for it, and the UI shows
/// it in the error slot as though something broke.
///
/// audio.cpp exposes no cancel route (`/v1/tasks/` offers run, batch, stream
/// and the unload pair, and nothing else), so killing the process is the only
/// way to stop a render. The honest thing is to name what happened.
fn render_error(detail: &str, cancelled: bool) -> String {
    if cancelled {
        "cancelled — the runtime was stopped, so Start it again to render".to_string()
    } else {
        format!("render failed: {detail}")
    }
}

/// Run one render. Blocks for as long as the model takes.
pub fn render(model: &str, params: &YuE2Params) -> Result<Rendered, String> {
    if CANCELLED.load(Ordering::Relaxed) {
        return Err("cancelled".to_string());
    }
    let port = running_port().ok_or_else(|| "audio.cpp is not running".to_string())?;

    let mut options = serde_json::Map::new();
    options.insert("style".into(), params.style.clone().into());
    if let Some(seed) = params.seed {
        options.insert("seed".into(), serde_json::json!(seed));
    }
    if let Some(g) = params.guidance_scale {
        options.insert("guidance_scale".into(), serde_json::json!(g));
    }
    if let Some(steps) = params.num_inference_steps {
        options.insert("num_inference_steps".into(), serde_json::json!(steps));
    }
    if let Some(cot) = params.cot.as_deref().filter(|c| !c.trim().is_empty()) {
        options.insert("cot".into(), cot.into());
    }
    if let Some(stop) = params.stop_after.as_deref().filter(|s| !s.trim().is_empty()) {
        options.insert("stop_after".into(), stop.into());
    }
    if let Some(max) = params.semantic_max_tokens {
        options.insert("semantic_max_tokens".into(), serde_json::json!(max));
    }
    if let Some(min) = params.semantic_min_tokens {
        options.insert("semantic_min_tokens".into(), serde_json::json!(min));
    }

    let body = serde_json::json!({
        "model": model,
        "request": {
            "lyrics": params.lyrics,
            "options": serde_json::Value::Object(options),
        }
    });

    let started = std::time::Instant::now();
    let response = ureq::post(&format!("http://127.0.0.1:{port}/v1/tasks/run"))
        // No client timeout shorter than the server's busy timeout: a CPU
        // render legitimately runs for many minutes.
        .timeout(Duration::from_secs(BUSY_TIMEOUT_MS as u64 + 120))
        .send_json(body)
        .map_err(|e| render_error(&e.to_string(), CANCELLED.load(Ordering::Relaxed)))?;

    let payload: serde_json::Value = response
        .into_json()
        .map_err(|e| format!("bad result: {e}"))?;

    if let Some(err) = payload.get("error").and_then(|e| e.get("message")).and_then(|m| m.as_str()) {
        return Err(err.to_string());
    }

    let result = payload.get("result").ok_or_else(|| "no result in response".to_string())?;
    let audio_b64 = result
        .get("audio")
        .and_then(|a| a.as_str())
        .ok_or_else(|| "result carried no audio".to_string())?;

    // Accept either bare Base64 or a data: URI.
    let encoded = audio_b64.split_once(",").map(|(_, b)| b).unwrap_or(audio_b64);
    let bytes = base64_decode(encoded).ok_or_else(|| "audio was not valid Base64".to_string())?;

    let sample_rate = result
        .get("sample_rate")
        .and_then(|s| s.as_u64())
        .unwrap_or(48_000) as u32;
    let channels = result.get("channels").and_then(|c| c.as_u64()).unwrap_or(2) as u32;

    let library = crate::paths::library_dir();
    std::fs::create_dir_all(&library)
        .map_err(|e| format!("cannot create {}: {e}", library.display()))?;
    // Name the file after the style prompt. YuE2 output used to be
    // `yue2-<timestamp>.wav`, which identified nothing in a file manager.
    let stem = crate::paths::song_stem("", &params.style, &params.lyrics);
    let path = crate::paths::unique_path(&library, &stem, "wav");

    let mut file = std::fs::File::create(&path)
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    file.write_all(&bytes)
        .map_err(|e| format!("cannot write audio: {e}"))?;

    // seconds from the WAV header rather than trusting the server's number.
    let seconds = wav_seconds(&bytes, sample_rate, channels);

    Ok(Rendered {
        path: path.display().to_string(),
        bytes: bytes.len() as u64,
        seconds,
        elapsed_secs: started.elapsed().as_secs_f64(),
        sample_rate,
    })
}

/// Cooperative cancellation for the caller's own bookkeeping. audio.cpp itself
/// cannot be interrupted, so this only stops us reporting a result the user
/// already walked away from.
pub fn cancel() {
    CANCELLED.store(true, Ordering::Relaxed);
    stop();
}

pub fn reset_cancel() {
    CANCELLED.store(false, Ordering::Relaxed);
}

/// Read duration from a RIFF header rather than decoding it.
fn wav_seconds(bytes: &[u8], fallback_rate: u32, fallback_channels: u32) -> f32 {
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return 0.0;
    }
    let read_u32 = |at: usize| -> u32 {
        u32::from_le_bytes([
            bytes[at],
            bytes[at + 1],
            bytes[at + 2],
            bytes[at + 3],
        ])
    };
    let read_u16 = |at: usize| -> u32 {
        u16::from_le_bytes([bytes[at], bytes[at + 1]]) as u32
    };
    let mut offset = 12usize;
    while offset + 8 <= bytes.len() {
        let id = &bytes[offset..offset + 4];
        let size = read_u32(offset + 4) as usize;
        if id == b"fmt " && offset + 8 + 16 <= bytes.len() {
            // The fmt body starts at offset + 8. Within it:
            //   +0 audioFormat, +2 numChannels, +4 sampleRate,
            //   +8 byteRate, +12 blockAlign, +14 bitsPerSample.
            let body = offset + 8;
            // numChannels and bitsPerSample are u16; sampleRate is u32.
            // Reading numChannels as u32 swallows the rate's first bytes.
            let channels = read_u16(body + 2);
            let rate = read_u32(body + 4);
            let bits = read_u16(body + 14);
            if channels == 0 || rate == 0 || bits == 0 {
                offset = match offset.checked_add(advance_of(size)) {
                    Some(next) => next,
                    None => break,
                };
                continue;
            }
            let bytes_per_second = rate * channels * (bits / 8);
            if bytes_per_second > 0 {
                let data_size = wav_data_size(bytes).unwrap_or(bytes.len() - offset);
                return data_size as f32 / bytes_per_second as f32;
            }
        }
        // Chunks are word-aligned; guard against a bogus size walking the
        // cursor past the end of the buffer.
        let advance = size.saturating_add(size % 2).saturating_add(8);
        if advance == 0 {
            break;
        }
        match offset.checked_add(advance) {
            Some(next) => offset = next,
            None => break,
        }
    }
    let _ = (fallback_rate, fallback_channels);
    0.0
}

/// Next chunk boundary: 8 byte header plus the padded body.
fn advance_of(size: usize) -> usize {
    let a = size.saturating_add(size % 2).saturating_add(8);
    if a == 0 {
        8
    } else {
        a
    }
}

fn wav_data_size(bytes: &[u8]) -> Option<usize> {
    let mut offset = 12usize;
    while offset + 8 <= bytes.len() {
        let id = &bytes[offset..offset + 4];
        let size = u32::from_le_bytes([
            bytes[offset + 4],
            bytes[offset + 5],
            bytes[offset + 6],
            bytes[offset + 7],
        ]) as usize;
        if id == b"data" {
            // The declared size can exceed the bytes actually present in a
            // truncated file; clamp or the duration comes out absurdly short.
            return Some(size.min(bytes.len().saturating_sub(offset + 8)));
        }
        // A bogus chunk size must not walk the cursor out of the file.
        let advance = size.saturating_add(size % 2).saturating_add(8);
        if advance == 0 {
            return None;
        }
        offset = match offset.checked_add(advance) {
            Some(next) => next,
            None => return None,
        };
    }
    None
}

fn base64_decode(input: &str) -> Option<Vec<u8>> {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut lookup = [255u8; 256];
    for (i, c) in TABLE.iter().enumerate() {
        lookup[*c as usize] = i as u8;
    }
    let mut out = Vec::with_capacity(input.len() / 4 * 3);
    let mut buffer: u32 = 0;
    let mut bits: u32 = 0;
    for byte in input.bytes() {
        if byte == b'=' || byte.is_ascii_whitespace() {
            continue;
        }
        let value = lookup[byte as usize];
        if value == 255 {
            return None;
        }
        buffer = (buffer << 6) | value as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    Some(out)
}

/// Quantisation label from a GGUF filename, e.g. `q8_0`.
fn quant_of(name: &str) -> Option<String> {
    let stem = name.strip_suffix(".gguf")?;
    let tail = stem.rsplit('-').next()?;
    let q = tail.to_ascii_lowercase();
    if q.starts_with("q") && q.len() >= 4 && q[1..].chars().all(|c| c.is_ascii_digit() || c == '_' || c == 'k' || c == 'm') {
        Some(tail.to_string())
    } else if q == "bf16" || q == "f16" || q == "f32" {
        Some(tail.to_string())
    } else {
        None
    }
}

fn link(from: &Path, to: &Path) -> std::io::Result<()> {
    let _ = std::fs::remove_file(to);
    std::os::unix::fs::symlink(from, to)
}

/// Component session-option keys per family, as audio.cpp declares them.
///
/// Keys are `<family>.<name>`, and audio.cpp throws when the named file is
/// absent from the model root rather than searching for a substitute. Anything
/// not listed here falls back to audio.cpp's own defaults, which is correct
/// only when the default filename happens to match.
fn component_options(
    family: &str,
    model_gguf: Option<&str>,
    vae_gguf: Option<&str>,
) -> Option<std::collections::BTreeMap<String, String>> {
    let mut map = std::collections::BTreeMap::new();
    match family {
        "yue2" => {
            if let Some(m) = model_gguf {
                map.insert("yue2.model_gguf".to_string(), m.to_string());
            }
            if let Some(v) = vae_gguf {
                map.insert("yue2.vae_gguf".to_string(), v.to_string());
            }
        }
        _ => return None,
    }
    if map.is_empty() {
        None
    } else {
        Some(map)
    }
}

/// True when every wanted symlink already exists and points where it should.
///
/// Compared by resolved link target rather than mtime: the whole reason to
/// check is to avoid destroying a directory a running server is using, and the
/// sources are immutable GGUF/JSON files under a directory we never write to.
fn view_is_current(view: &Path, wanted: &[(PathBuf, PathBuf)]) -> bool {
    if !view.is_dir() {
        return false;
    }
    wanted.iter().all(|(link_path, target)| {
        std::fs::read_link(link_path)
            .map(|existing| {
                // Compare canonically: the link may hold a relative path.
                match (existing.canonicalize(), target.canonicalize()) {
                    (Ok(a), Ok(b)) => a == b,
                    _ => existing == *target,
                }
            })
            .unwrap_or(false)
    })
}

/// One selectable model per quantisation of `family`.
///
/// audio.cpp resolves its files by name inside a single directory, so two
/// quants sitting side by side are ambiguous. Each quant gets a small view
/// directory of symlinks holding only that quant, which makes the choice
/// explicit and costs no disk.
pub fn quant_views(dir: &Path, family: &str) -> Vec<ModelEntry> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let files: Vec<PathBuf> = entries.filter_map(|e| e.ok()).map(|e| e.path()).collect();

    // The VAE is a separate component; prefer f16 for size, fall back to f32.
    let vae = ["f16", "f32"]
        .iter()
        .find_map(|q| {
            files.iter().find(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.contains(&format!("vae-{q}")) && n.ends_with(".gguf"))
            })
        })
        .cloned();

    let sidecars_dir = dir.join("sidecars");
    let sidecars: Vec<PathBuf> = if sidecars_dir.is_dir() {
        std::fs::read_dir(&sidecars_dir)
            .map(|s| s.filter_map(|e| e.ok()).map(|e| e.path()).collect())
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    let mut out = Vec::new();
    let root = crate::paths::app_data_dir().join("audiocpp").join("views");

    // Language model files, keyed by quant. Anything named "vae" is the other
    // component and must not become a selectable model.
    let mut lms: Vec<(String, PathBuf)> = files
        .iter()
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(".gguf") && !n.contains("vae"))
        })
        .filter_map(|p| {
            let name = p.file_name().and_then(|n| n.to_str())?;
            let q = quant_of(name)?;
            Some((q, p.clone()))
        })
        .collect();
    lms.sort_by(|a, b| a.0.cmp(&b.0));
    lms.dedup_by(|a, b| a.0 == b.0);

    for (quant, lm) in lms {
        let view = root.join(format!("{family}-{quant}"));
        let lm_name = lm.file_name().and_then(|n| n.to_str()).unwrap_or_default();

        // What this view should contain: (path inside the view, target).
        let mut wanted: Vec<(PathBuf, PathBuf)> = vec![(view.join(lm_name), lm.clone())];
        if let Some(v) = &vae {
            if let Some(n) = v.file_name().and_then(|n| n.to_str()) {
                wanted.push((view.join(n), v.clone()));
            }
        }
        // audio.cpp looks for sidecars under `<model_root>/sidecars`, not in the
        // root itself: `load_yue2_assets` builds `model_root / "sidecars" /
        // "yue2-model-config.json"` and throws if it is missing. Linking them
        // flat produces a view that registers fine and then dies on first task.
        for s in &sidecars {
            if let Some(n) = s.file_name().and_then(|n| n.to_str()) {
                wanted.push((view.join("sidecars").join(n), s.clone()));
            }
        }

        // Rebuild only when the view is not already exactly right. The view
        // directory is shared app state that a running server has open, and the
        // panel re-scans on mount and on every directory change -- tearing it
        // down unconditionally would pull it out from under a live model.
        if !view_is_current(&view, &wanted) {
            let _ = std::fs::remove_dir_all(&view);
            if std::fs::create_dir_all(view.join("sidecars")).is_err() {
                continue;
            }
            for (link_path, target) in &wanted {
                let _ = link(target, link_path);
            }
        }

        out.push(ModelEntry {
            id: format!("{family}-{quant}"),
            family: family.to_string(),
            task: "gen".to_string(),
            mode: "offline".to_string(),
            // Point at the LM file, not the view directory. audio.cpp refuses a
            // directory holding more than one GGUF ("ambiguous_directory_gguf")
            // unless it can be narrowed down, and YuE2 always needs two files:
            // the AR/NAR model and the VAE. Naming the model explicitly is the
            // documented escape, and model_root then resolves to this view, so
            // `yue2.vae_gguf` still finds its file beside it.
            // The symlink inside the view, not the original file: the whole point is
            // that the model root stays inside app data and the shared model
            // directory is only ever read.
            path: view.join(lm_name).display().to_string(),
            session_options: component_options(
                family,
                Some(lm_name),
                vae.as_ref()
                    .and_then(|v| v.file_name())
                    .and_then(|n| n.to_str()),
            ),
        });
    }
    out
}

/// Identify a model directory.
///
/// Family is detected from the sidecar config files, not the directory name:
/// LM Studio names snapshots by commit hash (`9711e46e…`), so a name-based
/// guess fails on exactly the layouts these models ship in.
pub fn probe_dir(path: &Path) -> Vec<ModelEntry> {
    let has_gguf = std::fs::read_dir(path)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .any(|e| e.path().extension().and_then(|x| x.to_str()) == Some("gguf"))
        })
        .unwrap_or(false);
    if !has_gguf {
        return Vec::new();
    }

    let has = |name: &str| path.join(name).is_file();

    // Sidecar names are authoritative: they are what audio.cpp requires.
    // The path is only a fallback, because model managers nest snapshots under
    // a directory named after the repo.
    let mut haystack = String::new();
    let mut cursor = Some(path);
    for _ in 0..4 {
        let Some(dir) = cursor else { break };
        if let Some(name) = dir.file_name().and_then(|n| n.to_str()) {
            haystack.push(' ');
            haystack.push_str(&name.to_lowercase());
        }
        cursor = dir.parent();
    }

    const FAMILIES: &[(&str, &str)] = &[
        ("yue2", "yue2"),
        ("heartmula", "heartmula"),
        ("stable_audio", "stable"),
        ("midashenglm_gen", "midashenglm"),
    ];

    let mut out = Vec::new();
    for (family, needle) in FAMILIES {
        let detected = has(&format!("{family}-model-config.json"))
            || has(&format!("{family}-config.json"))
            || haystack.contains(needle);
        if detected {
            // One entry per quantisation so each is separately selectable.
            let views = quant_views(path, family);
            if views.is_empty() {
                out.push(ModelEntry {
                    id: family.to_string(),
                    family: family.to_string(),
                    task: "gen".to_string(),
                    mode: "offline".to_string(),
                    path: path.display().to_string(),
                    session_options: None,
                });
            } else {
                out.extend(views);
            }
        }
    }
    out
}

/// One selectable model, plus what the picker needs to describe it.
///
/// Kept separate from `ModelEntry` because that struct is written verbatim into
/// the server config, where an unexpected key is more likely to be rejected
/// than ignored.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelOption {
    pub entry: ModelEntry,
    /// "YuE2 3B"
    pub label: String,
    /// "Q8_0"
    pub quant: String,
    /// "Text-to-music"
    pub task_label: String,
    /// Bytes the view will actually pull in, following the symlinks.
    pub size_bytes: u64,
    /// The decoder half. YuE2 will not render without it.
    pub has_vae: bool,
    /// The config JSONs and tokenizer audio.cpp resolves by name.
    pub has_sidecars: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanResult {
    pub directory: String,
    pub options: Vec<ModelOption>,
    /// Things that will bite later, said up front instead of mid-render.
    pub warnings: Vec<String>,
}

fn task_label(task: &str) -> String {
    match task {
        "gen" => "Text-to-music".to_string(),
        "tts" => "Text-to-speech".to_string(),
        "asr" => "Transcribe".to_string(),
        "vad" => "Voice activity".to_string(),
        other => other.to_string(),
    }
}

/// "yue2-3b" -> "YuE2 3B". Enough structure for a picker row without
/// hand-maintaining a table that goes stale when a new family lands upstream.
fn pretty_family(stem: &str) -> String {
    let mut words: Vec<String> = Vec::new();
    for word in stem.split(['-', '_']) {
        if word.is_empty() {
            continue;
        }
        let pretty = if is_param_count(word) {
            // A parameter count is a unit, not a word: 3b is "3B", not "3b".
            word.to_uppercase()
        } else {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        };
        words.push(pretty);
    }
    words.join(" ")
}

/// "3b", "7b", "1.5b" -- digits, optionally a decimal point, then a size unit.
fn is_param_count(word: &str) -> bool {
    let lower = word.to_ascii_lowercase();
    let Some(digits) = lower.strip_suffix('b') else {
        return false;
    };
    !digits.is_empty()
        && digits
            .chars()
            .all(|c| c.is_ascii_digit() || c == '.')
}

/// Bytes a view directory will really consume.
///
/// `DirEntry::metadata` does not traverse symlinks on Unix -- it reports the
/// link itself, so summing it over a view of symlinks yields the length of the
/// target *paths* (a few hundred bytes for a 4 GiB model). `fs::metadata` does
/// follow them, which is what the number the user reads has to be based on.
fn view_size(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .map(|e| std::fs::metadata(e.path()).map(|m| m.len()))
                .filter_map(|m| m.ok())
                .sum()
        })
        .unwrap_or(0)
}

/// The view's language-model file, i.e. the .gguf that is not the VAE.
fn view_model_stem(dir: &Path) -> Option<String> {
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            if name.ends_with(".gguf") && !name.contains("vae") {
                Some(name)
            } else {
                None
            }
        })
        .next()
        .map(|n| n.trim_end_matches(".gguf").to_string())
}

fn view_has(dir: &Path, needle: &str) -> bool {
    let hit = |p: &Path| {
        std::fs::read_dir(p)
            .map(|entries| {
                entries.filter_map(|e| e.ok()).any(|e| {
                    e.file_name()
                        .to_str()
                        .is_some_and(|n| n.contains(needle))
                })
            })
            .unwrap_or(false)
    };
    hit(dir) || hit(&dir.join("sidecars"))
}

/// JSON configs anywhere in the view. audio.cpp reads these from
/// `<model_root>/sidecars`, so that is where they are expected to be.
fn view_has_sidecars(dir: &Path) -> bool {
    let found = std::fs::read_dir(dir.join("sidecars"))
        .map(|entries| {
            entries.filter_map(|e| e.ok()).any(|e| {
                e.path()
                    .extension()
                    .and_then(|s| s.to_str())
                    .is_some_and(|s| s == "json")
            })
        })
        .unwrap_or(false);
    found || view_has(dir, ".json")
}

/// Work out what a directory offers, without starting anything.
///
/// This is what makes "assign a folder, get a working configuration" possible:
/// the family is detected, every quantisation is turned into its own view, and
/// each option reports the components it will need at render time so a missing
/// VAE shows up here rather than twenty minutes into a CPU render.
pub fn scan(path: &Path) -> ScanResult {
    let options: Vec<ModelOption> = probe_dir(path)
        .into_iter()
        .map(|entry| {
            // `path` is the model GGUF; its parent is the view directory that
            // holds the VAE and sidecars.
            let model_path = PathBuf::from(&entry.path);
            let dir = model_path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| model_path.clone());
            let quant = entry
                .id
                .rsplit_once('-')
                .map(|(_, q)| q.to_string())
                .unwrap_or_default();
            // Name the picker row after the weights themselves --
            // `yue2-3b-q8_0.gguf` is "Yue2 3B", not a bare "Yue2".
            let label = view_model_stem(&dir)
                .map(|stem| {
                    let suffix = format!("-{quant}");
                    let bare = if !quant.is_empty() && stem.ends_with(&suffix) {
                        stem[..stem.len() - suffix.len()].to_string()
                    } else {
                        stem
                    };
                    pretty_family(&bare)
                })
                .unwrap_or_else(|| pretty_family(&entry.family));
            let quant = if quant.is_empty() {
                String::new()
            } else {
                quant
            };
            ModelOption {
                label,
                task_label: task_label(&entry.task),
                size_bytes: view_size(&dir),
                has_vae: view_has(&dir, "vae"),
                has_sidecars: view_has_sidecars(&dir),
                quant,
                entry,
            }
        })
        .collect();

    let mut warnings = Vec::new();
    if options.is_empty() {
        warnings.push(format!(
            "No audio.cpp model found in {}. Expected a .gguf plus its sidecar configs.",
            path.display()
        ));
    }
    for o in &options {
        if !o.has_vae {
            warnings.push(format!(
                "{} has no VAE beside it. It will load but cannot decode audio.",
                o.entry.id
            ));
        }
        if !o.has_sidecars {
            warnings.push(format!(
                "{} has no sidecar config JSONs. audio.cpp resolves those by name and will refuse it.",
                o.entry.id
            ));
        }
    }

    ScanResult {
        directory: path.display().to_string(),
        options,
        warnings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Rendered` is camelCase on the wire. The panel used to read
    /// `elapsed_secs`/`sample_rate`, which serde never sends, so every render
    /// would have reported "at undefined Hz" and "NaNs to render".
    #[test]
    fn rendered_uses_camel_case_wire_names() {
        let r = Rendered {
            path: "/library/warm-lo-fi.wav".to_string(),
            bytes: 5_760_000,
            seconds: 30.0,
            elapsed_secs: 412.5,
            sample_rate: 48_000,
        };
        let v = serde_json::to_value(&r).expect("serialize");
        assert_eq!(v["sampleRate"], 48_000);
        assert_eq!(v["elapsedSecs"], 412.5);
        assert!(v.get("sample_rate").is_none(), "snake_case leaked: {v}");
        assert!(v.get("elapsed_secs").is_none(), "snake_case leaked: {v}");
    }

    fn entry(id: &str) -> ModelEntry {
        ModelEntry {
            id: id.to_string(),
            family: "yue2".to_string(),
            task: "gen".to_string(),
            mode: "offline".to_string(),
            path: format!("/tmp/{id}"),
            session_options: None,
        }
    }

    fn config_with(threads: u32, backend: &str) -> serde_json::Value {
        let dir = std::env::temp_dir().join(format!(
            "aceforge-ac-cfg-{threads}-{backend}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let knobs = crate::settings::Settings {
            audiocpp_threads: threads,
            audiocpp_backend: backend.to_string(),
            ..Default::default()
        };
        let path = write_config_at(&dir, 8188, &[entry("yue2-q8_0")], &knobs).expect("write config");
        let text = std::fs::read_to_string(&path).expect("read config");
        let _ = std::fs::remove_dir_all(&dir);
        serde_json::from_str(&text).expect("parse config")
    }

    /// 0 means "let audio.cpp decide", which it signals by the key being absent.
    /// It used to be written as 1: the UI showed 0, the config said 1, and a
    /// 4-core render silently used a single thread.
    #[test]
    fn zero_threads_omits_the_key_rather_than_writing_one() {
        let cfg = config_with(0, "cpu");
        let obj = cfg.as_object().expect("object");
        assert!(
            !obj.contains_key("threads"),
            "threads key present: {:?}",
            obj.get("threads")
        );
    }

    #[test]
    fn an_explicit_thread_count_is_written_through() {
        let cfg = config_with(4, "cpu");
        assert_eq!(cfg.get("threads").and_then(|v| v.as_u64()), Some(4));
    }

    #[test]
    fn backend_and_device_always_reach_the_config() {
        let cfg = config_with(0, "cuda");
        assert_eq!(cfg.get("backend").and_then(|v| v.as_str()), Some("cuda"));
        assert!(cfg.get("device").is_some());
    }

    /// `task` is the operation. The server rejects the domain word "music".
    #[test]
    fn model_entries_carry_gen_not_music() {
        let cfg = config_with(0, "cpu");
        let models = cfg.get("models").and_then(|m| m.as_array()).expect("models");
        assert_eq!(models[0].get("task").and_then(|v| v.as_str()), Some("gen"));
    }

/// audio.cpp resolves components by hardcoded filename and throws when they
    /// are missing. Selecting a non-default quant used to die at load with
    /// "yue2.model_gguf file does not exist".
    #[test]
    fn pins_the_component_filenames_for_each_quant() {
        let opts = component_options(
            "yue2",
            Some("yue2-3b-q4_0.gguf"),
            Some("yue2-vae-f16.gguf"),
        )
        .expect("options for yue2");
        assert_eq!(opts.get("yue2.model_gguf").map(String::as_str), Some("yue2-3b-q4_0.gguf"));
        assert_eq!(opts.get("yue2.vae_gguf").map(String::as_str), Some("yue2-vae-f16.gguf"));
    }

    #[test]
    fn unknown_families_fall_back_to_server_defaults() {
        assert!(component_options("brand_new_family", Some("m.gguf"), Some("v.gguf")).is_none());
    }

    /// Scan a real model directory.
///
/// Skipped unless `ACEFORGE_AUDIOCPP_TEST_DIR` points at one, because the
/// suite must not depend on multi-gigabyte weights being present. Run it with
/// that variable set to check detection, view creation and component pinning
/// against the real layout:
///
/// ```sh
/// ACEFORGE_AUDIOCPP_TEST_DIR="/path/to/snapshot" cargo test --release --lib real_scan
/// ```
#[test]
fn real_scan_reports_runnable_models() {
    let Ok(dir) = std::env::var("ACEFORGE_AUDIOCPP_TEST_DIR") else {
        eprintln!("skipped: set ACEFORGE_AUDIOCPP_TEST_DIR to exercise this");
        return;
    };
    let result = scan(std::path::Path::new(&dir));
    assert!(
        !result.options.is_empty(),
        "no models detected in {dir}: {:?}",
        result.warnings
    );

    for o in &result.options {
        println!(
            "{} -> label={} task={} quant={} size={} vae={} sidecars={}",
            o.entry.id,
            o.label,
            o.task_label,
            o.quant,
            o.size_bytes,
            o.has_vae,
            o.has_sidecars
        );
        assert!(o.has_vae, "{} has no VAE", o.entry.id);
        assert!(o.has_sidecars, "{} has no sidecars", o.entry.id);
        assert!(o.size_bytes > 0, "{} resolved to zero bytes", o.entry.id);

        // The whole point: audio.cpp throws when a component filename does not
        // match, so a per-quant view must name its own files.
        let opts = o
            .entry
            .session_options
            .as_ref()
            .unwrap_or_else(|| panic!("{} pinned no component filenames", o.entry.id));
        // `path` is the model GGUF; components resolve against its directory.
        let model_path = std::path::PathBuf::from(&o.entry.path);
        let view = model_path
            .parent()
            .unwrap_or(&model_path)
            .to_path_buf();
        for (key, file) in opts {
            assert!(
                view.join(file).is_file(),
                "{key} names {file}, which is not in {}",
                view.display()
            );
        }
    }
}

#[test]
    fn builds_readable_picker_labels() {
        assert_eq!(pretty_family("yue2-3b"), "Yue2 3B");
        assert_eq!(pretty_family("stable_audio"), "Stable Audio");
        assert_eq!(pretty_family("heartmula-7b"), "Heartmula 7B");
        assert_eq!(pretty_family("1.5b-model"), "1.5B Model");
    }

    /// "3b" is a size, "b" is not: the check must not eat single letters.
    #[test]
    fn only_treats_digit_prefixes_as_parameter_counts() {
        assert!(is_param_count("3b"));
        assert!(is_param_count("1.5b"));
        assert!(!is_param_count("b"));
        assert!(!is_param_count("bb"));
        assert!(!is_param_count("beta"));
    }

#[test]
    fn emits_session_options_into_the_server_config() {
        let dir = std::env::temp_dir().join(format!("aceforge-ac-so-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut m = entry("yue2-q8_0");
        m.path = "/views/yue2-q8_0/yue2-3b-q8_0.gguf".to_string();
        m.session_options = component_options(
            "yue2",
            Some("yue2-3b-q8_0.gguf"),
            Some("yue2-vae-f16.gguf"),
        );
        let knobs = crate::settings::Settings::default();
        let path = write_config_at(&dir, 8188, &[m], &knobs).expect("write config");
        let text = std::fs::read_to_string(&path).expect("read config");
        let _ = std::fs::remove_dir_all(&dir);

        // audio.cpp reads these off the model entry and feeds them to
        // find_option(); without them a non-default quant dies at load.
        let cfg: serde_json::Value = serde_json::from_str(&text).expect("parse");
        let first = &cfg["models"][0];
        assert_eq!(first["path"], "/views/yue2-q8_0/yue2-3b-q8_0.gguf");
        assert_eq!(first["session_options"]["yue2.model_gguf"], "yue2-3b-q8_0.gguf");
        assert_eq!(first["session_options"]["yue2.vae_gguf"], "yue2-vae-f16.gguf");
    }

    /// Entries with nothing pinned must not carry an empty object: audio.cpp
    /// treats a present-but-empty block differently from an absent one.
    #[test]
    fn omits_session_options_when_there_is_nothing_to_pin() {
        let dir = std::env::temp_dir().join(format!("aceforge-ac-noso-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let knobs = crate::settings::Settings::default();
        let path = write_config_at(&dir, 8188, &[entry("yue2")], &knobs).expect("write");
        let text = std::fs::read_to_string(&path).expect("read");
        let _ = std::fs::remove_dir_all(&dir);
        let cfg: serde_json::Value = serde_json::from_str(&text).expect("parse");
        assert!(cfg["models"][0].get("session_options").is_none());
    }

/// Re-scanning must not tear down a view a running server has open. The
    /// panel re-scans on mount and on every directory change, so an
    /// unconditional rebuild would delete the model root mid-session.
    #[test]
    fn rebuilding_is_skipped_when_the_view_already_matches() {
        let dir = std::env::temp_dir().join(format!("aceforge-view-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        let real = dir.join("model.gguf");
        std::fs::write(&real, b"weights").expect("write");

        let view = dir.join("view");
        std::fs::create_dir_all(view.join("sidecars")).expect("view");
        let wanted = vec![
            (view.join("model.gguf"), real.clone()),
            (view.join("sidecars/cfg.json"), dir.join("cfg.json")),
        ];
        // Not built yet, so not current.
        assert!(!view_is_current(&view, &wanted));

        for (link_path, target) in &wanted {
            std::fs::write(target, b"x").expect("write");
            let _ = link(target, link_path);
        }
        assert!(view_is_current(&view, &wanted));

        // Repointing one link must be detected.
        let other = dir.join("other.gguf");
        std::fs::write(&other, b"y").expect("write");
        let _ = link(&other, &view.join("model.gguf"));
        assert!(!view_is_current(&view, &wanted));

        let _ = std::fs::remove_dir_all(&dir);
    }

/// `tags` and `duration` were both being sent to YuE2 and neither exists in
    /// `yue2_cli_interface()`. audio.cpp forwards the options map untouched, so
    /// a wrong key is not an error -- it is a silently ignored setting. YuE2
    /// has no length control at all, which is why there is no seconds slider.
    #[test]
    fn yue2_rejects_options_it_does_not_declare() {
        let declared = [
            "style",
            "lyrics",
            "abc",
            "abc_file",
            "cot",
            "export_semantic",
            "stop_after",
            "seed",
            "guidance_scale",
            "num_inference_steps",
        ];
        // `duration_sec` genuinely is not a YuE2 option, unlike the sampling
        // overrides which `apply_sampling` reads even though the declared CLI
        // interface omits them.
        for bogus in ["tags", "duration", "duration_sec", "bpm", "keyscale"] {
            assert!(
                !declared.contains(&bogus),
                "{bogus} is not a YuE2 request option"
            );
        }
    }

    #[test]
    fn a_cancel_is_reported_as_a_cancel_not_a_network_error() {
        // Pressing cancel kills the server, so the request always dies with a
        // connection error. Showing that raw would read as a fault.
        let msg = render_error("connection reset by peer", true);
        assert!(msg.starts_with("cancelled"), "got: {msg}");
        assert!(!msg.contains("connection reset"), "leaked the raw error: {msg}");
        assert!(msg.contains("Start"), "should say how to recover: {msg}");
    }

    #[test]
    fn a_real_failure_still_reports_the_detail() {
        let msg = render_error("model yue2-q8_0 not found", false);
        assert!(msg.starts_with("render failed:"), "got: {msg}");
        assert!(msg.contains("not found"), "detail was lost: {msg}");
    }

    /// `semantic_max_tokens` is read by `apply_sampling` in `yue2/request.cpp`,
    /// not by the declared CLI interface. It is the real length control, and
    /// missing it is why there was no way to bound a render.
    #[test]
    fn length_knobs_are_sent_under_their_real_names() {
        let p = YuE2Params {
            style: "folk pop".to_string(),
            semantic_max_tokens: Some(2400),
            semantic_min_tokens: Some(600),
            ..Default::default()
        };
        let mut options = serde_json::Map::new();
        options.insert("semantic_max_tokens".into(), serde_json::json!(2400));
        options.insert("semantic_min_tokens".into(), serde_json::json!(600));
        assert_eq!(options["semantic_max_tokens"], 2400);
        assert_eq!(options["semantic_min_tokens"], 600);
        assert_eq!(p.semantic_max_tokens, Some(2400));
    }

    #[test]
    fn decodes_base64() {
        assert_eq!(base64_decode("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(base64_decode("aGk=").unwrap(), b"hi");
        assert!(base64_decode("!!!").is_none());
    }

    #[test]
    fn rejects_non_base64() {
        assert!(base64_decode("not base64!!").is_none());
    }

    #[test]
    fn reads_wav_duration() {
        // 44 byte header, 16 bit mono at 8 kHz, 1600 bytes of samples = 0.1s.
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36u32 + 1600).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes()); // mono
        wav.extend_from_slice(&8000u32.to_le_bytes());
        wav.extend_from_slice(&16000u32.to_le_bytes()); // byte rate
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&1600u32.to_le_bytes());
        wav.extend_from_slice(&[0u8; 1600]);
        let seconds = wav_seconds(&wav, 48000, 2);
        assert!((seconds - 0.1).abs() < 0.001, "got {seconds}");
    }

    #[test]
    fn wav_duration_of_garbage_is_zero() {
        assert_eq!(wav_seconds(b"not a wav file at all", 48000, 2), 0.0);
    }
}