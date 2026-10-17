//! Drives ace-server's async job API and lands finished audio on disk.
//!
//! The server never blocks a request: POST /lm and POST /synth both return a
//! job id immediately, and the client polls GET /job until the status turns
//! DONE, then fetches the body with ?result=1.
//!
//! Audio is written to a file and the path is returned, rather than pushed
//! across the IPC bridge as base64. A three minute track is ~3 MB of JSON
//! array, which the bridge would have to serialise in both directions.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Fixed in tools/ace-server.cpp; the client splits on the same string.
const BOUNDARY: &str = "ace-batch-boundary";

const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(400);
/// Hard ceiling on one stage. Generous for 4 GB hardware: a repaint of a
/// five-minute source has been measured at a few minutes, and the failure this
/// replaces was an indefinite hang rather than a slow render.
const JOB_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30 * 60);

/// A stage that stops writing to stderr for this long has stopped making
/// progress. Measured against the log's byte length, not against polls: the
/// sidecar logs steadily through model loads, sampling and decode, so silence
/// is the signal. Chosen well above the longest observed gap between log lines
/// on this machine so a slow render cannot trip it.
const STALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(180);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateParams {
    pub caption: String,
    #[serde(default)]
    pub lyrics: String,
    /// Empty means "let the planner choose".
    #[serde(default)]
    pub duration: f32,
    #[serde(default)]
    pub bpm: i32,
    #[serde(default)]
    pub keyscale: String,
    #[serde(default)]
    pub timesignature: String,
    #[serde(default)]
    pub vocal_language: String,
    /// 0 resolves from the loaded model (turbo 8, sft/base 50).
    #[serde(default)]
    pub inference_steps: i32,
    /// 0 resolves to 1.0, i.e. CFG off.
    #[serde(default)]
    pub guidance_scale: f32,
    #[serde(default)]
    pub shift: f32,
    /// -1 means random.
    #[serde(default = "minus_one")]
    pub seed: i64,
    #[serde(default)]
    pub output_format: String,
    /// Number of DiT variations per request.
    #[serde(default = "one")]
    pub synth_batch_size: i32,
    /// GGUF filename (without `.gguf`) of the planner LM. Empty means the
    /// first one the backend finds, which is not always the one you want.
    #[serde(default)]
    pub lm_model: String,
    /// GGUF filename (without `.gguf`) of the DiT.
    #[serde(default)]
    pub synth_model: String,
    /// Diffusion solver: euler, sde, dpm3m, stork4.
    #[serde(default = "default_solver")]
    pub solver: String,
    /// text2music, cover, repaint, lego, extract, complete.
    #[serde(default = "default_task_type")]
    pub task_type: String,
    /// Seconds into the source to repaint. Negative outpaints before it.
    #[serde(default)]
    pub repainting_start: f32,
    /// Seconds into the source to stop repainting. -1 means "to the end".
    #[serde(default = "minus_one_f32")]
    pub repainting_end: f32,
    /// 0-1: fraction of DiT steps that hear the source. Only meaningful for
    /// cover and repaint.
    #[serde(default = "one_f32")]
    pub audio_cover_strength: f32,
    /// Absolute path to audio to repaint or cover. Empty means text-to-music.
    ///
    /// Validated against the library directory before it is read, exactly like
    /// `delete_track`: this is a path that arrives from the frontend and gets
    /// uploaded to the backend.
    #[serde(default)]
    pub source_audio: String,
    /// Library track whose timbre to copy. Empty means no reference.
    ///
    /// This is the only real voice control ACE-Step has: `AceRequest` has no
    /// gender or speaker field, and a caption adjective is only a suggestion.
    /// The reference is VAE-encoded into timbre features and conditions the
    /// DiT directly, so "sound like this" beats describing it in words.
    ///
    /// Not part of the request JSON -- the server takes it as a multipart file
    /// part named `ref_audio`, alongside `audio` for the source.
    #[serde(default)]
    pub ref_audio: String,
    /// User-supplied name for the result. Empty falls back to the caption.
    #[serde(default)]
    pub title: String,
    /// Vocal gender hint appended to the caption before the planner sees it.
    ///
    /// There is no gender field in `AceRequest` and no model support for one.
    /// This is the same trick the reference UI uses: append the literal words
    /// "Male vocals" or "Female vocals" to the caption, which the planner then
    /// reflects in the CoT caption it writes. It is a hint, not a switch.
    #[serde(default)]
    pub vocal_gender: String,
    // ---- Planner (LM) sampling. Previously hardcoded here, which meant the
    // reference UI's equivalents did nothing and these were not adjustable. ----
    #[serde(default = "default_lm_temperature")]
    pub lm_temperature: f32,
    #[serde(default = "default_lm_cfg_scale")]
    pub lm_cfg_scale: f32,
    #[serde(default = "default_lm_top_p")]
    pub lm_top_p: f32,
    /// 0 disables hard top-k. `top_p` still applies.
    #[serde(default)]
    pub lm_top_k: i32,
    #[serde(default)]
    pub lm_negative_prompt: String,
    /// Let the planner rewrite the caption as chain-of-thought before the DiT
    /// sees it. Turning this off passes your caption through verbatim.
    #[serde(default = "yes")]
    pub use_cot_caption: bool,
    /// Comma-separated audio codes to condition on, bypassing the planner.
    ///
    /// Non-empty codes put the DiT in cover mode. Only useful for iterating on
    /// a specific arrangement, which is what the reference UI's passthrough
    /// box is for.
    #[serde(default)]
    pub audio_codes: String,
    /// LoRA adapter name, as registered by the server's adapter scan.
    #[serde(default)]
    pub adapter: String,
    /// 0-1ish. Strength of the adapter's contribution.
    #[serde(default = "one_f32")]
    pub adapter_scale: f32,
    /// Which stem to generate, for the stem-oriented task types.
    ///
    /// Becomes the DiT instruction verbatim: "Generate the {track} track based
    /// on the audio context:". Empty yields a generic instruction, which is
    /// why a bare lego run sounds like "a stem" rather than the drums.
    /// Ignored by text2music, cover and repaint.
    #[serde(default)]
    pub track: String,

    // ---- HOT-Step CPP only --------------------------------------------
    // The bundled acestep.cpp build ignores request fields it does not know,
    // so these are always safe to send. They reach the sampler only when
    // HOT-Step is the running engine, which is what makes them worth carrying
    // on one struct rather than maintaining a second parameter set.

    /// Encoded as the unconditional branch when set, which is how the planner is
    /// steered away from a sound rather than merely given a caption.
    #[serde(default)]
    pub negative_prompt: String,
    /// Which VAE decodes the latents: empty keeps the loaded one. HOT-Step also
    /// ships a fine-tuned ScragVAE and a PP-VAE cleanup pass.
    #[serde(default)]
    pub vae: String,
    /// Run a second autoencoder pass over the audio to take the fizz off.
    #[serde(default)]
    pub pp_vae_reencode: bool,
    /// One of HOT-Step's shipped Lua postprocess plugins, by name.
    #[serde(default)]
    pub postprocess_plugin: String,
    /// Fraction of the sequence CFG applies to. 1.0 is all of it.
    #[serde(default = "one_f32")]
    pub lm_cfg_cutoff_ratio: f32,
    /// A comma separated timestep schedule, replacing the solver's own.
    #[serde(default)]
    pub custom_timesteps: String,
    #[serde(default = "default_mp3_bitrate")]
    pub mp3_bitrate: i32,
    /// -1 keeps the model's own window length.
    #[serde(default = "minus_one_i32")]
    pub dit_sliding_window: i32,
    /// A second adapter slot for the planner LM, separate from the DiT one.
    /// HOT-Step keeps the two apart because they are applied at different
    /// depths of a different model.
    #[serde(default)]
    pub lm_adapter: String,
    #[serde(default = "one_f32")]
    pub lm_adapter_scale: f32,
}

fn default_lm_temperature() -> f32 {
    0.85
}
fn default_lm_cfg_scale() -> f32 {
    2.0
}
fn default_lm_top_p() -> f32 {
    0.9
}
fn yes() -> bool {
    true
}

fn default_solver() -> String {
    "euler".to_string()
}
fn default_mp3_bitrate() -> i32 {
    128
}
fn default_task_type() -> String {
    "text2music".to_string()
}
fn minus_one_f32() -> f32 {
    -1.0
}
fn minus_one_i32() -> i32 {
    -1
}
fn one_f32() -> f32 {
    1.0
}

/// Append a hint line without mangling an empty base.
fn append_line(base: &str, hint: &str) -> String {
    let trimmed = base.trim();
    if trimmed.is_empty() {
        hint.to_string()
    } else {
        format!("{trimmed}\n{hint}")
    }
}

fn minus_one() -> i64 {
    -1
}
fn one() -> i32 {
    1
}

impl Default for GenerateParams {
    fn default() -> Self {
        Self {
            caption: String::new(),
            lyrics: String::new(),
            duration: 0.0,
            bpm: 0,
            keyscale: String::new(),
            timesignature: String::new(),
            vocal_language: String::new(),
            inference_steps: 0,
            guidance_scale: 0.0,
            shift: 0.0,
            seed: -1,
            output_format: "mp3".into(),
            synth_batch_size: 1,
            lm_model: String::new(),
            synth_model: String::new(),
            solver: default_solver(),
            task_type: default_task_type(),
            repainting_start: 0.0,
            repainting_end: minus_one_f32(),
            audio_cover_strength: one_f32(),
            source_audio: String::new(),
            ref_audio: String::new(),
            title: String::new(),
            vocal_gender: String::new(),
            lm_temperature: default_lm_temperature(),
            lm_cfg_scale: default_lm_cfg_scale(),
            lm_top_p: default_lm_top_p(),
            lm_top_k: 0,
            lm_negative_prompt: String::new(),
            use_cot_caption: true,
            audio_codes: String::new(),
            adapter: String::new(),
            adapter_scale: one_f32(),
            track: String::new(),
            negative_prompt: String::new(),
            vae: String::new(),
            pp_vae_reencode: false,
            lm_adapter: String::new(),
            lm_adapter_scale: one_f32(),
            postprocess_plugin: String::new(),
            lm_cfg_cutoff_ratio: one_f32(),
            custom_timesteps: String::new(),
            mp3_bitrate: default_mp3_bitrate(),
            dit_sliding_window: minus_one_i32(),
        }
    }
}

/// The backend matches model names against the registry by full filename, so
/// a bare stem such as `acestep-5Hz-lm-1.7B-Q8_0` is reported as "LM not
/// found" even though the file is present. Empty stays empty: that is how the
/// caller says "whatever is resident".
fn gguf_name(name: &str) -> String {
    if name.is_empty() || name.ends_with(".gguf") {
        return name.to_string();
    }
    format!("{name}.gguf")
}

impl GenerateParams {
    /// The request the LM accepts. Empty fields are what make the LM fill
    /// them, so defaults here are deliberate omissions rather than zeroes.
    fn to_lm_json(&self) -> serde_json::Value {
        // Gender is not a model parameter. It is appended to the caption as
        // words, which is how the reference UI does it and the only mechanism
        // the planner has to go on.
        let caption = match self.vocal_gender.as_str() {
            "male" => append_line(&self.caption, "Male vocals"),
            "female" => append_line(&self.caption, "Female vocals"),
            _ => self.caption.clone(),
        };
        serde_json::json!({
            "caption": caption,
            "lyrics": self.lyrics,
            "duration": self.duration,
            "bpm": self.bpm,
            "keyscale": self.keyscale,
            "timesignature": self.timesignature,
            "vocal_language": self.vocal_language,
            "lm_temperature": self.lm_temperature,
            "lm_cfg_scale": self.lm_cfg_scale,
            "lm_top_p": self.lm_top_p,
            "lm_top_k": self.lm_top_k,
            "lm_negative_prompt": self.lm_negative_prompt,
            "use_cot_caption": self.use_cot_caption,
            "output_format": self.output_format,
            "synth_batch_size": self.synth_batch_size,
            // Empty strings are meaningful: the backend keeps whatever is
            // resident, or falls to the first registry entry.
            "lm_model": gguf_name(&self.lm_model),
            "synth_model": gguf_name(&self.synth_model),
            "solver": self.solver,
            // These four were declared on the struct and shown in the Sampling
            // panel but never sent, so the panel was decorative: the backend
            // fell back to the turbo model defaults (8 steps, guidance 1.0,
            // shift 3.0) no matter what was set. `/lm` echoes every field back
            // on the request it hands to `/synth`, so sending them here is
            // enough to reach the sampler.
            // 0 means "model default" and -1 means random; both are resolved
            // server-side, so they are passed through unchanged.
            "inference_steps": self.inference_steps,
            "guidance_scale": self.guidance_scale,
            "shift": self.shift,
            "seed": self.seed,
            "task_type": self.task_type,
            "repainting_start": self.repainting_start,
            "repainting_end": self.repainting_end,
            "audio_cover_strength": self.audio_cover_strength,
            // HOT-Step CPP planner-side extras. The bundled engine reads request
            // fields one at a time and ignores what it lacks, so these are inert
            // there rather than an error. `adapter` is deliberately absent: it
            // belongs to the DiT and rides in the synth payload below.
            "negative_prompt": self.negative_prompt,
            "lm_cfg_cutoff_ratio": self.lm_cfg_cutoff_ratio,
            "lm_adapter": self.lm_adapter,
            "lm_adapter_scale": self.lm_adapter_scale,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    pub id: String,
    /// Absolute path on disk; the frontend turns this into an asset URL.
    pub path: String,
    pub format: String,
    pub bytes: u64,
    pub caption: String,
    pub lyrics: String,
    pub bpm: i32,
    pub keyscale: String,
    pub timesignature: String,
    pub vocal_language: String,
    pub duration_secs: f32,
    pub created_at: u64,
    /// Absolute path to this track's cover image, if one could be assigned.
    pub cover: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateOutcome {
    pub tracks: Vec<Track>,
    pub cancelled: bool,
}

use crate::paths::library_dir;

pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// POST and read back the job id.
/// Submit a JSON body to any engine path and return the job id.
///
/// Public because the YuE2 route lives on the same server but is not part of the
/// two-stage ACE-Step flow.
pub fn submit_json(port: u16, path: &str, body: serde_json::Value) -> Result<String, String> {
    submit(port, path, body)
}

fn submit(port: u16, path: &str, body: serde_json::Value) -> Result<String, String> {
    let url = format!("http://127.0.0.1:{port}{path}");
    let response = ureq::post(&url)
        .timeout(std::time::Duration::from_secs(60))
        .send_json(body)
        .map_err(|e| format!("submit {path} failed: {e}"))?;

    job_id_from(response.into_json().map_err(|e| format!("bad submit response: {e}"))?, path)
}

/// Force LRC timestamps strictly increasing.
///
/// The aligner works at token granularity and can place several lines on the
/// same frame, which produces two rows with an identical timestamp. A karaoke
/// display cannot show that: whichever line is checked last wins and the other
/// flashes. A small floor keeps every line reachable in order.
fn sanitise_lrc(lrc: &str) -> String {
    /// Shortest gap between two lines, in seconds. Below this the display
    /// cannot usefully show one line before the next.
    const MIN_GAP: f32 = 0.25;

    let mut out = String::with_capacity(lrc.len());
    let mut previous: Option<f32> = None;

    for line in lrc.lines() {
        let Some((stamp, rest)) = line.split_once(']') else {
            out.push_str(line);
            out.push('\n');
            continue;
        };
        let Some((mins, secs)) = stamp.trim_start_matches('[').split_once(':') else {
            out.push_str(line);
            out.push('\n');
            continue;
        };
        let (Ok(mins), Ok(secs)) = (mins.parse::<f32>(), secs.parse::<f32>()) else {
            out.push_str(line);
            out.push('\n');
            continue;
        };

        let mut at = mins * 60.0 + secs;
        if let Some(prev) = previous {
            at = at.max(prev + MIN_GAP);
        }
        previous = Some(at);

        let m = at.floor() as u32;
        let s = at - m as f32;
        out.push_str(&format!("[{m:02}:{s:05.2}]{rest}\n"));
    }
    out
}

/// True when the render is instrumental, so lyric alignment is pointless.
fn is_instrumental(params: &GenerateParams) -> bool {
    let lyrics = params.lyrics.trim();
    lyrics.eq_ignore_ascii_case("[instrumental]") || lyrics.is_empty()
}

fn job_id_from(value: serde_json::Value, path: &str) -> Result<String, String> {
    value
        .get("id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| format!("{path} returned no job id: {value}"))
}

/// POST `/synth` as multipart so audio can ride along with the request.
///
/// Two independent file parts, matching the server's names exactly:
/// - `audio` — the source to repaint or cover. Optional.
/// - `ref_audio` — a timbre reference. Optional, and valid on its own:
///   `run_text2music` takes a reference with no source at all.
///
/// Sending either one switches the request from plain JSON to multipart, which
/// is why this is not just "always multipart": the JSON path is cheaper and is
/// what a plain text-to-music render should keep using.
pub fn submit_synth_with_audio(
    port: u16,
    requests: &serde_json::Value,
    source: Option<&Path>,
    reference: Option<&Path>,
) -> Result<String, String> {
    let json = serde_json::to_string(requests).map_err(|e| e.to_string())?;

    let read = |p: &Path| -> Result<Vec<u8>, String> {
        std::fs::read(p).map_err(|e| format!("cannot read {}: {e}", p.display()))
    };
    let source_bytes = source.map(read).transpose()?;
    let ref_bytes = reference.map(read).transpose()?;

    if source_bytes.is_none() && ref_bytes.is_none() {
        return submit(port, "/synth", requests.clone());
    }

    // Boundary only has to be unique in this body; the parts are WAV or MP3,
    // never multipart text themselves.
    let boundary = format!("----aceforge{}", now_nanos());
    let mut form = Vec::with_capacity(json.len() + 512);
    form.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    form.extend_from_slice(b"Content-Disposition: form-data; name=\"request\"\r\n\r\n");
    form.extend_from_slice(json.as_bytes());

    let part = |form: &mut Vec<u8>, name: &str, filename: &str, bytes: &[u8]| {
        form.extend_from_slice(format!("\r\n--{boundary}\r\n").as_bytes());
        form.extend_from_slice(
            format!(
                "Content-Disposition: form-data; name=\"{name}\"; filename=\"{filename}\"\r\n\r\n"
            )
            .as_bytes(),
        );
        form.extend_from_slice(bytes);
    };
    if let Some(b) = &source_bytes {
        part(&mut form, "audio", "source", b);
    }
    if let Some(b) = &ref_bytes {
        part(&mut form, "ref_audio", "reference", b);
    }
    form.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

    let response = ureq::post(&format!("http://127.0.0.1:{port}/synth"))
        .timeout(std::time::Duration::from_secs(300))
        // `Request::set` is the setter in ureq 2; `header` is a getter.
        .set(
            "Content-Type",
            &format!("multipart/form-data; boundary={boundary}"),
        )
        .send(&form[..])
        .map_err(|e| format!("submit /synth failed: {e}"))?;

    job_id_from(
        response.into_json().map_err(|e| format!("bad submit response: {e}"))?,
        "/synth",
    )
}

fn now_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

/// Only ever read audio from inside our own library.
///
/// `source_audio` and `ref_audio` both arrive from the frontend and are then
/// uploaded to the backend, so they are treated as untrusted: a path that
/// escapes the library would let the webview read an arbitrary file off disk.
fn resolve_library_audio(path: &str) -> Result<Option<PathBuf>, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let lib = library_dir();
    let target = PathBuf::from(trimmed);
    if !target.is_file() {
        return Err(format!("audio not found: {trimmed}"));
    }
    // Compare canonical paths: `starts_with` alone is defeated by `..`.
    let real = target
        .canonicalize()
        .map_err(|e| format!("cannot resolve {trimmed}: {e}"))?;
    let lib_real = lib
        .canonicalize()
        .map_err(|e| format!("library directory missing: {e}"))?;
    if !real.starts_with(&lib_real) {
        return Err("refusing to read audio outside the library".to_string());
    }
    Ok(Some(real))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobState {
    Running,
    Done,
    Failed,
    Cancelled,
}

impl JobState {
    fn parse(s: &str) -> JobState {
        match s {
            "done" => JobState::Done,
            "failed" => JobState::Failed,
            "cancelled" => JobState::Cancelled,
            _ => JobState::Running,
        }
    }
}

/// Pull a readable reason out of the sidecar's stderr.
///
/// `lines` must cover only this job's output (`server::stderr_since`), so
/// anything unmatched here really is the failure rather than leftovers from
/// the last render. The vocabulary is taken from what `ace-server` actually
/// prints -- `not found` covers "LM/DiT/VAE/Adapter not found", which is a
/// whole class of misconfiguration and shares no keyword with `FATAL`.
///
/// WARNING lines are dropped unless they also say FATAL: several are routine
/// (`failed to allocate partial head buffer` is a fallback, not an error).
fn backend_diagnostic(lines: &[String]) -> String {
    const MAX_LINES: usize = 3;

    if lines.is_empty() {
        return "no reason given (the backend wrote nothing to stderr)".to_string();
    }

    let flagged: Vec<&String> = lines
        .iter()
        .filter(|l| {
            let u = l.to_ascii_uppercase();
            let bad = u.contains("FATAL")
                || u.contains("ERROR")
                || u.contains("NOT FOUND")
                || u.contains("CANNOT")
                || u.contains("TRUNCATED")
                || u.contains("UNSUPPORTED")
                || (u.contains("FAILED") && !u.contains("WARNING"));
            bad
        })
        .collect();

    // Nothing matched: say so rather than passing warnings off as the cause.
    // The tail is still shown, because "the backend said nothing useful" plus
    // its last words beats a bare "no reason given".
    let picked: Vec<&String> = if !flagged.is_empty() {
        flagged.iter().rev().take(MAX_LINES).rev().copied().collect()
    } else {
        return format!(
            "no explicit error from the backend; last output: {}",
            clip_lines(&lines.iter().rev().take(MAX_LINES).rev().collect::<Vec<_>>(), 0)
        );
    };

    clip_lines(&picked, 0)
}

/// Join log lines for display: newest-last, each clipped so a multi-line
/// planner caption cannot flood the toast.
fn clip_lines(lines: &[&String], max_lines: usize) -> String {
    const MAX_LINE: usize = 160;
    let take = if max_lines == 0 { lines.len() } else { max_lines };
    lines
        .iter()
        .take(take)
        .map(|l| {
            let l = l.trim();
            if l.chars().count() > MAX_LINE {
                let cut: String = l.chars().take(MAX_LINE).collect();
                format!("{cut}...")
            } else {
                l.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" | ")
}

/// Poll until the job leaves RUNNING. Returns Err on timeout so a wedged
/// server surfaces instead of hanging the UI forever.
///
/// `mark` is the stderr log length captured before the job was submitted; only
/// bytes past it are read if the job fails.
pub fn wait_for_job(
    port: u16,
    id: &str,
    mark: u64,
    on_stage: &mut dyn FnMut(&str),
) -> Result<JobState, String> {
    let deadline = std::time::Instant::now() + JOB_TIMEOUT;
    let mut last_report = std::time::Instant::now();
    let models_dir = crate::settings::resolve_models_dir(None);
    let mut last_progress = std::time::Instant::now();
    let mut seen_len = crate::server::stderr_len(&models_dir);

    while std::time::Instant::now() < deadline {
        // Watchdog. The sidecar logs continuously through loads, sampling and
        // decode, so a log that stops growing means the job has stopped making
        // progress. Without this the old one-hour ceiling was the only guard,
        // and an indefinite hang looked exactly like a slow render.
        let len = crate::server::stderr_len(&models_dir);
        if len != seen_len {
            seen_len = len;
            last_progress = std::time::Instant::now();
        } else if last_progress.elapsed() > STALL_TIMEOUT {
            return Err(format!(
                "job {id} stalled — the backend stopped writing to its log for {}s. \\
                 The runtime is probably wedged; restart it from the sidebar.",
                STALL_TIMEOUT.as_secs()
            ));
        }

        let url = format!("http://127.0.0.1:{port}/job?id={id}");
        let body = ureq::get(&url)
            .timeout(std::time::Duration::from_secs(10))
            .call()
            .map_err(|e| format!("poll failed: {e}"))?
            .into_string()
            .map_err(|e| e.to_string())?;

        let value: serde_json::Value =
            serde_json::from_str(&body).map_err(|e| format!("bad poll body: {e}"))?;
        let state = JobState::parse(value.get("status").and_then(|v| v.as_str()).unwrap_or("running"));

        match state {
            JobState::Done => return Ok(JobState::Done),
            JobState::Failed => {
                // A failed job carries no error field: `Job` in `ace-server.cpp`
                // has nowhere to store one, and `/job` answers a bare
                // `{"status":"failed"}`. Reading `error` here is therefore only
                // useful against a future backend; the explanation we actually
                // have is the sidecar's stderr, which is where every FATAL and
                // every "LM not found" lands. Without this the UI only ever
                // shows "job <id> failed: no reason given" and a corrupt GGUF
                // looks identical to an OOM.
                let detail = value
                    .get("error")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| {
                        let dir = crate::settings::resolve_models_dir(None);
                        backend_diagnostic(&crate::server::stderr_since(&dir, mark, 80))
                    });
                return Err(format!("job {id} failed: {detail}"));
            }
            JobState::Cancelled => return Ok(JobState::Cancelled),
            JobState::Running => {
                // Throttle: a 50 step DiT on a mobile GPU takes a while, and
                // flooding the UI channel is worse than a quiet progress bar.
                if last_report.elapsed() > std::time::Duration::from_secs(2) {
                    on_stage("running");
                    last_report = std::time::Instant::now();
                }
                std::thread::sleep(POLL_INTERVAL);
            }
        }
    }
    Err(format!("job {id} timed out"))
}

/// The result body plus any `X-LRC-Text` header, which is base64 LRC text.
pub struct FetchedResult {
    pub body: Vec<u8>,
    pub lrc: Option<String>,
}

pub fn fetch_result(port: u16, id: &str) -> Result<FetchedResult, String> {
    let url = format!("http://127.0.0.1:{port}/job?id={id}&result=1");
    let response = ureq::get(&url)
        .timeout(std::time::Duration::from_secs(120))
        .call()
        .map_err(|e| format!("result fetch failed: {e}"))?;
    let lrc = response
        .header("X-LRC-Text")
        .map(decode_base64)
        .transpose()?;
    let body = response
        .into_reader()
        .read_to_end_bytes()
        .map_err(|e| format!("result read failed: {e}"))?;
    Ok(FetchedResult { body, lrc })
}

/// Decode standard base64. Written out rather than pulled from a crate: the
/// app already vendors its dependencies and this is one call site.
///
/// Padding is part of the alphabet, not a stray character: the server pads the
/// `X-LRC-Text` header to a multiple of four, and any LRC whose length is not a
/// multiple of three ends in `=` or `==`. Treating `=` as invalid failed
/// roughly two jobs in three.
fn decode_base64(input: &str) -> Result<String, String> {
    fn value(c: u8) -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some((c - b'A') as u32),
            b'a'..=b'z' => Some((c - b'a') as u32 + 26),
            b'0'..=b'9' => Some((c - b'0') as u32 + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            b'=' => Some(0),
            _ => None,
        }
    }
    let bytes: Vec<u8> = input.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    if bytes.len() % 4 == 1 {
        return Err("truncated base64".to_string());
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for quad in bytes.chunks(4) {
        let mut acc: u32 = 0;
        for c in quad {
            acc = (acc << 6) | value(*c).ok_or_else(|| format!("bad base64 character {:?}", *c as char))?;
        }
        let decoded = acc.to_be_bytes();
        // `acc` holds at most 24 bits, so its three bytes are the last three of
        // the big-endian form. A quad carries three bytes, minus one per pad.
        let keep = 3usize.saturating_sub(quad.iter().filter(|c| **c == b'=').count());
        out.extend_from_slice(&decoded[1..1 + keep]);
    }
    String::from_utf8(out).map_err(|e| format!("LRC was not valid UTF-8: {e}"))
}

pub fn cancel(port: u16, id: &str) -> Result<(), String> {
    let url = format!("http://127.0.0.1:{port}/job?id={id}&cancel=1");
    ureq::post(&url)
        .send_bytes(&[])
        .map_err(|e| format!("cancel failed: {e}"))?;
    Ok(())
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Pull the audio parts out of a multipart/mixed body.
///
/// Latent parts ride along in the same envelope as `application/octet-stream`
/// and are skipped: they are raw f32 tensors, useful for cheap re-renders
/// later but not playable.
pub fn parse_audio_parts(body: &[u8]) -> Vec<(String, Vec<u8>)> {
    let delim = format!("--{BOUNDARY}");
    let mut out = Vec::new();
    let mut cursor = 0usize;

    while let Some(rel) = find(&body[cursor..], delim.as_bytes()) {
        let start = cursor + rel + delim.len();
        // Skip the CRLF that follows the boundary line.
        let start = if body[start..].starts_with(b"\r\n") {
            start + 2
        } else {
            start
        };

        let Some(next_rel) = find(&body[start..], delim.as_bytes()) else {
            break;
        };
        let mut chunk = &body[start..start + next_rel];
        // Trailing CRLF before the next boundary is framing, not payload.
        if chunk.ends_with(b"\r\n") {
            chunk = &chunk[..chunk.len() - 2];
        }

        // Headers end at the first blank line.
        if let Some(split) = find(chunk, b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&chunk[..split]).to_lowercase();
            if let Some(mime) = headers
                .lines()
                .find_map(|l| l.strip_prefix("content-type:"))
                .map(|v| v.trim().to_string())
            {
                if mime.starts_with("audio/") {
                    let data = chunk[split + 4..].to_vec();
                    out.push((mime, data));
                }
            }
        }
        cursor = start + next_rel;
    }
    out
}

/// Pull fields off an LM-produced AceRequest so the UI can show what the
/// planner actually decided.
fn meta_from_request(value: &serde_json::Value) -> (String, String, i32, String, String, String, f32) {
    let s = |k: &str| value.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    (
        s("caption"),
        s("lyrics"),
        value.get("bpm").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
        s("keyscale"),
        s("timesignature"),
        s("vocal_language"),
        value.get("duration").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
    )
}

/// Run the full pipeline: LM writes lyrics and audio codes, then DiT + VAE
/// render them to audio.
/// `on_job` receives the id of whichever job is currently running. Without it
/// the caller has no way to cancel: `generate` polls internally and would
/// otherwise only ever report completion.
pub fn generate(
    port: u16,
    params: &GenerateParams,
    on_stage: &mut dyn FnMut(&str, &str),
    on_job: &mut dyn FnMut(&str),
) -> Result<GenerateOutcome, String> {
    if params.caption.trim().is_empty() {
        return Err("caption is required".to_string());
    }

    // Resolve wildcards here rather than in the UI. Expansion is a function of
    // the seed, so doing it before submission means the text that reaches the
    // engine is the text recorded on the job, and a retry renders the same
    // prompt instead of rolling the dice again. The seed itself was already
    // made concrete when the job was enqueued.
    let expanded_caption = crate::wildcards::expand(&params.caption, params.seed);
    let expanded_lyrics = crate::wildcards::expand(&params.lyrics, params.seed);
    let params = &{
        let mut p = params.clone();
        p.caption = expanded_caption;
        p.lyrics = expanded_lyrics;
        p
    };

    let out_dir = library_dir();
    std::fs::create_dir_all(&out_dir)
        .map_err(|e| format!("cannot create {}: {e}", out_dir.display()))?;

    // Stage 1: planner.
    //
    // Deliberately a single request. Asking the planner for N variations
    // requires posting an array, and the server batches those through CFU as
    // 2*N sequences: measured on this card, 3 items wants a 7.1 GiB KV cache
    // and the process dies allocating it. Multi-song output is therefore
    // driven from `synth_batch_size`, which renders several takes from one
    // plan and was verified to fit. A planner that cannot run is not a good
    // knob to leave on a 4 GB card.
    on_stage("lm", "planning lyrics");
    let models_dir = crate::settings::resolve_models_dir(None);
    let lm_mark = crate::server::stderr_mark(&models_dir);
    let lm_id = submit(port, "/lm", params.to_lm_json())?;
    on_job(&lm_id);
    let lm_state = wait_for_job(port, &lm_id, lm_mark, &mut |_| on_stage("lm", "planning lyrics"))?;

    if lm_state == JobState::Cancelled {
        return Ok(GenerateOutcome { tracks: Vec::new(), cancelled: true });
    }

    let lm_body = fetch_result(port, &lm_id)?.body;
    let lm_json: serde_json::Value = serde_json::from_slice(&lm_body)
        .map_err(|e| format!("bad /lm result: {e}"))?;

    // The LM always answers with an array, one entry per variation.
    let requests = match &lm_json {
        serde_json::Value::Array(items) => items.clone(),
        other => vec![other.clone()],
    };
    if requests.is_empty() {
        return Err("planner returned no songs".to_string());
    }

    // Stage 2: synthesis.
    on_stage("synth", "rendering audio");
    let source = resolve_library_audio(&params.source_audio)?;
    let reference = resolve_library_audio(&params.ref_audio)?;
    let synth_mark = crate::server::stderr_mark(&models_dir);

    // A user-supplied arrangement replaces the planner's, and an adapter or a
    // stem name is applied on top. All three are decided here rather than
    // before `/lm`, because each is a DiT-side setting and each costs nothing
    // when left empty.
    let adapter = params.adapter.trim();
    let track = params.track.trim();
    let codes = params.audio_codes.trim();
    // HOT-Step's decoder and solver extras are DiT-side, so they belong here for
    // the same reason `adapter` does: the planner has no use for them.
    let vae = params.vae.trim();
    let postprocess = params.postprocess_plugin.trim();
    let timesteps = params.custom_timesteps.trim();
    let needs_dit_extras =
        params.pp_vae_reencode || !vae.is_empty() || !postprocess.is_empty()
            || !timesteps.is_empty() || params.mp3_bitrate != default_mp3_bitrate()
            || params.dit_sliding_window != minus_one_i32();
    let requests = if codes.is_empty()
        && adapter.is_empty()
        && track.is_empty()
        && !needs_dit_extras
    {
        requests
    } else {
        let mut out = Vec::with_capacity(requests.len());
        for mut r in requests {
            if let Some(obj) = r.as_object_mut() {
                if !codes.is_empty() {
                    obj.insert("audio_codes".into(), codes.into());
                }
                if !adapter.is_empty() {
                    // The server resolves this against the adapter registry it
                    // built from `--adapters`, by name.
                    obj.insert("adapter".into(), adapter.into());
                    obj.insert("adapter_scale".into(), serde_json::json!(params.adapter_scale));
                }
                if !track.is_empty() {
                    obj.insert("track".into(), track.into());
                }
                if !vae.is_empty() {
                    obj.insert("vae".into(), vae.into());
                }
                if params.pp_vae_reencode {
                    obj.insert("pp_vae_reencode".into(), true.into());
                }
                if !postprocess.is_empty() {
                    obj.insert("postprocess_plugin".into(), postprocess.into());
                }
                if !timesteps.is_empty() {
                    obj.insert("custom_timesteps".into(), timesteps.into());
                }
                if params.mp3_bitrate != default_mp3_bitrate() {
                    obj.insert("mp3_bitrate".into(), params.mp3_bitrate.into());
                }
                if params.dit_sliding_window != minus_one_i32() {
                    obj.insert(
                        "dit_sliding_window".into(),
                        params.dit_sliding_window.into(),
                    );
                }
            }
            out.push(r);
        }
        out
    };

    // Ask the engine for lyric timestamps. The engine runs an extra DiT pass
    // for cross-attention alignment, which is opt-in and costs a little time;
    // it is skipped for instrumental tracks, where there is nothing to align.
    let wants_lrc = !is_instrumental(&params);
    let mut requests = requests;
    if wants_lrc {
        for r in requests.iter_mut() {
            if let Some(obj) = r.as_object_mut() {
                obj.insert("get_lrc".to_string(), serde_json::Value::Bool(true));
            }
        }
    }

    let synth_id = submit_synth_with_audio(
        port,
        &serde_json::Value::Array(requests.clone()),
        source.as_deref(),
        reference.as_deref(),
    )?;
    on_job(&synth_id);
    let synth_state = wait_for_job(port, &synth_id, synth_mark, &mut |_| on_stage("synth", "rendering audio"))?;

    if synth_state == JobState::Cancelled {
        return Ok(GenerateOutcome { tracks: Vec::new(), cancelled: true });
    }

    on_stage("synth", "decoding audio");
    let fetched = fetch_result(port, &synth_id)?;
    let result = fetched.body;
    let parts = parse_audio_parts(&result);
    if parts.is_empty() {
        return Err("synth returned no audio parts".to_string());
    }

    let stamp = now_secs();
    let extension = if params.output_format.starts_with("wav") { "wav" } else { "mp3" };
    let mut tracks = Vec::new();

    for (index, (mime, data)) in parts.into_iter().enumerate() {
        if data.is_empty() {
            continue;
        }
        let format = mime.trim_start_matches("audio/").to_string();
        // `synth_batch_size` renders several takes from one plan, so the
        // server returns a single request but multiple audio parts. Every take
        // belongs to that plan and must carry its metadata, not just the first.
        let meta = requests
            .get(index)
            .or_else(|| requests.first())
            .map(meta_from_request)
            .unwrap_or_default();

        let id = format!("{stamp}-{index}");
        // Name the file after the song. A user-supplied title wins; otherwise
        // fall back to the caption, then the lyrics. `1791555467-0.mp3` made
        // the library unscannable and revealed nothing in a file manager.
        let stem = crate::paths::song_stem(&params.title, &meta.0, &meta.1);
        let path = crate::paths::unique_path(&out_dir, &stem, extension);
        let id = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or(id);
        std::fs::write(&path, &data)
            .map_err(|e| format!("cannot write {}: {e}", path.display()))?;

        // Write the LRC beside the audio so the library can pick it up. A
        // failure here must not lose the render -- the song is already on disk
        // and the lyrics are still in the request.
        //
        // Only the take it was measured against. The engine aligns lyric tokens
        // to the audio produced in that pass, and with `synth_batch_size` each
        // take is a different performance, so handing the same LRC to all of
        // them times take 2's singing against take 1's audio. Every take but
        // the first still gets lyrics -- they simply do not follow along until
        // they are re-timed.
        if index == 0 {
            if let Some(lrc) = fetched.lrc.as_deref().map(|text| sanitise_lrc(text)) {
                let lrc_path = path.with_extension("lrc");
                if let Err(e) = std::fs::write(&lrc_path, &lrc) {
                    eprintln!("[lrc] cannot write {}: {e}", lrc_path.display());
                } else {
                    eprintln!("[lrc] wrote {} ({} bytes)", lrc_path.display(), lrc.len());
                }
            }
        }

        let track = Track {
            id,
            bytes: data.len() as u64,
            path: path.display().to_string(),
            format,
            caption: meta.0,
            lyrics: meta.1,
            bpm: meta.2,
            keyscale: meta.3,
            timesignature: meta.4,
            vocal_language: meta.5,
            duration_secs: meta.6,
            created_at: stamp,
            cover: crate::covers::cover_for(&path.display().to_string()),
        };
        write_metadata(&track);
        tracks.push(track);
    }

    if tracks.is_empty() {
        return Err("all audio parts were empty".to_string());
    }

    on_stage("done", "complete");
    Ok(GenerateOutcome { tracks, cancelled: false })
}

/// Metadata sidecar, `<track>.json` beside the audio file.
fn metadata_path(track: &Track) -> PathBuf {
    let mut p = PathBuf::from(&track.path);
    p.set_extension("json");
    p
}

/// Also used by the YuE2 path, which writes its own tracks into the same library.
pub fn write_metadata(track: &Track) {
    if let Ok(text) = serde_json::to_string_pretty(track) {
        let _ = std::fs::write(metadata_path(track), text);
    }
}

/// Every track already on disk, newest first. Lets the UI survive a restart
/// without keeping its own index.
pub fn list_tracks() -> Vec<Track> {
    let dir = library_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };

    let mut files: Vec<(u64, PathBuf)> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let path = e.path();
            let ext = path.extension()?.to_str()?.to_ascii_lowercase();
            if ext != "mp3" && ext != "wav" {
                return None;
            }
            let meta = e.metadata().ok()?;
            let modified = meta
                .modified()
                .ok()
                .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            Some((modified, path))
        })
        .collect();

    files.sort_by(|a, b| b.0.cmp(&a.0));
    files
        .into_iter()
        .map(|(_, path)| track_from_path(&path))
        .collect()
}

fn track_from_path(path: &Path) -> Track {
    // Prefer the sidecar: it carries the planner's decisions (caption, lyrics,
    // bpm, key) which cannot be recovered from the audio itself.
    if let Ok(text) = std::fs::read_to_string(path.with_extension("json")) {
        if let Ok(mut stored) = serde_json::from_str::<Track>(&text) {
            // Trust the filesystem for size; the sidecar may be stale.
            stored.bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(stored.bytes);
            stored.path = path.display().to_string();
            // Cover art is not stored in the sidecar. It used to be assigned
            // only on the fallback below, which every track with metadata skips
            // by returning here -- so 40 of 41 songs had none and the library
            // showed a broken image for them.
            if stored.cover.is_none() {
                stored.cover = crate::covers::cover_for(&path.display().to_string());
            }
            return stored;
        }
    }

    let id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();
    let extension = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("mp3")
        .to_string();
    let bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);

    Track {
        id: id.clone(),
        path: path.display().to_string(),
        format: if extension == "wav" { "wav".into() } else { "mpeg".into() },
        bytes,
        caption: String::new(),
        lyrics: String::new(),
        bpm: 0,
        keyscale: String::new(),
        timesignature: String::new(),
        vocal_language: String::new(),
        duration_secs: 0.0,
        cover: crate::covers::cover_for(&path.display().to_string()),
        created_at: id
            .split('-')
            .next()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0),
    }
}
/// Read a boxed reader to the end. Local helper so the call site stays
/// readable and the import stays scoped to one place.
trait ReadToEnd {
    fn read_to_end_bytes(&mut self) -> std::io::Result<Vec<u8>>;
}

impl<R: std::io::Read + ?Sized> ReadToEnd for R {
    fn read_to_end_bytes(&mut self) -> std::io::Result<Vec<u8>> {
        let mut buf = Vec::new();
        std::io::Read::read_to_end(self, &mut buf)?;
        Ok(buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encodes the way `ace-server` does, so the round trip covers the padding
    /// the server actually emits.
    fn encode(input: &[u8]) -> String {
        const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in input.chunks(3) {
            let mut v = (chunk[0] as u32) << 16;
            if chunk.len() > 1 {
                v |= (chunk[1] as u32) << 8;
            }
            if chunk.len() > 2 {
                v |= chunk[2] as u32;
            }
            out.push(T[(v >> 18) as usize & 63] as char);
            out.push(T[(v >> 12) as usize & 63] as char);
            out.push(if chunk.len() > 1 { T[(v >> 6) as usize & 63] as char } else { '=' });
            out.push(if chunk.len() > 2 { T[v as usize & 63] as char } else { '=' });
        }
        out
    }

    /// `=` was rejected as a bad character, so any LRC whose length was not a
    /// multiple of three failed to decode and took the whole render down with
    /// it. Every length modulo three must survive.
    #[test]
    fn decodes_padded_base64() {
        for len in 0..64usize {
            let raw: Vec<u8> = (0..len).map(|i| b'a' + (i % 26) as u8).collect();
            let encoded = encode(&raw);
            assert!(encoded.contains('=') || raw.is_empty() || len % 3 == 0);
            assert_eq!(decode_base64(&encoded).unwrap(), String::from_utf8(raw).unwrap());
        }
    }

    #[test]
    fn decodes_real_lrc_shape() {
        let lrc = "[00:07.84]Yeah mon... bless up... one love inna di place\n[00:11.85]Mi wake up early\n";
        assert_eq!(decode_base64(&encode(lrc.as_bytes())).unwrap(), lrc);
    }

    #[test]
    fn rejects_bad_characters() {
        assert!(decode_base64("not base64!!").is_err());
        assert!(decode_base64("AAAAA").is_err(), "a length of 5 cannot be valid");
    }

    /// The Sampling panel showed steps, guidance, shift and seed, but
    /// `to_lm_json` never sent them. The backend silently used the turbo
    /// defaults, so the controls did nothing. `/lm` echoes each field back on
    /// the request it hands to `/synth`, which is what makes sending them here
    /// sufficient.
    #[test]
    fn sampling_controls_reach_the_backend() {
        let p = GenerateParams {
            caption: "test".to_string(),
            inference_steps: 25,
            guidance_scale: 3.5,
            shift: 2.0,
            seed: 12345,
            ..Default::default()
        };
        let v = p.to_lm_json();
        assert_eq!(v["inference_steps"], 25);
        assert_eq!(v["guidance_scale"].as_f64().unwrap(), 3.5);
        assert_eq!(v["shift"].as_f64().unwrap(), 2.0);
        assert_eq!(v["seed"], 12345);
    }

    /// Sentinels mean something specific to the backend and must survive:
    /// 0 is "model default" for steps/guidance/shift, -1 is a random seed.
    #[test]
    fn sentinels_pass_through_unchanged() {
        let p = GenerateParams {
            caption: "test".to_string(),
            ..Default::default()
        };
        let v = p.to_lm_json();
        assert_eq!(v["inference_steps"], 0);
        assert_eq!(v["guidance_scale"].as_f64().unwrap(), 0.0);
        assert_eq!(v["seed"], -1);
        assert_eq!(v["task_type"], "text2music");
        assert_eq!(v["repainting_end"].as_f64().unwrap(), -1.0);
        assert_eq!(v["audio_cover_strength"].as_f64().unwrap(), 1.0);
    }

    #[test]
    fn repaint_region_reaches_the_backend() {
        let p = GenerateParams {
            caption: "test".to_string(),
            task_type: "repaint".to_string(),
            repainting_start: 2.0,
            repainting_end: 6.5,
            audio_cover_strength: 0.8,
            ..Default::default()
        };
        let v = p.to_lm_json();
        assert_eq!(v["task_type"], "repaint");
        assert_eq!(v["repainting_start"].as_f64().unwrap(), 2.0);
        assert_eq!(v["repainting_end"].as_f64().unwrap(), 6.5);
        // Compared against the f32 it came from: JSON widens 0.8f32 to
        // 0.800000011920929, so an exact f64 comparison would fail.
        assert_eq!(v["audio_cover_strength"].as_f64().unwrap(), 0.8f32 as f64);
    }

    /// `source_audio` is a path from the frontend that then gets uploaded to
    /// the backend. An empty one means text-to-music and must not error.
    /// The planner sampling knobs used to be hardcoded here, so the reference
    /// UI's equivalents could not have worked: 0.85 / 2.0 / 0.9 and CoT always
    /// on, with top-k and the negative prompt never sent at all.
    #[test]
    fn planner_sampling_reaches_the_backend() {
        let p = GenerateParams {
            caption: "test".to_string(),
            lm_temperature: 0.8,
            lm_cfg_scale: 2.2,
            lm_top_p: 0.92,
            lm_top_k: 40,
            lm_negative_prompt: "loud drums".to_string(),
            use_cot_caption: false,
            ..Default::default()
        };
        let v = p.to_lm_json();
        assert_eq!(v["lm_temperature"].as_f64().unwrap(), 0.8f32 as f64);
        assert_eq!(v["lm_cfg_scale"].as_f64().unwrap(), 2.2f32 as f64);
        assert_eq!(v["lm_top_p"].as_f64().unwrap(), 0.92f32 as f64);
        assert_eq!(v["lm_top_k"], 40);
        assert_eq!(v["lm_negative_prompt"], "loud drums");
        assert_eq!(v["use_cot_caption"], false);
    }

    /// Gender is not a model parameter. It works by being appended to the
    /// caption as words, which is what the reference UI does and the only
    /// channel the planner has.
    #[test]
    fn vocal_gender_is_appended_to_the_caption_as_text() {
        let base = "warm lo-fi beat".to_string();
        let male = GenerateParams {
            caption: base.clone(),
            vocal_gender: "male".to_string(),
            ..Default::default()
        };
        let caption = male.to_lm_json()["caption"].as_str().unwrap().to_string();
        assert!(caption.starts_with("warm lo-fi beat"), "got: {caption}");
        assert!(caption.contains("Male vocals"), "got: {caption}");
        // Must not become a field the model might act on.
        let v = male.to_lm_json();
        assert!(v.get("vocal_gender").is_none(), "not a model field: {v}");

        let female = GenerateParams {
            caption: base.clone(),
            vocal_gender: "female".to_string(),
            ..Default::default()
        };
        assert!(female.to_lm_json()["caption"]
            .as_str()
            .unwrap()
            .contains("Female vocals"));

        // No preference must leave the caption exactly as typed.
        let plain = GenerateParams {
            caption: base.clone(),
            ..Default::default()
        };
        assert_eq!(plain.to_lm_json()["caption"].as_str().unwrap(), base);
    }

    #[test]
    fn a_gender_hint_with_no_caption_is_still_sent() {
        let p = GenerateParams {
            caption: String::new(),
            vocal_gender: "female".to_string(),
            ..Default::default()
        };
        assert_eq!(p.to_lm_json()["caption"].as_str().unwrap(), "Female vocals");
    }

    /// `use_ref_audio_is_not_a_request_field` covers `ref_audio`. Adapters are
    /// set on the `/synth` request instead, not the `/lm` one.
    #[test]
    fn adapter_is_not_a_planner_field() {
        let p = GenerateParams {
            caption: "test".to_string(),
            adapter: "my-lora.safetensors".to_string(),
            ..Default::default()
        };
        let v = p.to_lm_json();
        assert!(v.get("adapter").is_none(), "adapter is a DiT setting: {v}");
        assert!(v.get("adapter_scale").is_none(), "{v}");
    }

    /// The HOT-Step extras are split across the two payloads by which model
    /// they configure. Sending the decoder choice to the planner would be
    /// ignored, which reads as "the VAE selector does nothing".
    #[test]
    fn hotstep_extras_land_on_the_side_that_uses_them() {
        let p = GenerateParams {
            caption: "test".to_string(),
            negative_prompt: "no drums".to_string(),
            vae: "scragvae".to_string(),
            lm_adapter: "planner-lora".to_string(),
            postprocess_plugin: "declick".to_string(),
            ..Default::default()
        };
        let lm = p.to_lm_json();
        for planner_side in ["negative_prompt", "lm_adapter", "lm_adapter_scale", "lm_cfg_cutoff_ratio"] {
            assert!(lm.get(planner_side).is_some(), "{planner_side} is a planner field");
        }
        for dit_side in ["vae", "pp_vae_reencode", "postprocess_plugin", "custom_timesteps", "mp3_bitrate"] {
            assert!(lm.get(dit_side).is_none(), "{dit_side} must not ride in the planner payload");
        }
    }

    /// `track` is the stem name and drives the DiT instruction for lego,
    /// extract and complete. It is a synth-side field, so it must not ride in
    /// the planner payload.
    #[test]
    fn the_stem_name_is_not_a_planner_field() {
        let p = GenerateParams {
            caption: "test".to_string(),
            track: "drums".to_string(),
            ..Default::default()
        };
        let v = p.to_lm_json();
        assert!(v.get("track").is_none(), "stem name is a DiT setting: {v}");
    }

    #[test]
    fn no_audio_is_not_an_error() {
        assert!(resolve_library_audio("").unwrap().is_none());
        assert!(resolve_library_audio("   ").unwrap().is_none());
    }

    /// The webview chooses this path, so it must not be able to reach outside
    /// the library. `starts_with` on the raw path is defeated by `..`.
    #[test]
    fn audio_paths_cannot_escape_the_library() {
        let dir = std::env::temp_dir().join(format!("aceforge-src-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        std::fs::write(dir.join("secret.wav"), b"x").expect("write");

        // It is inside the temp dir, not the real library, so this must be
        // refused regardless of how the path is spelled.
        let escape = format!("{}/../../etc/hostname", dir.display());
        let err = resolve_library_audio(&escape).unwrap_err();
        assert!(err.contains("outside the library"), "got: {err}");

        // The reference path is just as untrusted as the source one; it goes
        // to the same place.
        let err = resolve_library_audio(&format!("{}/secret.wav", dir.display())).unwrap_err();
        assert!(err.contains("outside the library"), "got: {err}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The reference is a multipart file part, not a request field, so it must
    /// NOT appear in the LM payload -- the server would ignore it there and we
    /// would think it was wired up.
    #[test]
    fn ref_audio_is_not_a_request_field() {
        let p = GenerateParams {
            caption: "test".to_string(),
            ref_audio: "/library/voice.mp3".to_string(),
            ..Default::default()
        };
        let v = p.to_lm_json();
        assert!(
            v.get("ref_audio").is_none(),
            "ref_audio belongs in a multipart part, not the JSON: {v}"
        );
    }

    #[test]
    fn a_missing_source_is_reported_not_ignored() {
        let err = resolve_library_audio("/definitely/not/here.wav").unwrap_err();
        assert!(err.contains("not found"), "got: {err}");
    }

    /// A title the user typed beats anything derived, and a title made only of
    /// punctuation must not produce an empty stem.
    #[test]
    fn title_beats_caption_and_falls_back_safely() {
        assert_eq!(crate::paths::song_stem("Neon Rain", "warm lo-fi beat", ""), "neon-rain");
        assert_eq!(crate::paths::song_stem("!!!", "warm lo-fi beat", ""), "warm-lo-fi-beat");
        assert_eq!(crate::paths::song_stem("", "", ""), "untitled");
    }

    /// `Track` is camelCase on the wire and must stay that way: it is both the
    /// IPC payload and the on-disk sidecar format. Reading `vocal_language` in
    /// TypeScript while Rust emitted `vocalLanguage` silently dropped the
    /// language and zeroed the duration -- no error anywhere, just data that
    /// never arrived.
    #[test]
    fn track_keeps_its_camel_case_wire_names() {
        let t = Track {
            id: "warm-lo-fi".to_string(),
            path: "/library/warm-lo-fi.mp3".to_string(),
            format: "mpeg".to_string(),
            bytes: 480_000,
            caption: "warm lo-fi".to_string(),
            lyrics: "[verse]".to_string(),
            bpm: 94,
            keyscale: "G major".to_string(),
            timesignature: "2".to_string(),
            vocal_language: "en".to_string(),
            duration_secs: 30.0,
            created_at: 1_700_000_000,
            cover: Some("/covers/01-arcs.svg".to_string()),
        };
        let v = serde_json::to_value(&t).expect("serialize");
        for key in [
            "id",
            "path",
            "format",
            "bytes",
            "caption",
            "lyrics",
            "bpm",
            "keyscale",
            "timesignature",
            "vocalLanguage",
            "durationSecs",
            "createdAt",
        ] {
            assert!(v.get(key).is_some(), "missing {key} in {v}");
        }
        assert!(v.get("vocal_language").is_none(), "snake_case leaked: {v}");
        assert!(v.get("duration_secs").is_none(), "snake_case leaked: {v}");

        // And it must round-trip, or `track_from_path` silently falls back to
        // filesystem-only metadata and loses the caption and lyrics.
        let back: Track = serde_json::from_value(v).expect("round trip");
        assert_eq!(back.vocal_language, "en");
        assert_eq!(back.duration_secs, 30.0);
    }

    /// `keyscale` and `timesignature` are lowercase in BOTH languages. They are
    /// single words, so camelCase leaves them alone -- this guards the guard.
    #[test]
    fn single_word_track_fields_are_not_renamed() {
        let v = serde_json::to_value(GenerateParams::default()).expect("serialize");
        for key in ["keyscale", "timesignature", "caption", "lyrics", "bpm", "seed"] {
            assert!(v.get(key).is_some(), "missing {key} in {v}");
        }
        for key in ["lmModel", "synthModel", "outputFormat", "inferenceSteps"] {
            assert!(v.get(key).is_some(), "missing camelCase {key} in {v}");
        }
    }

    fn owned(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|s| s.to_string()).collect()
    }

    /// The original bug: the real cause was on stderr but the user saw "no
    /// reason given". These are verbatim strings from `ace-server.cpp`.
    #[test]
    fn surfaces_the_corrupt_gguf_message() {
        let lines = owned(&[
            "[Server] Job abc created (1 requests)",
            "[Load] TextEncoder backend: CUDA0 (CPU threads: 2)",
            "[GGUF] FATAL: '/m/Qwen3-Embedding-0.6B-BF16.gguf' is truncated or corrupt.",
            "[Load] FATAL: cannot load /m/Qwen3-Embedding-0.6B-BF16.gguf",
            "[Encode-Text] FATAL: store_require_text_enc failed",
        ]);
        let out = backend_diagnostic(&lines);
        assert!(out.contains("truncated or corrupt"), "got: {out}");
        assert!(!out.contains("no reason given"));
    }

    /// `not found` shares no keyword with FATAL. Missing this made the filter
    /// fall through to raw progress lines and report a successful render as
    /// the failure.
    #[test]
    fn surfaces_a_model_not_found_error() {
        let lines = owned(&[
            "[Server] Job abc created (LM, mode=0, N=1)",
            "[LM-Phase1] Prefill 140ms, 71 tokens",
            "[Server] LM not found: no-such-model-Q8_0.gguf",
        ]);
        let out = backend_diagnostic(&lines);
        assert!(out.contains("LM not found"), "got: {out}");
        assert!(!out.contains("Prefill"), "progress leaked into the error: {out}");
    }

    /// Several warnings are routine fallbacks, not failures. They must not be
    /// dressed up as the cause, but they are still worth showing as context.
    #[test]
    fn does_not_pass_routine_warnings_off_as_the_cause() {
        let lines = owned(&[
            "[LM] WARNING: failed to allocate partial head buffer, using full vocab",
            "[Server] WARNING: failed to decode ref_audio, ignoring",
        ]);
        let out = backend_diagnostic(&lines);
        assert!(
            out.starts_with("no explicit error from the backend"),
            "warnings were presented as the failure: {out}"
        );
        assert!(out.contains("partial head buffer"), "context was dropped: {out}");
    }

    #[test]
    fn reports_when_the_backend_was_silent() {
        assert!(backend_diagnostic(&[]).contains("no reason given"));
    }

    /// A long planner caption must not flood the toast.
    #[test]
    fn clips_long_lines() {
        let long = format!("[Server] ERROR: {}", "x".repeat(400));
        let out = backend_diagnostic(&[long]);
        assert!(out.len() < 220, "len was {}", out.len());
        assert!(out.ends_with("..."), "got: {out}");
    }
}


#[cfg(test)]
mod lrc_sanitise_tests {
    use super::sanitise_lrc;

    #[test]
    fn duplicated_timestamps_are_pushed_apart() {
        // Two rows landing on the same frame is what makes a karaoke display
        // flicker: both are "current" at once and neither is ever readable.
        let out = sanitise_lrc("[00:04.00]first\n[00:04.00]second\n[00:09.00]third\n");
        let ts: Vec<f32> = out
            .lines()
            .map(|l| {
                let (m, s) = l[1..l.find(']').unwrap()].split_once(':').unwrap();
                m.parse::<f32>().unwrap() * 60.0 + s.parse::<f32>().unwrap()
            })
            .collect();
        assert!(ts.windows(2).all(|w| w[1] > w[0]), "not strictly increasing: {ts:?}");
    }

    #[test]
    fn ordering_and_text_are_preserved() {
        let out = sanitise_lrc("[00:01.00]hello there\n[00:05.00]second line\n");
        assert!(out.contains("]hello there"), "text lost: {out}");
        assert!(out.contains("]second line"), "text lost: {out}");
        assert!(out.find("hello") < out.find("second"), "order changed: {out}");
    }

    #[test]
    fn lines_without_a_timestamp_pass_through() {
        let out = sanitise_lrc("no stamp here\n[00:02.00]real\n");
        assert!(out.contains("no stamp here"));
        assert!(out.contains("]real"));
    }

    #[test]
    fn an_empty_input_stays_empty() {
        assert_eq!(sanitise_lrc(""), "");
    }
}

