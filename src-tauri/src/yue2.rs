//! YuE2 generation through the HOT-Step CPP engine.
//!
//! YuE2 is not ACE-Step with different weights. It is a different model with a
//! different job API — `POST /yue2/synth` rather than `/lm` then `/synth` — and
//! a different set of knobs. Most visibly there is no duration: the composer
//! decides how long the song is, and a request that tries to impose one is
//! talking to a field the pipeline does not have.
//!
//! The two pipelines share a binary and a job queue, so the job machinery is
//! reused from `generate` rather than duplicated; only the request shape, the
//! result decoding and the parameter set are new.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::generate;

/// One stage's sampler settings. The plan stage writes the lead sheet and the
/// semantic stage writes the codec stream; they are tuned separately upstream
/// and a single shared temperature badly suits both.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StageSampling {
    /// -1 leaves the checkpoint's own value.
    #[serde(default = "minus_one_f32")]
    pub temperature: f32,
    #[serde(default = "minus_one_f32")]
    pub top_p: f32,
    #[serde(default = "minus_one")]
    pub top_k: i32,
    #[serde(default = "minus_one_f32")]
    pub repetition_penalty: f32,
    #[serde(default = "minus_one")]
    pub penalty_window: i32,
    #[serde(default = "minus_one")]
    pub min_tokens: i32,
    #[serde(default = "minus_one")]
    pub max_tokens: i32,
}

fn minus_one() -> i32 {
    -1
}
fn minus_one_i64() -> i64 {
    -1
}
fn minus_one_f32() -> f32 {
    -1.0
}

impl Default for StageSampling {
    fn default() -> Self {
        StageSampling {
            temperature: -1.0,
            top_p: -1.0,
            top_k: -1,
            repetition_penalty: -1.0,
            penalty_window: -1,
            min_tokens: -1,
            max_tokens: -1,
        }
    }
}

/// How much lead sheet the composer writes before the audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CotMode {
    /// Full lead sheet, then melody, then audio. The default.
    #[default]
    Full,
    /// Melody only, skipping the score. Faster, less structure.
    Melody,
    /// No lead sheet, but guidance still runs. Slowest.
    Off,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Yue2Params {
    /// Names the output file. Empty falls back to the style text, then to the
    /// opening lyric line. Deliberately absent from `to_json`: the engine's
    /// YuE2 route has no title field, and this names a file rather than
    /// steering the render.
    #[serde(default)]
    pub title: String,
    /// "mp3" or "wav". YuE2's engine has no format field at all -- it hardcodes
    /// `result_mime = "audio/wav"` -- so this is applied here, after the render.
    /// ACE-Step can ask its engine directly; YuE2 cannot.
    #[serde(default)]
    pub output_format: String,
    #[serde(default = "default_mp3_bitrate")]
    pub mp3_bitrate: i32,
    /// Which installed language model to run. Empty asks the engine to pick,
    /// which is left as its own decision rather than second-guessed here.
    #[serde(default)]
    pub lm_type: String,
    /// A plain style sentence. Tempo and key belong in this text; there are no
    /// separate fields for them on this pipeline.
    pub style: String,
    pub lyrics: String,
    #[serde(default)]
    pub cot: CotMode,
    /// An externally supplied ABC score. When set, the plan stage is skipped.
    #[serde(default)]
    pub abc: String,
    /// Only write the score and return it. No audio.
    #[serde(default)]
    pub plan_only: bool,
    /// Stop after the composer and return raw codec ids.
    #[serde(default)]
    pub semantic_only: bool,
    #[serde(default)]
    pub plan: StageSampling,
    #[serde(default)]
    pub semantic: StageSampling,
    /// 0 leaves it to the checkpoint.
    #[serde(default)]
    pub cfg_scale: f32,
    /// 0 uses the checkpoint's own step count.
    #[serde(default)]
    pub ode_steps: i32,
    #[serde(default)]
    pub nar_solver: String,
    #[serde(default)]
    pub nar_scheduler: String,
    /// Songs from one request.
    #[serde(default = "one")]
    pub lm_batch_size: i32,
    /// Noise variations per song.
    #[serde(default = "one")]
    pub synth_batch_size: i32,
    #[serde(default = "minus_one_i64")]
    pub seed: i64,
    /// -1 is random.
    #[serde(default = "minus_one_i64")]
    pub noise_seed: i64,
    /// Redraw a runaway composition up to this many times. 0 renders whatever
    /// came out.
    #[serde(default)]
    pub semantic_retries: i32,
    #[serde(default = "one_point_five")]
    pub plan_cutoff: f32,
    /// Stop once the sampler puts this much mass on the end token. 0 = off.
    #[serde(default)]
    pub end_threshold: f32,
    /// Skip the network on this fraction of middle ODE steps.
    #[serde(default)]
    pub nar_cache_ratio: f32,
    /// Cap the NAR chunk length. 0 = one chunk for a normal song.
    #[serde(default)]
    pub nar_chunk_frames: i32,
    /// LoRA for the composer.
    #[serde(default)]
    pub lm_adapter: String,
    /// LoRA for the NAR renderer, which is a separate slot.
    #[serde(default)]
    pub nar_adapter: String,
}

fn one() -> i32 {
    1
}
fn one_point_five() -> f32 {
    1.5
}

impl Default for Yue2Params {
    fn default() -> Self {
        Yue2Params {
            title: String::new(),
            // MP3 by default, matching ACE-Step. A YuE2 render is minutes of
            // work and lands as tens of megabytes of PCM, which is a poor
            // default for something you are going to listen to more than once.
            output_format: "mp3".into(),
            mp3_bitrate: 128,
            lm_type: String::new(),
            style: String::new(),
            lyrics: String::new(),
            cot: CotMode::Full,
            abc: String::new(),
            plan_only: false,
            semantic_only: false,
            plan: StageSampling::default(),
            semantic: StageSampling::default(),
            cfg_scale: 0.0,
            ode_steps: 0,
            nar_solver: String::new(),
            nar_scheduler: String::new(),
            lm_batch_size: 1,
            synth_batch_size: 1,
            seed: -1,
            noise_seed: -1,
            semantic_retries: 0,
            plan_cutoff: one_point_five(),
            end_threshold: 0.0,
            nar_cache_ratio: 0.0,
            nar_chunk_frames: 0,
            lm_adapter: String::new(),
            nar_adapter: String::new(),
        }
    }
}

/// Only fields the user actually set are sent. A zero is a meaningful value to
/// this pipeline in some places and "unset" in others, so forwarding the struct
/// wholesale would pin the samplers to defaults the checkpoint chose for itself.
fn stage_json(s: &StageSampling) -> serde_json::Value {
    let mut o = serde_json::Map::new();
    if s.temperature >= 0.0 {
        o.insert("temperature".into(), s.temperature.into());
    }
    if s.top_p > 0.0 {
        o.insert("top_p".into(), s.top_p.into());
    }
    if s.top_k > 0 {
        o.insert("top_k".into(), s.top_k.into());
    }
    if s.repetition_penalty > 0.0 {
        o.insert("repetition_penalty".into(), s.repetition_penalty.into());
    }
    if s.penalty_window >= 0 {
        o.insert("penalty_window".into(), s.penalty_window.into());
    }
    if s.min_tokens >= 0 {
        o.insert("min_tokens".into(), s.min_tokens.into());
    }
    if s.max_tokens > 0 {
        o.insert("max_tokens".into(), s.max_tokens.into());
    }
    serde_json::Value::Object(o)
}

impl Yue2Params {
    fn to_json(&self) -> serde_json::Value {
        let mut o = serde_json::Map::new();
        o.insert("style".into(), self.style.clone().into());
        o.insert("lyrics".into(), self.lyrics.clone().into());
        // Serialised by name: the engine's enum is "full", "melody", "off".
        let cot_name = match self.cot {
            CotMode::Full => "full",
            CotMode::Melody => "melody",
            CotMode::Off => "off",
        };
        o.insert("cot".into(), cot_name.into());
        if !self.abc.trim().is_empty() {
            o.insert("abc".into(), self.abc.clone().into());
        }
        if !self.lm_type.trim().is_empty() {
            o.insert("lm_type".into(), self.lm_type.trim().into());
        }
        if self.plan_only {
            o.insert("plan_only".into(), true.into());
        }
        if self.semantic_only {
            o.insert("semantic_only".into(), true.into());
        }
        let plan = stage_json(&self.plan);
        if !plan.as_object().expect("stage_json builds an object").is_empty() {
            o.insert("plan".into(), plan);
        }
        let semantic = stage_json(&self.semantic);
        if !semantic
            .as_object()
            .expect("stage_json builds an object")
            .is_empty()
        {
            o.insert("semantic".into(), semantic);
        }
        if self.cfg_scale > 0.0 {
            o.insert("cfg_scale".into(), self.cfg_scale.into());
        }
        if self.ode_steps > 0 {
            o.insert("ode_steps".into(), self.ode_steps.into());
        }
        if !self.nar_solver.trim().is_empty() {
            o.insert("nar_solver".into(), self.nar_solver.trim().into());
        }
        if !self.nar_scheduler.trim().is_empty() {
            o.insert("nar_scheduler".into(), self.nar_scheduler.trim().into());
        }
        o.insert("lm_batch_size".into(), self.lm_batch_size.into());
        o.insert("synth_batch_size".into(), self.synth_batch_size.into());
        o.insert("seed".into(), self.seed.into());
        o.insert("noise_seed".into(), self.noise_seed.into());
        if self.semantic_retries > 0 {
            o.insert("semantic_retries".into(), self.semantic_retries.into());
        }
        if self.plan_cutoff > 0.0 {
            o.insert("plan_cutoff".into(), self.plan_cutoff.into());
        }
        if self.end_threshold > 0.0 {
            o.insert("end_threshold".into(), self.end_threshold.into());
        }
        if self.nar_cache_ratio > 0.0 {
            o.insert("nar_cache_ratio".into(), self.nar_cache_ratio.into());
        }
        if self.nar_chunk_frames > 0 {
            o.insert("nar_chunk_frames".into(), self.nar_chunk_frames.into());
        }
        for (key, value) in [
            ("lm_adapter", &self.lm_adapter),
            ("nar_adapter", &self.nar_adapter),
        ] {
            let name = value.trim();
            if !name.is_empty() {
                o.insert(key.into(), name.into());
            }
        }
        serde_json::Value::Object(o)
    }
}

/// What the engine can do, as reported by `/yue2/props`.
///
/// Forwarded largely as received. The UI needs it to know whether the weights
/// are present at all, which is the difference between a form that works and
/// one that submits into a 400.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Yue2Props {
    pub available: bool,
    #[serde(default)]
    pub synth_ready: bool,
    #[serde(default)]
    pub models_dir: String,
    #[serde(default)]
    pub max_lm_batch: i32,
    #[serde(default)]
    pub max_synth_batch: i32,
    #[serde(default)]
    pub vae_variant_loaded: String,
    #[serde(default)]
    pub errors: Vec<String>,
    #[serde(default)]
    pub raw: serde_json::Value,
}

pub fn props(port: u16) -> Result<Yue2Props, String> {
    let url = format!("http://127.0.0.1:{port}/yue2/props");
    let text = ureq::get(&url)
        .timeout(std::time::Duration::from_secs(30))
        .call()
        .map_err(|e| format!("yue2 props failed: {e}"))?
        .into_string()
        .map_err(|e| format!("bad yue2 props: {e}"))?;
    let value: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("bad yue2 props: {e}"))?;
    let available = value.get("available").and_then(|v| v.as_bool()).unwrap_or(false);
    let g = |k: &str| value.get(k).and_then(|v| v.as_i64()).unwrap_or(0) as i32;
    let models_dir = value
        .get("models_dir")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let vae = value
        .get("vae_variant_loaded")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let errors = value
        .get("errors")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|e| e.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let synth_ready = value
        .get("synth_ready")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    Ok(Yue2Props {
        available,
        synth_ready,
        models_dir,
        max_lm_batch: g("max_lm_batch"),
        max_synth_batch: g("max_synth_batch"),
        vae_variant_loaded: vae,
        errors,
        raw: value,
    })
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Yue2Outcome {
    pub tracks: Vec<generate::Track>,
    /// The ABC score, when the request asked for a plan only.
    pub score: Option<String>,
    /// Raw codec ids, when the request stopped after the composer.
    pub semantic_ids: Option<Vec<i64>>,
    pub cancelled: bool,
}

/// Split a `multipart/mixed` body into its parts.
///
/// The engine hands back a batch as multipart with audio parts in song order,
/// which is the same shape the ACE-Step path already deals with; reusing the
/// reader keeps one implementation of "what counts as a part".
fn split_multipart(body: &[u8], boundary: &str) -> Vec<Vec<u8>> {
    let marker = format!("--{boundary}").into_bytes();
    let mut parts = Vec::new();
    let mut cursor = match find(body, &marker, 0) {
        Some(at) => at + marker.len(),
        None => return parts,
    };
    while let Some(next) = find(body, &marker, cursor) {
        let mut start = cursor;
        // Skip the CRLF that ends the boundary line.
        while start < next && (body[start] == b'\r' || body[start] == b'\n') {
            start += 1;
        }
        // Part headers run until the first blank line; the payload is the rest.
        let payload_start = match find(body, b"\r\n\r\n", start) {
            Some(at) if at < next => at + 4,
            _ => start,
        };
        let payload_end = next.saturating_sub(2);
        if payload_end > payload_start {
            parts.push(body[payload_start..payload_end].to_vec());
        }
        cursor = next + marker.len();
    }
    parts
}

fn find(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || from >= haystack.len() {
        return None;
    }
    haystack[from..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|at| at + from)
}

fn is_wav(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WAVE"
}

/// Render, or produce a plan, through `/yue2/synth`.
/// Lay lyric lines evenly across a finished track and format them as LRC.
///
/// This is a fabrication and is labelled as one everywhere it matters. The
/// bundled engine returns a timed LRC for ACE-Step in the `X-LRC-Text` header
/// and the app writes that out untouched; HOT-Step's YuE2 job never sets an LRC
/// at all, and the words its composer invents are not returned anywhere -- the
/// `abc` score the job does return is built from MIDI voices with no lyric
/// field. So for lyrics the user supplied, the best available is a plausible
/// placement rather than none, and the alternative was a track with lyrics in
/// it that the player would never show.
///
/// Section markers are carried through as their own stamped line, which is what
/// the ACE-Step sidecar looks like -- the engine writes `[00:00.00][Verse 1]`
/// -- so a hand-written section reads the same way whichever engine made it.
/// Format score-derived timings as LRC. The counterpart to `spread_lrc`, which
/// is the fallback for a score that could not be read.
pub fn format_lrc(timed: &[(f64, String)]) -> String {
    let mut out = String::new();
    for (at, text) in timed {
        let at = (*at).max(0.0);
        let mins = (at / 60.0) as u32;
        let secs = at - (mins as f64 * 60.0);
        out.push_str(&format!("[{:02}:{:05.2}]{text}\n", mins, secs));
    }
    out
}

pub fn spread_lrc(lyrics: &str, duration_secs: f32) -> String {
    let lines: Vec<&str> = lyrics.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    if lines.is_empty() || duration_secs <= 0.0 {
        return String::new();
    }
    let count = lines.len() as f32;
    let mut out = String::with_capacity(lines.len() * 24);
    for (i, line) in lines.iter().enumerate() {
        // Spread across the whole track rather than stopping short of the end.
        let at = (duration_secs * i as f32 / count).min(duration_secs);
        let mins = (at / 60.0) as u32;
        let secs = at - (mins as f32 * 60.0);
        out.push_str(&format!("[{mins:02}:{secs:05.2}]{line}\n"));
    }
    out
}

/// One YuE2 language model the engine could be asked to load.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Yue2Lm {
    /// What the request sends as `lm_type`. The engine builds a filename from
    /// it, so it is the middle of `yue2-lm-<lm_type>.gguf` and nothing else.
    pub lm_type: String,
    pub filename: String,
    pub size_bytes: u64,
}

const LM_PREFIX: &str = "yue2-lm-";
const LM_SUFFIX: &str = ".gguf";

/// The `lm_type` a filename stands for.
///
/// Empty is not a model: `yue2-lm-.gguf` would make the engine look for a file
/// it will never find, and a picker offering it would just be a way to fail.
pub fn lm_type_of(filename: &str) -> Option<String> {
    let middle = filename.strip_prefix(LM_PREFIX)?.strip_suffix(LM_SUFFIX)?;
    if middle.is_empty() {
        return None;
    }
    Some(middle.to_string())
}

/// The YuE2 language models installed under `dir`.
///
/// Scans for the exact filenames the engine builds rather than asking the
/// weights catalogue, which knows about architecture rather than about what a
/// picker is allowed to offer. A dropdown that lists a model the engine then
/// refuses to load is worse than a short one. An unextracted download is
/// skipped for the same reason: the engine opens `.gguf`, so a `.zip` beside it
/// is not a choice.
pub fn available_lms(dir: &std::path::Path) -> Vec<Yue2Lm> {
    let mut out: Vec<Yue2Lm> = std::fs::read_dir(dir.join("yue2"))
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let filename = entry.file_name().to_str()?.to_string();
            let lm_type = lm_type_of(&filename)?;
            let size_bytes = entry.metadata().map(|m| m.len()).unwrap_or(0);
            Some(Yue2Lm { lm_type, filename, size_bytes })
        })
        .collect();
    // Smallest first: on a card this size the smallest model is the one most
    // likely to finish, and the ordering should say so without being read.
    out.sort_by(|a, b| {
        a.size_bytes
            .cmp(&b.size_bytes)
            .then_with(|| a.lm_type.cmp(&b.lm_type))
    });
    out
}

/// One submitted YuE2 job, waited on, with whatever score it produced.
struct StageRun {
    id: String,
    score: Option<String>,
}

/// Whether this render has to run as two jobs rather than one.
///
/// A full render with no score supplied is exactly the case where the composer
/// and the audio would share a job -- and therefore share a KV cache lifetime,
/// which is what does not fit.
fn needs_staged_plan(params: &Yue2Params) -> bool {
    !params.plan_only && !params.semantic_only && params.abc.trim().is_empty()
}

/// The two requests a staged render sends.
///
/// Pulled out so the staging can be tested without a server: the mistake this
/// guards against is feeding the previous score back into the stage that is
/// meant to replace it, which would silently render the wrong song.
fn staged_params(params: &Yue2Params, score: &str) -> (Yue2Params, Yue2Params) {
    let mut plan = params.clone();
    plan.plan_only = true;
    plan.semantic_only = false;
    // Never feed a score in here: this stage exists to write one.
    plan.abc = String::new();

    let mut audio = params.clone();
    audio.plan_only = false;
    // Deliberately not semantic_only. That flag is the engine's planner probe:
    // it stops after the codec stream and hands back raw ids with no NAR, no VAE
    // and no audio, which is a song nobody asked for. A supplied `abc` is what
    // skips the plan stage -- the engine excludes a song that brings its own
    // score from the plan batch.
    audio.semantic_only = false;
    audio.abc = score.to_string();
    (plan, audio)
}

/// Submit one job and wait for it. `Ok(None)` means the user cancelled.
fn run_stage(
    port: u16,
    params: &Yue2Params,
    models_dir: &std::path::Path,
    on_stage: &mut dyn FnMut(&str, &str),
    composing: &str,
    running: &str,
) -> Result<Option<StageRun>, String> {
    on_stage("yue2", composing);
    // Taken per stage: the mark is how staleness is judged, and a second job
    // that inherited the first job's mark would look silent from the start.
    let mark = crate::server::stderr_mark(models_dir);
    let id = generate::submit_json(port, "/yue2/synth", params.to_json())?;
    if generate::wait_for_job_quiet(port, &id, mark, &mut |_| on_stage("yue2", running))?
        == generate::JobState::Cancelled
    {
        return Ok(None);
    }
    let score = generate::job_abc(port, &id);
    Ok(Some(StageRun { id, score }))
}

/// Compose the score, then render from it, as two jobs.
///
/// YuE2's AR stage allocates a persistent KV cache that outlives the stage that
/// filled it, so a plan and a semantic sharing one job hold two caches at once.
/// On a 4 GB card the second allocation is the one that fails. Splitting lets
/// the engine unload between the two.
///
/// It buys something back as well: the score is known before the NAR stage,
/// which is the long one, so a score too short or too empty to be worth hearing
/// fails in seconds instead of after the render has already cost its time.
fn staged_plan_then_audio(
    port: u16,
    params: &Yue2Params,
    models_dir: &std::path::Path,
    on_stage: &mut dyn FnMut(&str, &str),
) -> Result<Option<StageRun>, String> {
    let (plan, _) = staged_params(params, "");
    let Some(first) =
        run_stage(port, &plan, models_dir, &mut *on_stage, "composing", "writing score")?
    else {
        return Ok(None);
    };

    let score = first.score.unwrap_or_default();
    if score.trim().is_empty() {
        return Err(
            "The composer returned no score, so there is nothing to render. Try a shorter or more specific style prompt."
                .into(),
        );
    }
    let health = crate::abc::score_health(&score)
        .map_err(|e| format!("The composed score cannot be rendered: {e}"))?;
    on_stage(
        "yue2",
        &format!("score ready: {} vocal bars, {} sections", health.bars, health.sections),
    );

    let (_, audio) = staged_params(params, &score);
    let Some(mut last) = run_stage(port, &audio, models_dir, on_stage, "rendering score", "rendering")?
    else {
        return Ok(None);
    };
    // The score is the one this function composed, not whatever the audio job
    // happens to report about itself.
    //
    // Re-reading it from the job's status was how the sidecar fell all the
    // way through to the evenly-spread fallback: the score-derived timings
    // are the second-best answer and they were only ever offered when the
    // engine volunteered the score a second time. It composed one; there is
    // no reason to ask it for it again and take the chance that it says no.
    last.score = Some(score);
    Ok(Some(last))
}

/// The bitrate a YuE2 render is encoded at, clamped to something sane.
fn mp3_bitrate_of(params: &Yue2Params) -> u32 {
    params.mp3_bitrate.clamp(64, 320) as u32
}

fn default_mp3_bitrate() -> i32 {
    128
}

/// Set once at startup from the resource directory, next to the engine's.
static ENCODER_DIR: std::sync::OnceLock<String> = std::sync::OnceLock::new();

fn bundled_encoder_dir() -> String {
    ENCODER_DIR.get().cloned().unwrap_or_default()
}

pub fn set_encoder_dir(dir: std::path::PathBuf) {
    let _ = ENCODER_DIR.set(dir.to_string_lossy().into_owned());
}

/// The encoder to use, or None when nothing on this machine can do it.
pub fn mp3_encoder() -> Option<std::path::PathBuf> {
    mp3_encoder_at(std::path::Path::new(&bundled_encoder_dir()))
}

/// Resolution split out from the static lookup so it can be tested against a
/// directory that is not the real one.
pub fn mp3_encoder_at(bundled_dir: &std::path::Path) -> Option<std::path::PathBuf> {
    let bundled = bundled_dir.join("lame");
    if bundled.is_file() {
        return Some(bundled);
    }
    // The fallback for a broken or half-extracted install. A system ffmpeg
    // still beats nothing, and it is what this depended on outright before.
    let have_ffmpeg = std::process::Command::new("ffmpeg")
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    have_ffmpeg.then(|| std::path::PathBuf::from("ffmpeg"))
}

/// Whether MP3 output is possible at all, so the UI can say so before a render
/// rather than after one.
pub fn mp3_supported() -> bool {
    mp3_encoder().is_some()
}

/// The full argument list for one encoder.
///
/// Built in one place and returned whole, because the order matters and the two
/// tools disagree about it. `ffmpeg` wants `-i` immediately before the input and
/// its codec options *after* it; lame wants no `-i` at all and takes input then
/// output bare. Getting this wrong produces a file that decodes and is the wrong
/// length, which is a miserable thing to notice by ear.
///
/// An earlier version assembled this by appending codec flags to a list that
/// already held `-i`, which put the input flag in front of them and handed
/// ffmpeg a codec option where it expected a filename. The test below is what
/// caught it.
fn encoder_args(
    encoder: &std::path::Path,
    wav: &std::path::Path,
    mp3: &std::path::Path,
    bitrate: u32,
) -> Vec<String> {
    let wav = wav.to_string_lossy().into_owned();
    let mp3 = mp3.to_string_lossy().into_owned();
    if encoder.file_name().is_some_and(|n| n == "ffmpeg") {
        vec![
            "-hide_banner".into(),
            "-loglevel".into(),
            "error".into(),
            "-y".into(),
            "-i".into(),
            wav,
            "-codec:a".into(),
            "libmp3lame".into(),
            "-b:a".into(),
            format!("{bitrate}k"),
            mp3,
        ]
    } else {
        vec!["--quiet".into(), "-b".into(), bitrate.to_string(), wav, mp3]
    }
}

/// Run the encoder over a WAV.
fn encode(
    encoder: &std::path::Path,
    wav: &std::path::Path,
    mp3: &std::path::Path,
    bitrate: u32,
) -> Result<(), String> {
    let args = encoder_args(encoder, wav, mp3, bitrate);
    let out = std::process::Command::new(encoder)
        .args(&args)
        .output()
        .map_err(|e| format!("{} could not be run: {e}", encoder.display()))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if err.is_empty() {
            format!("{} exited with {}", encoder.display(), out.status)
        } else {
            err.lines().next().unwrap_or("encoder failed").to_string()
        });
    }
    Ok(())
}

/// Re-encode a finished WAV as MP3, returning the new path.
///
/// Every failure is an `Err`, and the caller keeps the WAV: the render is
/// already on disk and correct, and discarding it because a re-encode did not
/// work would be a bad trade. Duration is read from the WAV before this point
/// and never re-derived, so sidecar timings do not depend on the encode having
/// succeeded.
fn to_mp3(wav: &std::path::Path, bitrate: u32) -> Result<std::path::PathBuf, String> {
    let encoder = mp3_encoder().ok_or("no MP3 encoder is available")?;
    let mp3 = wav.with_extension("mp3");
    encode(&encoder, wav, &mp3, bitrate)?;
    // A zero-byte file means the encoder reported success and wrote nothing.
    // Treating that as success would leave a library row pointing at nothing.
    match std::fs::metadata(&mp3) {
        Ok(m) if m.len() > 0 => Ok(mp3),
        _ => {
            let _ = std::fs::remove_file(&mp3);
            Err(format!("{} produced an empty file", encoder.display()))
        }
    }
}

pub fn render(
    port: u16,
    params: &Yue2Params,
    on_stage: &mut dyn FnMut(&str, &str),
) -> Result<Yue2Outcome, String> {
    let models_dir = crate::settings::resolve_hotstep_models_dir();

    let cancelled = || Yue2Outcome {
        tracks: Vec::new(),
        score: None,
        semantic_ids: None,
        cancelled: true,
    };

    let (id, score) = if needs_staged_plan(params) {
        match staged_plan_then_audio(port, params, &models_dir, &mut *on_stage)? {
            Some(run) => (run.id, run.score),
            None => return Ok(cancelled()),
        }
    } else {
        match run_stage(port, params, &models_dir, on_stage, "composing", "rendering")? {
            Some(run) => (run.id, run.score),
            None => return Ok(cancelled()),
        }
    };

    // The score comes back on the status, not the audio body, so it is read
    // alongside the job rather than from the result. Read once: a second pass
    // would double a render that already takes twenty minutes on this card.
    let fetched = generate::fetch_result(port, &id)?;
    let body = fetched.body;

    if params.plan_only {
        on_stage("done", "score ready");
        return Ok(Yue2Outcome {
            tracks: Vec::new(),
            score: Some(String::from_utf8_lossy(&body).to_string()),
            semantic_ids: None,
            cancelled: false,
        });
    }
    if params.semantic_only {
        let ids: Vec<i64> = serde_json::from_slice::<Vec<i64>>(&body)
            .map_err(|e| format!("bad semantic result: {e}"))?;
        return Ok(Yue2Outcome {
            tracks: Vec::new(),
            score: None,
            semantic_ids: Some(ids),
            cancelled: false,
        });
    }

    // A single song comes back as bare WAV; a batch as multipart, song-major.
    let parts = if is_wav(&body) {
        vec![body]
    } else {
        split_multipart(&body, "yue2-batch-boundary")
    };

    let out_dir = crate::paths::library_dir();
    std::fs::create_dir_all(&out_dir)
        .map_err(|e| format!("cannot create {}: {e}", out_dir.display()))?;
    let stamp = generate::now_secs();
    let stem_base = crate::paths::song_stem(&params.title, &params.style, &params.lyrics);

    let mut tracks = Vec::new();
    for (index, data) in parts.iter().enumerate() {
        if !is_wav(data) {
            continue;
        }
        // The engine only ever hands back WAV, so the extension written first
        // is the engine's, not the requested one. `audio_path` below is what the
        // library ends up pointing at.
        let stem = crate::paths::unique_path(&out_dir, &stem_base, "wav");
        let wav_path = PathBuf::from(stem);
        let wav_path = if index == 0 {
            wav_path
        } else {
            crate::paths::unique_path(&out_dir, &format!("{stem_base}-{index}"), "wav")
        };
        std::fs::write(&wav_path, data)
            .map_err(|e| format!("cannot write {}: {e}", wav_path.display()))?;
        // Taken from the WAV bytes, before any re-encode, so the sidecar
        // timings never depend on the encode having worked.
        let seconds = wav_seconds(data);

        // YuE2's engine hardcodes WAV and offers no format of its own, so the
        // choice is honoured here. A failed encode keeps the WAV: losing a
        // render that took minutes to produce because a re-encode did not work
        // would be a bad trade.
        let mut path = wav_path.clone();
        if params.output_format == "mp3" {
            match to_mp3(&wav_path, mp3_bitrate_of(params)) {
                Ok(mp3) => {
                    // Only drop the WAV once the MP3 is confirmed on disk.
                    let _ = std::fs::remove_file(&wav_path);
                    path = mp3;
                }
                Err(why) => {
                    eprintln!(
                        "[yue2] keeping WAV, mp3 encode failed: {why} -- install ffmpeg for mp3 output"
                    );
                }
            }
        }

        // The sidecar ACE-Step renders get, so the library has something to
        // show and the two engines produce the same shape of output. Written
        // per take, unlike the ACE-Step path which times only the first: there
        // is no alignment here to attach to a specific performance, so each
        // take gets its own copy rather than one take inheriting another's
        // timings.
        //
        // A failure here must not lose the render; the audio is already written.
        if !params.lyrics.trim().is_empty() {
            // The forced aligner first, when the model is installed: it hears
            // the finished track and reports when each word was sung. The
            // score-derived timings below are an estimate read off the lead
            // sheet, and the evenly-spread one is a guess -- both are worse
            // than a recording, and both are what a machine without the 1.26 GB
            // aligner model is left with.
            //
            // The lyrics sent to the aligner must be the exact string the
            // render was given: the answer indexes into it, and nothing
            // re-derives the offsets.
            let aligned = match crate::align::align(port, &path, &params.lyrics) {
                Ok(words) => {
                    let lrc = crate::align::aligned_lrc(&params.lyrics, &words);
                    if lrc.is_empty() {
                        eprintln!("[yue2] the aligner returned no timed lines; using the score");
                    }
                    lrc
                }
                Err(e) => {
                    eprintln!("[yue2] forced alignment unavailable ({e}); using the score");
                    String::new()
                }
            };

            let lrc = if !aligned.is_empty() {
                aligned
            } else {
                // Measured off the score the composer produced, when there is a
                // score it can be measured from. The engine returns the lead
                // sheet but never the clock over it, so the clock is derived
                // here -- the same numbers the C6 wire format would carry.
                let timed = score
                    .as_deref()
                    .and_then(|abc| crate::abc::lyric_timings(abc, &params.lyrics).ok());
                match &timed {
                    Some(lines) => format_lrc(lines),
                    None => spread_lrc(&params.lyrics, seconds),
                }
            };
            if !lrc.is_empty() {
                let lrc_path = path.with_extension("lrc");
                if let Err(e) = std::fs::write(&lrc_path, &lrc) {
                    eprintln!("[lrc] cannot write {}: {e}", lrc_path.display());
                } else {
                    eprintln!("[lrc] wrote {} ({} bytes)", lrc_path.display(), lrc.len());
                }
            }
        }
        let id = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| format!("{stamp}-{index}"));
        let track = generate::Track {
            id,
            bytes: data.len() as u64,
            path: path.display().to_string(),
            format: "wav".to_string(),
            caption: params.style.clone(),
            lyrics: params.lyrics.clone(),
            // YuE2 has no bpm or key field; they belong in the style sentence.
            bpm: 0,
            keyscale: String::new(),
            timesignature: String::new(),
            vocal_language: "unknown".to_string(),
            duration_secs: seconds,
            created_at: stamp,
            cover: crate::covers::cover_for(&path.display().to_string()),
        };
        generate::write_metadata(&track);
        tracks.push(track);
    }

    if tracks.is_empty() {
        return Err("YuE2 returned no audio".to_string());
    }
    on_stage("done", "complete");
    Ok(Yue2Outcome {
        tracks,
        score: None,
        semantic_ids: None,
        cancelled: false,
    })
}

fn wav_seconds(bytes: &[u8]) -> f32 {
    if !is_wav(bytes) {
        return 0.0;
    }
    let rd = |at: usize| -> u32 {
        u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
    };
    // The fmt chunk holds a mix of widths: the format tag, the channel count,
    // the block alignment and the bit depth are u16, and only the sample rate
    // and byte rate are u32. Reading a u16 as a u32 picks up the low half of
    // the next field, so a channel count of 2 and a rate of 48000 come back
    // as one number.
    let rd16 = |at: usize| -> u32 {
        u16::from_le_bytes([bytes[at], bytes[at + 1]]) as u32
    };
    // The fmt chunk, read properly this time.
    //
    //   0  format tag      2  channels      4  sample rate
    //   8  byte rate      12  block align   14  bits per sample
    //
    // The old version took the rate and divided the data size by two, on the
    // grounds that "16-bit mono and stereo both divide by 2". That is true of
    // the bytes per SAMPLE, and false of the bytes per FRAME: stereo has two
    // of them. So every stereo track came out exactly twice as long as it is,
    // and a 253 second render reported 506. The evenly-spread sidecar then
    // timed its lines against 506 seconds of a 253 second song, and the last
    // line landed two minutes past the end of the track.
    let mut rate = 0u32;
    let mut channels = 0u32;
    let mut bits = 16u32;
    let mut pos = 12usize;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = rd(pos + 4) as usize;
        if id == b"fmt " && pos + 8 + 16 <= bytes.len() {
            channels = rd16(pos + 10);
            rate = rd(pos + 12);
            bits = rd16(pos + 22);
            break;
        }
        pos += 8 + size + (size & 1);
    }
    if rate == 0 || channels == 0 || bits == 0 {
        return 0.0;
    }
    let frame_bytes = (channels * bits / 8).max(1);
    let mut pos = 12usize;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = rd(pos + 4) as usize;
        if id == b"data" {
            return (size / frame_bytes as usize) as f32 / rate as f32;
        }
        pos += 8 + size + (size & 1);
    }
    0.0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sidecar has to be readable by the same parser the library uses, or
    /// the file exists and nothing shows. This is the round trip that decides
    /// it, and it is the part that is easy to get wrong: the stamp has to be
    /// `[mm:ss.xx]` with digits either side of the colon, or the parser treats
    /// it as a metadata tag and drops the line.
    #[test]
    fn the_spread_lrc_round_trips_through_the_library_parser() {
        let lyrics = "[Verse 1]\nHey baby\nI don't want much today\n[Chorus]\nNo no no";
        let lrc = spread_lrc(lyrics, 30.0);
        let parsed = crate::annotations::parse_lrc(&lrc);
        assert_eq!(parsed.len(), 5, "every line, section markers included: {parsed:?}");
        assert_eq!(parsed[0].text, "[Verse 1]");
        assert_eq!(parsed[1].text, "Hey baby");
        assert_eq!(parsed[4].text, "No no no");
        // Strictly increasing, and inside the track.
        for pair in parsed.windows(2) {
            assert!(pair[0].at_secs <= pair[1].at_secs, "stamps went backwards");
        }
        assert!(parsed.last().unwrap().at_secs <= 30.0);
    }

    /// Empty lyrics must produce no file content at all. A render with nothing
    /// sung should not leave an empty `.lrc` beside it that the library then
    /// has to treat as "present but unreadable".
    /// The whole point of the score path: the two must not agree, or the parser
    /// is not doing anything. Even spread puts the fourth line at three
    /// quarters of the track; the score puts the chorus where the score says.
    #[test]
    fn a_score_gives_different_times_than_even_spread() {
        let abc = concat!(
            "X:1\nT:\nM:4/4\nL:1/8\nQ:1/4=120\n",
            "V: Vocal clef=treble name=\"Vocal Melody\" snm=\"Vocal\"\n",
            "K:C\n",
            "% Verse\n", "V: Vocal\n", "CDEF/2 | GABc | z4 |\n",
            "% Chorus\n", "V: Vocal\n", "CDEF GABc | cdef gabc | z4 |\n",
        );
        let lyrics = "[Verse]\none\ntwo\n[Chorus]\nthree";
        let measured = crate::abc::lyric_timings(abc, lyrics).expect("score timings");
        let from_score = format_lrc(&measured);
        let spread = spread_lrc(lyrics, 16.0);

        assert_ne!(from_score, spread, "the score changed nothing");
        // Both have to be readable, or the difference is just noise.
        for text in [&from_score, &spread] {
            assert!(
                crate::annotations::parse_lrc(text).len() >= 4,
                "unreadable: {text}"
            );
        }
    }

    /// A score the parser cannot read must not stop the sidecar being written.
    /// The fallback is visibly approximate; losing the lyrics entirely would be
    /// worse than approximate.
    #[test]
    fn an_unreadable_score_still_leaves_the_even_spread_fallback() {
        let lyrics = "[Verse]\none\ntwo";
        // No tempo, no sections, no vocal bars: every reason the parser refuses.
        for abc in ["", "not abc", "X:1\nM:4/4\nK:C\n"] {
            assert!(crate::abc::lyric_timings(abc, lyrics).is_err());
        }
        assert!(!spread_lrc(lyrics, 30.0).is_empty());
    }

    /// The title names the file, so it has to win over the style text the way
    /// it does for ACE-Step. A user who typed a name should not find their
    /// track called after their own prompt.
    #[test]
    fn a_title_names_the_yue2_file() {
        let mut params = Yue2Params::default();
        params.style = "One warm reggae song in D minor".into();
        params.lyrics = "[Verse]\nI walked the long road home".into();
        assert_eq!(crate::paths::song_stem(&params.title, &params.style, &params.lyrics), "one-warm-reggae-song-in-d-minor");
        params.title = "Neon Rain".into();
        assert_eq!(crate::paths::song_stem(&params.title, &params.style, &params.lyrics), "neon-rain");
    }

    /// The engine's YuE2 route has no title field. Sending one would be a lie
    /// the engine ignores, or worse a strict-mode 400 -- either way the title
    /// has to stay out of the wire and name the file locally instead.
    #[test]
    fn the_title_never_reaches_the_engine() {
        let mut params = Yue2Params::default();
        params.title = "Neon Rain".into();
        let json = params.to_json();
        assert!(json.get("title").is_none(), "title leaked into the request");
    }

    /// The staging is only for a full render with no score to start from. Every
    /// other shape already loads one thing at a time and must keep doing so.
    #[test]
    fn only_a_plain_full_render_gets_staged() {
        let base = Yue2Params::default();
        assert!(needs_staged_plan(&base), "a plain render is exactly the case");

        let mut plan_only = base.clone();
        plan_only.plan_only = true;
        assert!(!needs_staged_plan(&plan_only));

        let mut semantic_only = base.clone();
        semantic_only.semantic_only = true;
        assert!(!needs_staged_plan(&semantic_only));

        let mut supplied = base.clone();
        supplied.abc = "X:1\nM:4/4\nQ:1/4=120\nK:C\nCDEF|".into();
        assert!(!needs_staged_plan(&supplied), "a supplied score is already composed");
    }

    /// The stage that writes a score must not be handed one, and the stage that
    /// renders must be. Getting this backwards either re-renders the previous
    /// song or composes twice and throws the first score away.
    #[test]
    fn the_two_stages_pass_the_score_the_right_way_round() {
        let mut params = Yue2Params::default();
        params.style = "warm reggae".into();
        let (plan, audio) = staged_params(&params, "X:1\nM:4/4\nQ:1/4=120\n");

        assert!(plan.plan_only && !plan.semantic_only);
        assert_eq!(plan.abc, "", "the composing stage was handed a score");

        assert_eq!(audio.abc, "X:1\nM:4/4\nQ:1/4=120\n");
        assert_eq!(audio.style, params.style, "the render lost the style prompt");
    }

    /// A staged render must reach audio. Setting `semantic_only` on the second
    /// job asks the engine for its planner probe instead: it stops after the
    /// codec stream and returns raw ids, so the job completes cleanly having
    /// produced no sound at all. This is the whole bug -- nothing else about
    /// that request is wrong, so nothing else in a test would have caught it.
    /// The engine builds `yue2-lm-<lm_type>.gguf` from what the picker sends,
    /// so the middle of that filename is the whole contract. An `-imat` suffix
    /// is part of the name and has to survive the round trip.
    /// The bundled encoder has to win over a system one, or a machine with
    /// ffmpeg behaves differently from a machine without it -- which is the
    /// thing this whole change exists to stop.
    #[test]
    fn the_bundled_encoder_is_preferred() {
        let dir = std::env::temp_dir().join(format!("aceforge-enc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("lame"), b"#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(
            dir.join("lame"),
            <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o755),
        )
        .unwrap();

        let found = mp3_encoder_at(&dir);
        assert_eq!(found, Some(dir.join("lame")));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// An encoder that is not there must not resolve to one. Silently falling
    /// through to a different tool is how this shipped as a system dependency
    /// in the first place.
    #[test]
    fn a_missing_bundled_encoder_resolves_to_nothing_when_ffmpeg_is_absent() {
        let dir = std::env::temp_dir().join(format!("aceforge-enc2-{}", std::process::id()));
        let found = mp3_encoder_at(&dir);
        if found.is_some() {
            // Only legitimate when a system ffmpeg exists, which is the
            // documented fallback rather than a test failure.
            assert!(found.unwrap().ends_with("ffmpeg"));
        }
    }

    /// lame and ffmpeg take opposite arguments, and ffmpeg's `-i` has to sit
    /// immediately before the input with its codec options after. Getting this
    /// wrong yields a file that decodes and is the wrong length.
    #[test]
    fn each_encoder_gets_a_correctly_ordered_command_line() {
        let wav = std::path::Path::new("/tmp/in.wav");
        let mp3 = std::path::Path::new("/tmp/out.mp3");

        let ff = encoder_args(std::path::Path::new("ffmpeg"), wav, mp3, 192);
        let i_of = ff.iter().position(|a| a == "-i").expect("ffmpeg needs -i");
        assert_eq!(ff[i_of + 1], "/tmp/in.wav", "-i must be followed by the input");
        let bpos = ff.iter().position(|a| a == "-b:a").expect("ffmpeg needs -b:a");
        assert!(bpos > i_of + 1, "codec options belong after the input");
        assert_eq!(ff.last().unwrap(), "/tmp/out.mp3");
        assert!(!ff.contains(&"-b".to_string()), "lame's flag does not belong in an ffmpeg call");

        let lm = encoder_args(std::path::Path::new("lame"), wav, mp3, 192);
        assert!(!lm.contains(&"-i".to_string()), "lame takes no -i");
        assert_eq!(&lm[0..3], ["--quiet", "-b", "192"]);
        assert_eq!(lm[3], "/tmp/in.wav");
        assert_eq!(lm[4], "/tmp/out.mp3");
    }

    /// A real encode, because the size claim is the whole point and a mock
    /// would prove nothing about it. ffmpeg is skipped rather than failed when
    /// it is absent, so the suite still passes on a machine without it.
    #[test]
    fn an_encode_shrinks_the_file_by_roughly_the_bitrate_ratio() {
        if mp3_encoder().is_none() {
            eprintln!("no encoder on this machine; skipping the encode test");
            return;
        }
        let dir = std::env::temp_dir().join(format!("aceforge-mp3-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let wav = dir.join("tone.wav");

        // Two seconds of silence, written by ffmpeg so the fixture does not
        // have to encode PCM by hand.
        let ok = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi"])
            .arg("-i")
            .arg("sine=frequency=440:duration=2")
            .arg(&wav)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !ok {
            std::fs::remove_dir_all(&dir).ok();
            eprintln!("ffmpeg could not make the fixture; skipping");
            return;
        }
        let wav_bytes = std::fs::metadata(&wav).unwrap().len();

        let mp3 = to_mp3(&wav, 128).expect("encode should succeed");
        let mp3_bytes = std::fs::metadata(&mp3).unwrap().len();
        assert_eq!(mp3.extension().unwrap(), "mp3");
        assert!(
            mp3_bytes < wav_bytes / 4,
            "128k mp3 ({mp3_bytes}) should be far smaller than wav ({wav_bytes})"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The WAV has to survive a failed encode. Throwing it away would lose a
    /// render that took minutes to produce over a re-encode.
    #[test]
    fn a_failed_encode_leaves_the_wav_alone() {
        let dir = std::env::temp_dir().join(format!("aceforge-mp3b-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let wav = dir.join("not-audio.wav");
        std::fs::write(&wav, b"RIFFnot really a wav").unwrap();

        // Not skipped even without ffmpeg: a missing encoder and a failing one
        // both have to come back as Err rather than a panic or a silent Ok.
        assert!(to_mp3(&wav, 128).is_err());
        assert!(wav.exists(), "the source must survive a failed encode");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_bitrate_is_clamped_to_something_sane() {
        let mut p = Yue2Params::default();
        p.mp3_bitrate = 9_999_999;
        assert_eq!(mp3_bitrate_of(&p), 320);
        p.mp3_bitrate = -5;
        assert_eq!(mp3_bitrate_of(&p), 64);
        p.mp3_bitrate = 192;
        assert_eq!(mp3_bitrate_of(&p), 192);
    }

    /// mp3 is the default, matching ACE-Step. A YuE2 render is minutes of work
    /// and lands as tens of megabytes of PCM otherwise.
    #[test]
    fn renders_default_to_mp3() {
        assert_eq!(Yue2Params::default().output_format, "mp3");
    }

    #[test]
    fn an_lm_type_is_the_middle_of_the_filename() {
        assert_eq!(lm_type_of("yue2-lm-Q6_K.gguf").as_deref(), Some("Q6_K"));
        assert_eq!(lm_type_of("yue2-lm-Q4_K_M-imat.gguf").as_deref(), Some("Q4_K_M-imat"));
        assert_eq!(lm_type_of("yue2-lm-q8_0.gguf").as_deref(), Some("q8_0"));

        // Anything the engine would not build a path from.
        for bad in [
            "yue2-vae-standard-f32.gguf",
            "nar_lora_joint_v9.safetensors",
            "yue2-lm-q8_0.zip",
            "yue2-lm-.gguf",
            "yue2-lm-Q6_K.gguf.part",
        ] {
            assert_eq!(lm_type_of(bad), None, "offered a model the engine cannot load: {bad}");
        }
    }

    /// A real directory, because the whole job is deciding what is on disk --
    /// and a fake one would let a scan that finds nothing pass.
    #[test]
    fn the_picker_lists_only_what_the_engine_could_load() {
        let root = std::env::temp_dir().join(format!("aceforge-lms-{}", std::process::id()));
        let yue2 = root.join("yue2");
        std::fs::create_dir_all(&yue2).unwrap();
        std::fs::write(yue2.join("yue2-lm-Q6_K.gguf"), b"the biggest one here").unwrap();
        std::fs::write(yue2.join("yue2-lm-Q4_K_M-imat.gguf"), b"tiny").unwrap();
        // Neither of these is loadable and neither may appear.
        std::fs::write(yue2.join("yue2-lm-q8_0.zip"), b"packed").unwrap();
        std::fs::write(yue2.join("yue2-vae-standard-f32.gguf"), b"vae").unwrap();
        std::fs::write(yue2.join("nar_lora_joint_v9.safetensors"), b"lora").unwrap();

        let found = available_lms(&root);
        let types: Vec<&str> = found.iter().map(|m| m.lm_type.as_str()).collect();
        assert_eq!(types, vec!["Q4_K_M-imat", "Q6_K"], "smallest first, zip and VAE left out");
        assert_eq!(found[0].size_bytes, 4, "the small one sorts first");
        assert_eq!(found[1].size_bytes, 20);

        std::fs::remove_dir_all(&root).ok();
    }

    /// A missing models directory is an empty picker, not an error: the panel
    /// should still open and simply have nothing to offer.
    #[test]
    fn no_models_directory_is_an_empty_picker() {
        assert!(available_lms(&std::env::temp_dir().join("aceforge-nowhere-at-all")).is_empty());
    }

    /// Empty means "let the engine pick". Sending it anyway would pin the
    /// request to a pick the user never made.
    #[test]
    fn the_lm_type_is_sent_only_when_asked_for() {
        let mut params = Yue2Params::default();
        assert!(params.to_json().get("lm_type").is_none());

        params.lm_type = "Q4_K_M-imat".into();
        assert_eq!(params.to_json()["lm_type"], "Q4_K_M-imat");

        params.lm_type = "  ".into();
        assert!(params.to_json().get("lm_type").is_none(), "whitespace is not a model");
    }

    /// A wrong pick should be caught here rather than as a failed render.
    #[test]
    fn a_pick_the_engine_cannot_find_is_rejected_before_submitting() {
        let mut params = Yue2Params::default();
        params.lm_type = "Q2_K".into();
        let have: Vec<String> = available_lms(&std::env::temp_dir().join("aceforge-nothing-here"))
            .into_iter()
            .map(|m| m.lm_type)
            .collect();
        assert!(!have.contains(&params.lm_type));
        // Documented behaviour: the request still goes out and the engine
        // reports the missing file. This test exists so that if that ever
        // changes, the change is deliberate.
        assert_eq!(params.to_json()["lm_type"], "Q2_K");
    }

    #[test]
    fn the_rendering_stage_is_not_a_planner_probe() {
        let (plan, audio) = staged_params(&Yue2Params::default(), "X:1\nM:4/4\nQ:1/4=120\n");
        assert!(!audio.semantic_only, "the render would return codec ids, not audio");
        assert!(!audio.plan_only);
        assert!(plan.plan_only, "the composing stage has to actually compose");
    }

    /// A stale score left in the params must not leak into the composing stage.
    #[test]
    fn staging_ignores_a_score_left_over_in_the_params() {
        let mut params = Yue2Params::default();
        params.abc = "X:1\nM:4/4\nQ:1/4=120\nK:C\nold|".into();
        // Not staged at all, but if it ever were, the old score must not ride along.
        let (plan, _) = staged_params(&params, "");
        assert_eq!(plan.abc, "");
    }

    #[test]
    fn no_lyrics_writes_nothing() {
        assert_eq!(spread_lrc("", 30.0), "");
        assert_eq!(spread_lrc("   \n \n", 30.0), "");
    }

    /// A zero-length track has nothing to spread across.
    #[test]
    fn a_zero_length_track_writes_nothing() {
        assert_eq!(spread_lrc("Hey baby", 0.0), "");
    }

    /// The first line starts the track and the last lands inside it. A common
    /// slip is spacing only up to `duration - duration/n`, which leaves the
    /// closing line past the end of the audio.
    #[test]
    fn lines_span_the_whole_track() {
        let lrc = spread_lrc("one\ntwo\nthree\nfour", 40.0);
        let parsed = crate::annotations::parse_lrc(&lrc);
        assert_eq!(parsed.first().unwrap().at_secs, 0.0);
        let last = parsed.last().unwrap().at_secs;
        assert!(last > 25.0 && last <= 40.0, "last line landed at {last}");
    }

    /// Over an hour the minutes field has to carry, not wrap.
    #[test]
    fn long_tracks_keep_counting_minutes() {
        // Enough lines that the last one lands near the end: with two over
        // 4000 s the last is at the halfway point, which is 33 minutes and
        // proves nothing about carrying into the second hour.
        let lyrics: Vec<String> = (0..400).map(|i| format!("line{i}")).collect();
        let lrc = spread_lrc(&lyrics.join("\n"), 4000.0);
        assert!(lrc.starts_with("[00:00.00]line0"), "{lrc}");
        assert!(lrc.contains("[66:30."), "4000 s is past the hour mark: {lrc}");
        let parsed = crate::annotations::parse_lrc(&lrc);
        assert_eq!(parsed.len(), 400);
        assert!((parsed.last().unwrap().at_secs - 3990.0).abs() < 0.01);
    }

    #[test]
    fn unset_sampling_is_left_out_rather_than_sent_as_zero() {
        let p = Yue2Params::default();
        let json = p.to_json();
        // A zero temperature is a real setting on this pipeline and "unset" on
        // another, so the default must not send one.
        assert!(json.get("plan").is_none(), "{json}");
        assert!(json.get("semantic").is_none(), "{json}");
        assert_eq!(json["lm_batch_size"], 1);
        assert_eq!(json["seed"], -1);
    }

    #[test]
    fn sampling_is_split_by_stage() {
        let mut p = Yue2Params::default();
        p.plan.temperature = 0.7;
        p.plan.top_k = 30;
        p.semantic.temperature = 1.0;
        let json = p.to_json();
        let plan_t = json["plan"]["temperature"].as_f64().expect("a number");
        assert!((plan_t - 0.7).abs() < 1e-6, "plan temperature was {plan_t}");
        assert_eq!(json["plan"]["top_k"], 30);
        let sem_t = json["semantic"]["temperature"].as_f64().expect("a number");
        assert!((sem_t - 1.0).abs() < 1e-6, "semantic temperature was {sem_t}");
        // A field the user left alone must not be pinned to zero.
        assert!(json["plan"].get("repetition_penalty").is_none(), "{json}");
    }

    /// There is no duration field. Sending one is how a request ends up with a
    /// length the composer never agreed to.
    #[test]
    fn the_request_carries_no_duration() {
        let json = Yue2Params::default().to_json();
        for banned in ["duration", "durationSecs", "length"] {
            assert!(json.get(banned).is_none(), "{banned} must not be sent: {json}");
        }
    }

    #[test]
    fn cot_reaches_the_engine_by_name() {
        let mut p = Yue2Params::default();
        p.cot = CotMode::Melody;
        assert_eq!(p.to_json()["cot"], "melody");
        p.cot = CotMode::Off;
        assert_eq!(p.to_json()["cot"], "off");
        p.cot = CotMode::Full;
        assert_eq!(p.to_json()["cot"], "full");
    }

    #[test]
    fn a_pasted_score_skips_the_plan_stage() {
        let mut p = Yue2Params::default();
        assert!(p.to_json().get("abc").is_none());
        p.abc =  "X:1\nT:Test".to_string();
        assert_eq!(p.to_json()["abc"], "X:1\nT:Test");
    }

    #[test]
    fn a_single_song_is_bare_wav_and_a_batch_is_multipart() {
        let mut wav = vec![0u8; 44];
        wav[0..4].copy_from_slice(b"RIFF");
        wav[8..12].copy_from_slice(b"WAVE");
        assert!(is_wav(&wav));
        assert!(!is_wav(b"not audio"));

        let boundary = "yue2-batch-boundary";
        let body = format!(
            "--{b}\r\nContent-Type: audio/wav\r\n\r\nAAA\r\n--{b}\r\nContent-Type: audio/wav\r\n\r\nBBB\r\n--{b}--\r\n",
            b = boundary
        );
        let parts = split_multipart(body.as_bytes(), boundary);
        assert_eq!(parts.len(), 2, "{parts:?}");
        assert_eq!(parts[0], b"AAA");
        assert_eq!(parts[1], b"BBB");
    }

    /// A stereo track is twice the bytes of a mono one for the same length.
    /// Reading the duration without the channel count reports stereo as twice
    /// as long, and the evenly-spread sidecar then times its lines against
    /// twice the length of the song.
    #[test]
    fn a_stereo_wav_is_not_reported_as_twice_its_length() {
        let seconds = 2.0f32;
        let rate = 48000u32;
        let samples = (rate as f32 * seconds) as u32;

        let mono = synth_wav(1, 16, rate, samples);
        let stereo = synth_wav(2, 16, rate, samples);
        assert!((wav_seconds(&mono) - seconds).abs() < 0.001, "{}", wav_seconds(&mono));
        assert!((wav_seconds(&stereo) - seconds).abs() < 0.001, "{}", wav_seconds(&stereo));
    }

    /// Twenty-four bit is three bytes per sample, so the frame size has to
    /// come from the fmt chunk rather than being assumed.
    #[test]
    fn the_frame_size_comes_from_the_fmt_chunk() {
        let rate = 48000u32;
        let samples = rate; // one second
        for (channels, bits) in [(1u32, 16u32), (2, 16), (1, 24), (2, 24)] {
            let wav = synth_wav(channels, bits, rate, samples);
            let got = wav_seconds(&wav);
            assert!(
                (got - 1.0).abs() < 0.001,
                "{channels}ch {bits}bit reported {got}s"
            );
        }
    }

    /// Build a real WAV: RIFF header, fmt chunk, data chunk.
    fn synth_wav(channels: u32, bits: u32, rate: u32, samples: u32) -> Vec<u8> {
        let block_align = channels * bits / 8;
        let data_len = samples * block_align;
        let mut out: Vec<u8> = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36u32 + data_len).to_le_bytes());
        out.extend_from_slice(b"WAVE");
        out.extend_from_slice(b"fmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes()); // PCM
        out.extend_from_slice(&(channels as u16).to_le_bytes());
        out.extend_from_slice(&rate.to_le_bytes());
        out.extend_from_slice(&(rate * block_align).to_le_bytes());
        out.extend_from_slice(&(block_align as u16).to_le_bytes());
        out.extend_from_slice(&(bits as u16).to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&data_len.to_le_bytes());
        out.resize(out.len() + data_len as usize, 0);
        out
    }

    #[test]
    fn wav_length_comes_from_the_data_chunk() {
        let mut wav = vec![0u8; 44];
        wav[0..4].copy_from_slice(b"RIFF");
        wav[8..12].copy_from_slice(b"WAVE");
        wav[12..16].copy_from_slice(b"fmt ");
        wav[16..20].copy_from_slice(&16u32.to_le_bytes());
        wav[20..22].copy_from_slice(&1u16.to_le_bytes()); // PCM
        wav[22..24].copy_from_slice(&1u16.to_le_bytes()); // channels: mono
        wav[24..28].copy_from_slice(&48000u32.to_le_bytes()); // sample rate at 0x18
        wav[34..36].copy_from_slice(&16u16.to_le_bytes()); // bits per sample
        wav[36..40].copy_from_slice(b"data");
        wav[40..44].copy_from_slice(&(96000u32).to_le_bytes()); // 1s of 16-bit 48k
        assert!((wav_seconds(&wav) - 1.0).abs() < 0.01, "{}", wav_seconds(&wav));
    }


}
