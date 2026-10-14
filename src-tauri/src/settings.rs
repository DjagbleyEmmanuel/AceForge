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
    /// audio.cpp models live somewhere else entirely: one fused GGUF per
    /// family plus sidecar configs, which acestep.cpp's four-file layout is not.
    #[serde(default)]
    pub audiocpp_models_dir: String,
    /// audio.cpp backend: cpu, cuda, vulkan, metal, hip.
    #[serde(default = "default_ac_backend")]
    pub audiocpp_backend: String,
    #[serde(default)]
    pub audiocpp_device: u32,
    /// CPU threads. 0 means let audio.cpp decide.
    #[serde(default)]
    pub audiocpp_threads: u32,
    /// Where finished songs and imported audio live. Empty means the default
    /// app-data library.
    ///
    /// Moving this changes the Tauri asset-protocol scope at runtime as well:
    /// the scope ships as `$APPDATA/**`, so a library anywhere else would be
    /// written fine and then fail to play.
    #[serde(default)]
    pub library_dir: String,
}

fn default_ac_backend() -> String {
    // CUDA initialises fine here but a 3B model does not fit in 4 GB, and a
    // failed allocation is worse than a slow render.
    "cpu".to_string()
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
            audiocpp_models_dir: String::new(),
            audiocpp_backend: default_ac_backend(),
            audiocpp_device: 0,
            audiocpp_threads: 0,
            library_dir: String::new(),
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