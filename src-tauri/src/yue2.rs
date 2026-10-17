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
pub fn render(
    port: u16,
    params: &Yue2Params,
    on_stage: &mut dyn FnMut(&str, &str),
) -> Result<Yue2Outcome, String> {
    let models_dir = crate::settings::resolve_hotstep_models_dir();
    let mark = crate::server::stderr_mark(&models_dir);

    on_stage("yue2", "composing");
    let id = generate::submit_json(port, "/yue2/synth", params.to_json())?;

    let state = generate::wait_for_job(port, &id, mark, &mut |_| on_stage("yue2", "rendering"))?;
    if state == generate::JobState::Cancelled {
        return Ok(Yue2Outcome {
            tracks: Vec::new(),
            score: None,
            semantic_ids: None,
            cancelled: true,
        });
    }

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
    let stem_base = crate::paths::song_stem("", &params.style, &params.lyrics);

    let mut tracks = Vec::new();
    for (index, data) in parts.iter().enumerate() {
        if !is_wav(data) {
            continue;
        }
        let stem = crate::paths::unique_path(&out_dir, &stem_base, "wav");
        let path = PathBuf::from(stem);
        let path = if index == 0 {
            path
        } else {
            crate::paths::unique_path(&out_dir, &format!("{stem_base}-{index}"), "wav")
        };
        std::fs::write(&path, data)
            .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        let seconds = wav_seconds(data);
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
    let mut pos = 12usize;
    let mut rate = 0u32;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = rd(pos + 4) as usize;
        if id == b"fmt " && pos + 8 + 16 <= bytes.len() {
            rate = rd(pos + 12);
            break;
        }
        pos += 8 + size + (size & 1);
    }
    let mut pos = 12usize;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = rd(pos + 4) as usize;
        if id == b"data" {
            // 16-bit mono and stereo both divide by 2; the channel count is not
            // read here because the caller only wants a length for the list.
            return if rate > 0 {
                (size / 2) as f32 / rate as f32
            } else {
                0.0
            };
        }
        pos += 8 + size + (size & 1);
    }
    0.0
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn wav_length_comes_from_the_data_chunk() {
        let mut wav = vec![0u8; 44];
        wav[0..4].copy_from_slice(b"RIFF");
        wav[8..12].copy_from_slice(b"WAVE");
        wav[12..16].copy_from_slice(b"fmt ");
        wav[16..20].copy_from_slice(&16u32.to_le_bytes());
        wav[24..28].copy_from_slice(&48000u32.to_le_bytes()); // sample rate at 0x18
        wav[36..40].copy_from_slice(b"data");
        wav[40..44].copy_from_slice(&(96000u32).to_le_bytes()); // 1s of 16-bit 48k
        assert!((wav_seconds(&wav) - 1.0).abs() < 0.01, "{}", wav_seconds(&wav));
    }
}