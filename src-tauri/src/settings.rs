//! Persisted app settings.
//!
//! Small JSON file in the user's config dir. Deliberately not a database and
//! not Tauri store: one key, hand-editable if it ever gets wedged.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    /// Where the user keeps GGUF weights. Empty means "use the default".
    pub models_dir: String,
    /// Start ace-server automatically once weights validate.
    pub autostart_server: bool,
    /// Latent frames per VAE tile. This is the single biggest lever on peak
    /// VRAM: the decoder builds a 1920x-upsampled graph per tile, so the
    /// upstream default of 1024 will not fit a 4 GB card.
    #[serde(default = "default_vae_chunk")]
    pub vae_chunk: i32,
    /// LM KV cache size. The upstream default of 8191 is sized for 8 GB cards:
    /// with the 1.7B planner an 802-token prefill cannot allocate its graph
    /// buffer on a 4 GB card at 8192 and the server dies with OOM.
    #[serde(default = "default_max_seq")]
    pub max_seq: i32,
    /// Weights for the HOT-Step CPP engine.
    ///
    /// Deliberately separate from `models_dir`: the two engines do not agree on
    /// a file layout, and sharing one folder means each engine loads the other's
    /// weights and fails in a way that looks like a broken model rather than a
    /// wrong folder. Empty means the default location, which reuses the ACE-Step
    /// folder when it exists so a single set of weights serves both.
    #[serde(default)]
    pub hotstep_models_dir: String,
    /// LoRA/LoKr adapters for the HOT-Step engine, scanned at start-up.
    #[serde(default)]
    pub hotstep_adapters_dir: String,
    /// Which HOT-Step backend to drive. The engine picks the pipeline from the
    /// weights present, so this is a preference about which to load first, not
    /// a switch that enables something.
    #[serde(default = "default_hotstep_backend")]
    pub hotstep_backend: String,
    /// Where finished songs and imported audio live. Empty means the default
    /// app-data library.
    ///
    /// Moving this changes the Tauri asset-protocol scope at runtime as well:
    /// the scope ships as `$APPDATA/**`, so a library anywhere else would be
    /// written fine and then fail to play.
    #[serde(default)]
    pub library_dir: String,
    /// A folder of images used as cover art. Empty means "use the covers
    /// bundled with the app". Anything the webview can decode is accepted, so
    /// this is not limited to a fixed extension list at the settings level.
    #[serde(default)]
    pub covers_dir: String,
}

fn default_hotstep_backend() -> String {
    // HOT-Step picks the pipeline from the weights it finds. ACE-Step is the
    // one whose weights are already on this machine, so it is the useful
    // default; the others light up once their models are downloaded.
    "acestep".to_string()
}

fn default_max_seq() -> i32 {
    4096
}

/// 256 keeps VAE decode inside a 4 GB budget while staying fast enough:
/// measured 19.6s of stereo audio decoded and MP3 encoded in 17s.
fn default_vae_chunk() -> i32 {
    256
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            models_dir: String::new(),
            autostart_server: false,
            vae_chunk: default_vae_chunk(),
            max_seq: default_max_seq(),
            hotstep_models_dir: String::new(),
            hotstep_adapters_dir: String::new(),
            hotstep_backend: default_hotstep_backend(),
            library_dir: String::new(),
            covers_dir: String::new(),
        }
    }
}

fn settings_path() -> PathBuf {
    crate::paths::settings_file()
}

pub fn load() -> Settings {
    let path = settings_path();
    let Ok(text) = std::fs::read_to_string(path) else {
        return Settings::default();
    };
    // A corrupt file should not brick the app; fall back to defaults.
    serde_json::from_str(&text).unwrap_or_default()
}

pub fn save(settings: &Settings) -> Result<(), String> {
    let path = settings_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// Resolve the models directory. Precedence:
///
/// 1. `ACEFORGE_MODELS_DIR` — dev escape hatch, wins so a build tree can be
///    pointed at a scratch dir without editing config.
/// 2. The saved setting, if the user picked one.
/// 3. The default under the user's data dir.
/// Weights folder for the HOT-Step engine.
///
/// Falls back to the ACE-Step folder rather than a directory of its own: both
/// engines read the same `acestep-*` GGUFs for the ACE-Step backend, and
/// asking someone to point a second setting at the folder they already filled
/// in would only invite the two to disagree.
pub fn resolve_hotstep_models_dir() -> PathBuf {
    let saved = load().hotstep_models_dir;
    if !saved.trim().is_empty() {
        return PathBuf::from(saved);
    }
    std::env::var("ACEFORGE_HOTSTEP_MODELS_DIR")
        .ok()
        .filter(|d| !d.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| resolve_models_dir(None))
}

/// Adapters for the HOT-Step engine, which ships Lua solver, scheduler,
/// guidance and postprocess plugins alongside the weights.
pub fn resolve_hotstep_adapters_dir() -> Option<PathBuf> {
    let saved = load().hotstep_adapters_dir;
    if !saved.trim().is_empty() {
        return Some(PathBuf::from(saved));
    }
    // Only passed when it exists: the engine scans the directory eagerly and
    // logs a miss on every start otherwise.
    let guess = resolve_hotstep_models_dir().join("adapters");
    guess.is_dir().then_some(guess)
}

pub fn resolve_models_dir(override_dir: Option<String>) -> PathBuf {
    override_dir
        .filter(|d| !d.trim().is_empty())
        .or_else(|| std::env::var("ACEFORGE_MODELS_DIR").ok().filter(|d| !d.trim().is_empty()))
        .or_else(|| {
            let saved = load().models_dir;
            if saved.trim().is_empty() {
                None
            } else {
                Some(saved)
            }
        })
        .map(PathBuf::from)
        .unwrap_or_else(default_models_dir)
}

pub fn default_models_dir() -> PathBuf {
    crate::paths::app_data_dir().join("models")
}

/// Expand a leading `~` and make the path absolute, so a value typed into the
/// settings box behaves like a real path everywhere else.
pub fn normalize(input: &str) -> Result<PathBuf, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("path is empty".to_string());
    }

    let expanded = if trimmed == "~" || trimmed.starts_with("~/") {
        let home = dirs::home_dir().ok_or_else(|| "cannot resolve home directory".to_string())?;
        if trimmed == "~" {
            home
        } else {
            home.join(&trimmed[2..])
        }
    } else {
        PathBuf::from(trimmed)
    };

    // Reject relative paths rather than resolving them against the cwd: a GUI
    // app's cwd is wherever the launcher happened to start it, so `models/`
    // would silently point somewhere different on every run.
    if !expanded.is_absolute() {
        return Err(format!(
            "path must be absolute: {}",
            expanded.display()
        ));
    }
    Ok(expanded)
}