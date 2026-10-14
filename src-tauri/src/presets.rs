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
    /// Whether this preset is allowed to write the lyrics field.
    ///
    /// Almost none of them should. A preset is a set of *rendering* knobs --
    /// steps, guidance, length -- and applying one silently replaced whatever
    /// lyrics the user had pasted in, which is the single most expensive thing
    /// a preset can clobber. So the field is opt-in and only the preset whose
    /// entire point is the vocal mode claims it.
    ///
    /// "My lyrics" in particular must never set it: its purpose is the text
    /// already on screen.
    pub sets_lyrics: bool,
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
            sets_lyrics: false,
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
            sets_lyrics: true,
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
            sets_lyrics: false,
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
            sets_lyrics: false,
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
            sets_lyrics: false,
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
            sets_lyrics: false,
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
            sets_lyrics: false,
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
#[cfg(test)]
mod tests {
    use super::*;

    /// Applying a preset must not destroy lyrics the user pasted in.
    ///
    /// This is the regression that prompted the `sets_lyrics` flag: every
    /// preset carried a `lyrics` value, and the UI applied it wholesale, so
    /// selecting any preset replaced the text in the lyrics box -- either with
    /// `[Instrumental]` or with an empty string.
    #[test]
    fn only_the_instrumental_preset_writes_the_lyrics_field() {
        let all_presets = all();
        let owners: Vec<&str> = all_presets
            .iter()
            .filter(|p| p.sets_lyrics)
            .map(|p| p.id.as_str())
            .collect();
        assert_eq!(
            owners,
            vec!["instrumental"],
            "a preset other than the instrumental one now overwrites pasted lyrics"
        );
    }

    /// The preset whose whole purpose is the user's own text must never claim
    /// the field, however convenient it would be.
    #[test]
    fn the_custom_lyrics_preset_does_not_own_lyrics() {
        let p = all().into_iter().find(|p| p.id == "custom-lyrics").expect("preset exists");
        assert!(
            !p.sets_lyrics,
            "a preset for the user's own lyrics must leave them alone"
        );
    }

    /// Presets that do own the field must actually put something in it,
    /// otherwise the flag is silently a no-op and selecting one leaves a
    /// vocal track with no indication that instrumental was asked for.
    #[test]
    fn a_preset_that_owns_lyrics_supplies_some() {
        for p in all().into_iter().filter(|p| p.sets_lyrics) {
            assert!(
                !p.params.lyrics.trim().is_empty(),
                "{} claims the lyrics field but supplies nothing",
                p.id
            );
        }
    }

    /// The flag has to reach the frontend, or the UI cannot honour it.
    #[test]
    fn sets_lyrics_is_serialised_for_the_frontend() {
        let json = serde_json::to_string(&all()).unwrap();
        assert!(json.contains("\"setsLyrics\":true"), "got {}", &json[..200]);
        assert!(json.contains("\"setsLyrics\":false"), "got {}", &json[..200]);
    }
}

