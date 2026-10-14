//! Musical reference data and sampling presets.
//!
//! Lives in Rust so the option lists cannot drift from what the pipeline
//! accepts, and so the frontend renders them rather than re-declaring them.
//!
//! Every value here is grounded in how the model actually behaves:
//! - turbo is guidance-distilled: 8 steps, CFG pinned to 1.0, shift 3.0.
//! - base and SFT take the CFG path: ~50 steps, shift 1.0.
//! - the `extract` route is the one place guidance genuinely runs high.

use serde::{Deserialize, Serialize};

use crate::generate::GenerateParams;

/// A musical key as the model spells it, e.g. `C major`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Key {
    pub value: String,
    pub tonic: String,
    pub mode: String,
}

fn keys() -> Vec<Key> {
    let tonics = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    let mut out = Vec::new();
    for mode in ["major", "minor"] {
        for tonic in tonics {
            out.push(Key {
                value: format!("{tonic} {mode}"),
                tonic: tonic.to_string(),
                mode: mode.to_string(),
            });
        }
    }
    out
}

/// A tempo suggestion with a feel attached, so the number means something.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BpmPreset {
    pub value: u32,
    pub label: String,
}

fn bpms() -> Vec<BpmPreset> {
    [
        (60, "ballad, breathing"),
        (70, "downtempo, spacious"),
        (80, "lo-fi, head-nod"),
        (90, "hip-hop, laid back"),
        (100, "pop, four to the floor"),
        (120, "house, driving"),
        (128, "trance, rolling"),
        (140, "drum & bass, fast"),
        (160, "punk, urgent"),
        (174, "drum machine, relentless"),
    ]
    .iter()
    .map(|(v, l)| BpmPreset {
        value: *v as u32,
        label: (*l).to_string(),
    })
    .collect()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Language {
    pub code: String,
    pub label: String,
}

/// `auto` is the empty string: the planner guesses, and on this model it
/// guesses badly from an English prompt. `unknown` is a distinct, meaningful
/// value — the explicit "no particular language" signal to the DiT.
fn languages() -> Vec<Language> {
    [
        ("en", "English"),
        ("fr", "French"),
        ("es", "Spanish"),
        ("de", "German"),
        ("it", "Italian"),
        ("pt", "Portuguese"),
        ("nl", "Dutch"),
        ("ru", "Russian"),
        ("pl", "Polish"),
        ("tr", "Turkish"),
        ("ar", "Arabic"),
        ("hi", "Hindi"),
        ("ja", "Japanese"),
        ("ko", "Korean"),
        ("zh", "Chinese"),
        ("id", "Indonesian"),
        ("sv", "Swedish"),
        ("unknown", "no specific language"),
    ]
    .iter()
    .map(|(c, l)| Language {
        code: (*c).to_string(),
        label: (*l).to_string(),
    })
    .collect()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SamplingPreset {
    pub id: String,
    pub label: String,
    pub description: String,
    /// Which DiT family this is meant for.
    pub intended_for: String,
    pub approx_seconds: String,
    pub params: SamplingValues,
}

/// Just the diffusion knobs, so a sampling preset never clobbers the caption.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SamplingValues {
    pub inference_steps: i32,
    pub guidance_scale: f32,
    pub shift: f32,
    pub solver: String,
    pub synth_batch_size: i32,
}

fn sampling() -> Vec<SamplingPreset> {
    vec![
        SamplingPreset {
            id: "turbo".into(),
            label: "Turbo".into(),
            description: "Eight steps, guidance off, shift 3.0. This is the turbo \
                          model's distilled setting: more steps do not improve it \
                          and guidance above 1.0 is ignored outright.".into(),
            intended_for: "turbo DiT".into(),
            approx_seconds: "fastest".into(),
            params: SamplingValues {
                inference_steps: 8,
                guidance_scale: 1.0,
                shift: 3.0,
                solver: "euler".into(),
                synth_batch_size: 1,
            },
        },
        SamplingPreset {
            id: "sft".into(),
            label: "SFT / Base".into(),
            description: "Fifty steps at shift 1.0 with guidance off. The setting \
                          the base and SFT models were trained for, and the reason \
                          they sound markedly better than turbo.".into(),
            intended_for: "sft or base DiT".into(),
            approx_seconds: "slow".into(),
            params: SamplingValues {
                inference_steps: 50,
                guidance_scale: 1.0,
                shift: 1.0,
                solver: "euler".into(),
                synth_batch_size: 1,
            },
        },
        SamplingPreset {
            id: "balanced".into(),
            label: "Balanced".into(),
            description: "Sixteen steps at shift 2.0. A middle ground: roughly \
                          double turbo's cost for a more settled result, without \
                          committing to the full SFT budget.".into(),
            intended_for: "either DiT".into(),
            approx_seconds: "moderate".into(),
            params: SamplingValues {
                inference_steps: 16,
                guidance_scale: 1.0,
                shift: 2.0,
                solver: "euler".into(),
                synth_batch_size: 1,
            },
        },
        SamplingPreset {
            id: "guided".into(),
            label: "CFG guided".into(),
            description: "Guidance 4.0 pushes harder toward the prompt. Higher \
                          guidance follows the caption more literally and loses \
                          naturalness; the extract route runs at 7.0.".into(),
            intended_for: "base or sft DiT".into(),
            approx_seconds: "slow".into(),
            params: SamplingValues {
                inference_steps: 32,
                guidance_scale: 4.0,
                shift: 3.0,
                solver: "euler".into(),
                synth_batch_size: 1,
            },
        },
        SamplingPreset {
            id: "variations".into(),
            label: "Variations".into(),
            description: "Four DiT passes over the same lyrics and codes. Each gets \
                          a consecutive seed, so one prompt yields four genuinely \
                          different renders rather than resampled noise.".into(),
            intended_for: "either DiT".into(),
            approx_seconds: "4x a single render".into(),
            params: SamplingValues {
                inference_steps: 8,
                guidance_scale: 1.0,
                shift: 3.0,
                solver: "euler".into(),
                synth_batch_size: 4,
            },
        },
        SamplingPreset {
            id: "high-order".into(),
            label: "High-order solver".into(),
            description: "DPM++ 3M instead of Euler: a third-order multistep \
                          method that reaches the same endpoint in far fewer \
                          evaluations. State across steps, so the first few are \
                          bootstrapped from Euler.".into(),
            intended_for: "either DiT".into(),
            approx_seconds: "moderate".into(),
            params: SamplingValues {
                inference_steps: 24,
                guidance_scale: 1.0,
                shift: 2.0,
                solver: "dpm3m".into(),
                synth_batch_size: 1,
            },
        },
    ]
}

/// Reference data for the UI pickers.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MusicReference {
    pub keys: Vec<Key>,
    pub bpm_presets: Vec<BpmPreset>,
    pub languages: Vec<Language>,
    pub sampling_presets: Vec<SamplingPreset>,
    pub solvers: Vec<&'static str>,
}

pub fn reference() -> MusicReference {
    MusicReference {
        keys: keys(),
        bpm_presets: bpms(),
        languages: languages(),
        sampling_presets: sampling(),
        solvers: vec!["euler", "sde", "dpm3m", "stork4"],
    }
}

/// Merge a sampling preset into the current request.
///
/// Rust owns the merge so a sampling preset can never clobber the caption,
/// lyrics or duration the user already set.
pub fn apply_sampling(current: &GenerateParams, values: &SamplingValues) -> GenerateParams {
    GenerateParams {
        inference_steps: values.inference_steps,
        guidance_scale: values.guidance_scale,
        shift: values.shift,
        solver: values.solver.clone(),
        synth_batch_size: values.synth_batch_size,
        ..current.clone()
    }
}

pub fn find_sampling(id: &str) -> Option<SamplingPreset> {
    sampling().into_iter().find(|p| p.id == id)
}