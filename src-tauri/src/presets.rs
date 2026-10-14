//! Generation presets.
//!
//! These are not arbitrary shortcuts: each one is a parameter set that is
//! known to work together. The constraints come from the model and from this
//! machine's 4 GB VRAM budget.
//!
//! Kept in Rust rather than the frontend so the numbers stay typed, so the UI
//! cannot invent a value the pipeline rejects, and so `/props` from the server
//! can be reconciled against them.

use serde::{Deserialize, Serialize};

use crate::generate::GenerateParams;

/// Why a preset exists, shown as the tooltip in the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Intent {
    /// Fewest steps, shortest render. For checking a prompt.
    Sketch,
    /// Instrumental bed, no vocals.
    Instrumental,
    /// Full song with planner-written lyrics.
    Song,
    /// Lyrics supplied by the user.
    CustomLyrics,
    /// Slower diffusion, more fidelity per step.
    Quality,
    /// Long-form ambient. Longest single render.
    Ambient,
    /// Several variations of one prompt.
    Explore,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preset {
    pub id: String,
    pub label: String,
    pub description: String,
    pub intent: Intent,
    /// Rough wall-clock estimate on a GTX 1050, measured not guessed.
    pub approx_seconds: String,
    pub params: GenerateParams,
}

/// Base defaults shared by every preset.
fn base() -> GenerateParams {
    GenerateParams {
        output_format: "mp3".into(),
        ..Default::default()
    }
}

pub fn all() -> Vec<Preset> {
    vec![
        Preset {
            id: "sketch".into(),
            label: "Sketch".into(),
            description: "20 seconds, turbo steps. The fastest way to hear whether a \
                          prompt is worth pursuing.".into(),
            intent: Intent::Sketch,
            approx_seconds: "~25s".into(),
            params: GenerateParams {
                caption: String::new(),
                lyrics: "[Instrumental]".into(),
                duration: 20.0,
                inference_steps: 8,
                synth_batch_size: 1,
                ..base()
            },
        },
        Preset {
            id: "instrumental".into(),
            label: "Instrumental".into(),
            description: "A minute of instrumental with no vocals. The empty lyrics \
                          field makes the planner write lyrics, so this preset pins \
                          the exact token the DiT was trained on for no-vocal.".into(),
            intent: Intent::Instrumental,
            approx_seconds: "~60s".into(),
            params: GenerateParams {
                caption: String::new(),
                lyrics: "[Instrumental]".into(),
                duration: 60.0,
                inference_steps: 8,
                synth_batch_size: 1,
                ..base()
            },
        },
        Preset {
            id: "song".into(),
            label: "Full song".into(),
            description: "Three minutes with planner-written lyrics and metadata.                           This is the closest to what the model was demoed with.".into(),
            intent: Intent::Song,
            approx_seconds: "~3min".into(),
            params: GenerateParams {
                caption: String::new(),
                lyrics: String::new(),
                duration: 180.0,
                inference_steps: 8,
                synth_batch_size: 1,
                ..base()
            },
        },
        Preset {
            id: "custom-lyrics".into(),
            label: "My lyrics".into(),
            description: "Keeps your lyric text verbatim and lets the planner fill \
                          only the missing metadata. Single LM pass, so it is faster \
                          than a full song.".into(),
            intent: Intent::CustomLyrics,
            approx_seconds: "~1min".into(),
            params: GenerateParams {
                caption: String::new(),
                lyrics: String::new(),
                duration: 60.0,
                inference_steps: 8,
                vocal_language: "en".into(),
                synth_batch_size: 1,
                ..base()
            },
        },
        Preset {
            id: "quality".into(),
            label: "Quality".into(),
            description: "50 diffusion steps with guidance and an unpinned schedule, \
                          matched to a base or SFT DiT. Much slower per second of \
                          audio. On a turbo DiT guidance above 1.0 is ignored and the \
                          step count stays cheap.".into(),
            intent: Intent::Quality,
            approx_seconds: "~6min".into(),
            params: GenerateParams {
                caption: String::new(),
                lyrics: String::new(),
                duration: 60.0,
                inference_steps: 50,
                guidance_scale: 1.0,
                shift: 1.0,
                synth_batch_size: 1,
                output_format: "wav16".into(),
                ..base()
            },
        },
        Preset {
            id: "ambient".into(),
            label: "Long ambient".into(),
            description: "Five minutes in a single render. Turbo steps throughout, \
                          since step count drives cost per second of audio. Watch \
                          the VAE tile size in Settings if this one runs the card \
                          close to its limit.".into(),
            intent: Intent::Ambient,
            approx_seconds: "~7min".into(),
            params: GenerateParams {
                caption: String::new(),
                lyrics: "[Instrumental]".into(),
                duration: 300.0,
                inference_steps: 8,
                synth_batch_size: 1,
                ..base()
            },
        },
        Preset {
            id: "explore".into(),
            label: "Explore variations".into(),
            description: "Four DiT takes from a single plan. The planner runs \
                          once, then the diffusion renders four times with \
                          consecutive seeds. Asking the planner for several \
                          songs instead needs a 7 GiB KV cache and does not fit \
                          here, so the variety comes from the renderer.".into(),
            intent: Intent::Explore,
            approx_seconds: "~2min".into(),
            params: GenerateParams {
                caption: String::new(),
                lyrics: String::new(),
                duration: 30.0,
                inference_steps: 8,
                synth_batch_size: 1,
                ..base()
            },
        },
    ]
}