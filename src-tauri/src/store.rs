//! Hugging Face model store for ACE-Step GGUFs.
//!
//! Discovery-only: this module resolves what is available on the Hub and what
//! is already on disk. It never downloads. The user fetches weights themselves.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const REPO: &str = "Serveurperso/ACE-Step-1.5-GGUF";
const TREE_URL: &str =
    "https://huggingface.co/api/models/Serveurperso/ACE-Step-1.5-GGUF/tree/main";


/// Which pipeline stage a weight file feeds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Role {
    /// Qwen3 causal LM that writes lyrics + 5Hz audio codes.
    Planner,
    /// Qwen3-Embedding-0.6B text encoder. Architecturally locked, no variants.
    TextEncoder,
    /// Flow matching diffusion transformer.
    Dit,
    /// Oobleck VAE decoder. Always BF16.
    Vae,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeightFile {
    pub filename: String,
    pub url: String,
    pub size_bytes: u64,
    pub role: Role,
    pub present: bool,
    /// Quantisation label, parsed out of the suffix (Q4_K_M, Q8_0, BF16...).
    pub quant: String,
    /// Model family: `turbo`, `sft`, `xl-turbo`, `5Hz-lm-0.6B`, `vae`...
    pub variant: String,
    /// True when the running server has actually loaded or can load this
    /// file. Resolved from `/props`, which reports the model registry the
    /// backend built by scanning `general.architecture` metadata. Anything
    /// not in that registry is a file this backend cannot serve.
    pub backend_supported: bool,
    /// Bytes already on disk for a partial download.
    pub partial_bytes: u64,
    /// On disk, but not the size the catalogue publishes for it.
    ///
    /// A truncated GGUF is the failure mode that hides best: the server picks
    /// models by name and dies mid-render with a GGUF error, so it reads like a
    /// model bug rather than a download bug. `present` is forced false for
    /// these so they never reach a model picker.
    #[serde(default)]
    pub size_mismatch: bool,
}

/// A named, complete set of weights. Selecting a preset is what makes a
/// models directory runnable; mixing arbitrary quants usually is not.
#[derive(Debug, Clone, Serialize)]
pub struct Preset {
    pub id: String,
    pub label: String,
    pub description: String,
    pub total_bytes: u64,
    /// VRAM headroom this preset was designed around.
    pub fits_4gb: bool,
    pub files: Vec<String>,
}

/// `acestep-v15-turbo-Q4_K_M.gguf` -> quant `Q4_K_M`, variant `turbo`.
fn split_name(filename: &str) -> (String, String) {
    let stem = filename.strip_suffix(".gguf").unwrap_or(filename);
    let quants = ["BF16", "Q4_K_M", "Q5_K_M", "Q6_K", "Q8_0"];
    for q in quants {
        if let Some(rest) = stem.strip_suffix(&format!("-{q}")) {
            return (q.to_string(), variant_of(rest));
        }
    }
    ("BF16".to_string(), variant_of(stem))
}

fn variant_of(stem: &str) -> String {
    stem.strip_prefix("acestep-v15-")
        .or_else(|| stem.strip_prefix("acestep-5Hz-lm-"))
        .or_else(|| stem.strip_prefix("Qwen3-Embedding-"))
        .unwrap_or(stem)
        .to_string()
}

fn classify(filename: &str) -> Option<Role> {
    if filename.starts_with("vae-") {
        Some(Role::Vae)
    } else if filename.starts_with("Qwen3-Embedding-") {
        Some(Role::TextEncoder)
    } else if filename.starts_with("acestep-5Hz-lm-") {
        Some(Role::Planner)
    } else if filename.starts_with("acestep-v15-") {
        Some(Role::Dit)
    } else {
        None
    }
}

fn file_size(entry: &serde_json::Value) -> u64 {
    entry
        .get("size")
        .and_then(|v| v.as_u64())
        .or_else(|| {
            entry
                .get("lfs")
                .and_then(|l| l.get("size"))
                .and_then(|v| v.as_u64())
        })
        .unwrap_or(0)
}

/// Fetch the file listing for the repo. Parses the Hub tree endpoint directly
/// rather than depending on the `hf` CLI, so nothing needs to be installed.
pub fn list_remote_weights() -> Result<Vec<WeightFile>, String> {
    let response = ureq::get(TREE_URL)
        .call()
        .map_err(|e| format!("hub unreachable: {e}"))?;

    let entries: Vec<serde_json::Value> = response
        .into_json()
        .map_err(|e| format!("bad hub response: {e}"))?;

    let mut files: Vec<WeightFile> = entries
        .iter()
        .filter_map(|entry: &serde_json::Value| {
            let filename = entry.get("path")?.as_str()?.to_string();
            let role = classify(&filename)?;
            let (quant, variant) = split_name(&filename);
            Some(WeightFile {
                url: format!("https://huggingface.co/{REPO}/resolve/main/{filename}"),
                size_bytes: file_size(entry),
                role,
                present: false,
                quant,
                variant,
                backend_supported: false,
                partial_bytes: 0,
                size_mismatch: false,
                filename,
            })
        })
        .collect();

    files.sort_by(|a: &WeightFile, b: &WeightFile| {
        a.role
            .cmp(&b.role)
            .then_with(|| a.filename.cmp(&b.filename))
    });
    Ok(files)
}

/// On-disk catalogue cache.
///
/// The published list is fetched from the Hub, but it is cached so the store
/// still knows what *else* exists while offline. This is enrichment only:
/// nothing about what you already have depends on it.
fn catalogue_path() -> PathBuf {
    crate::paths::app_data_dir().join("catalogue.json")
}

pub fn load_catalogue() -> Vec<WeightFile> {
    let Ok(text) = std::fs::read_to_string(catalogue_path()) else {
        return Vec::new();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

pub fn save_catalogue(files: &[WeightFile]) {
    let dir = crate::paths::app_data_dir();
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    if let Ok(text) = serde_json::to_string(files) {
        let _ = std::fs::write(catalogue_path(), text);
    }
}

pub fn catalogue_is_stale() -> bool {
    let Ok(meta) = std::fs::metadata(catalogue_path()) else {
        return true;
    };
    let age = meta
        .modified()
        .ok()
        .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs().saturating_sub(age) > 6 * 3600)
        .unwrap_or(true)
}

/// Read the models directory.
///
/// **Purely a filesystem scan.** It must never touch the network: this is what
/// gates the runtime, the model pickers and the Start button, and an earlier
/// version derived the list from the Hub, so a dropped connection wiped out
/// every locally installed weight and locked the app offline.
pub fn scan_local_with(dir: &Path, supported: Option<&[String]>) -> Vec<WeightFile> {
    scan_local_against(dir, supported, &load_catalogue())
}

/// The scan proper, with the catalogue passed in rather than read from disk.
/// Split out so the size check can be tested without touching real app state.
fn scan_local_against(
    dir: &Path,
    supported: Option<&[String]>,
    published: &[WeightFile],
) -> Vec<WeightFile> {
    let mut out: Vec<WeightFile> = Vec::new();

    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };

    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("gguf") {
            continue;
        }
        let Some(filename) = path.file_name().and_then(|n| n.to_str()).map(String::from) else {
            continue;
        };
        let Some(role) = classify(&filename) else { continue };

        // Prefer the cached catalogue for the published size; fall back to the
        // real file size so an uncatalogued file still shows something sane.
        let cached = published.iter().find(|f| f.filename == filename);
        let expected = cached.map(|f| f.size_bytes).filter(|s| *s > 0).unwrap_or(0);
        let size = match expected {
            0 => entry.metadata().map(|m| m.len()).unwrap_or(0),
            n => n,
        };
        let (quant, variant) = split_name(&filename);

        // Compare against the published size when we know it. Only a real
        // catalogue entry counts: without one, `size` is just this file's own
        // length and the comparison would be vacuously true.
        let on_disk = entry.metadata().map(|m| m.len()).unwrap_or(0);
        let size_mismatch = expected > 0 && on_disk != expected;

        out.push(WeightFile {
            url: cached
                .map(|f| f.url.clone())
                .unwrap_or_else(|| format!("https://huggingface.co/{REPO}/resolve/main/{filename}")),
            size_bytes: size,
            role,
            // A short file is not an installed model. Leaving `present` true
            // is what let a truncated text encoder reach a model picker and
            // kill a render.
            present: !size_mismatch,
            quant,
            variant,
            backend_supported: match supported {
                Some(list) => list.iter().any(|s| s == &filename),
                None => true,
            },
            partial_bytes: segment_bytes(dir, &filename, size),
            size_mismatch,
            filename,
        });
    }

    // Offer published-but-absent weights too, so the store is still browsable.
    for file in published {
        if out.iter().any(|f| f.filename == file.filename) {
            continue;
        }
        let present = dir.join(&file.filename).is_file();
        out.push(WeightFile {
            present,
            partial_bytes: if present {
                0
            } else {
                segment_bytes(dir, &file.filename, file.size_bytes)
            },
            // Nothing on disk at all, so there is no size to be wrong about.
            // Set explicitly rather than inherited: the cached catalogue must
            // never leak a stale flag into a live scan.
            size_mismatch: false,
            backend_supported: match supported {
                Some(list) => list.iter().any(|s| s == &file.filename),
                None => true,
            },
            ..file.clone()
        });
    }

    out.sort_by(|a: &WeightFile, b: &WeightFile| {
        // Installed first, then by role and name: the list you can actually use
        // should not be buried under 50 weights you have not downloaded.
        b.present
            .cmp(&a.present)
            .then_with(|| a.role.cmp(&b.role))
            .then_with(|| a.filename.cmp(&b.filename))
    });
    out
}

/// Refresh the published catalogue from the Hub and cache it.
///
/// Explicit and non-blocking from the caller's perspective: the UI renders from
/// the local scan first and calls this in the background.
pub fn refresh_catalogue() -> Result<Vec<WeightFile>, String> {
    let files = list_remote_weights()?;
    save_catalogue(&files);
    Ok(files)
}

pub fn scan_local(dir: &Path) -> Vec<WeightFile> {
    scan_local_with(dir, None)
}

/// Bytes already fetched for an in-progress download.
fn segment_bytes(dir: &Path, filename: &str, total: u64) -> u64 {
    if total == 0 {
        return 0;
    }
    let mut sum = 0u64;
    for i in 0..4 {
        let path = dir.join(format!("{filename}.part{i}"));
        if let Ok(meta) = std::fs::metadata(&path) {
            sum += meta.len();
        }
    }
    sum.min(total)
}

/// Presets mirror what `models.sh` can assemble, restricted to combinations
/// that are actually runnable.
///
/// Constraints that shaped these:
/// - The 0.6B/1.7B LMs only ever exist as BF16 or Q8_0. Aggressive quant is
///   not offered because it is not published for them.
/// - The 4B LM has no Q4_K_M: it breaks audio code generation.
/// - The VAE is always BF16. It is small and quality critical.
/// - The text encoder is architecturally locked to 0.6B; its CondEncoder
///   projection weights are baked into every DiT checkpoint.
fn build_presets(all: &[WeightFile]) -> Vec<Preset> {
    let has = |name: &str| all.iter().any(|f| f.filename == name);
    let size = |name: &str| {
        all.iter()
            .find(|f| f.filename == name)
            .map(|f| f.size_bytes)
            .unwrap_or(0)
    };

    let mut presets = Vec::new();

    // The 4 GB card preset: smallest planner, smallest DiT.
    let lean: Vec<String> = [
        "vae-BF16.gguf",
        "Qwen3-Embedding-0.6B-Q8_0.gguf",
        "acestep-5Hz-lm-0.6B-Q8_0.gguf",
        "acestep-v15-turbo-Q4_K_M.gguf",
    ]
    .iter()
    .filter(|n| has(n))
    .map(|s| s.to_string())
    .collect();

    if lean.len() == 4 {
        presets.push(Preset {
            total_bytes: lean.iter().map(|n| size(n)).sum(),
            id: "lean-4gb".into(),
            label: "Lean (4 GB VRAM)".into(),
            description: "0.6B planner with a Q4_K_M turbo DiT. The default when the \
                          ModelStore evicts between stages, so peak VRAM tracks the \
                          largest single module rather than the sum. Turbo runs 8 steps."
                .into(),
            fits_4gb: true,
            files: lean,
        });
    }

    // Better lyrics from the 1.7B planner, same cheap DiT.
    let mid: Vec<String> = [
        "vae-BF16.gguf",
        "Qwen3-Embedding-0.6B-Q8_0.gguf",
        "acestep-5Hz-lm-1.7B-Q8_0.gguf",
        "acestep-v15-turbo-Q4_K_M.gguf",
    ]
    .iter()
    .filter(|n| has(n))
    .map(|s| s.to_string())
    .collect();

    if mid.len() == 4 {
        presets.push(Preset {
            total_bytes: mid.iter().map(|n| size(n)).sum(),
            id: "mid-4gb".into(),
            label: "Mid (4 GB VRAM, better lyrics)".into(),
            description: "1.7B planner for stronger lyrics and metadata, still a Q4_K_M \
                          turbo DiT. Peak VRAM is set by the planner at roughly 1.8 GB."
                .into(),
            fits_4gb: true,
            files: mid,
        });
    }

    // Quality DiT for a bigger budget. SFT is 32-50 steps instead of 8.
    for (id, label, dit, fits) in [
        ("quality-sft-q4", "Quality SFT Q4_K_M", "acestep-v15-sft-Q4_K_M.gguf", false),
        ("quality-turbo-q8", "Quality turbo Q8_0", "acestep-v15-turbo-Q8_0.gguf", false),
    ] {
        let set: Vec<String> = [
            "vae-BF16.gguf",
            "Qwen3-Embedding-0.6B-Q8_0.gguf",
            "acestep-5Hz-lm-1.7B-Q8_0.gguf",
            dit,
        ]
        .iter()
        .filter(|n| has(n))
        .map(|s| s.to_string())
        .collect();

        if set.len() == 4 {
            presets.push(Preset {
                total_bytes: set.iter().map(|n| size(n)).sum(),
                id: id.into(),
                label: label.into(),
                description: if fits {
                    String::new()
                } else {
                    "Higher fidelity DiT. Expect to need 6-8 GB of VRAM, or accept \
                     partial offload to system RAM."
                        .into()
                },
                fits_4gb: fits,
                files: set,
            });
        }
    }

    presets
}

pub fn presets(all: &[WeightFile]) -> Vec<Preset> {
    build_presets(all)
}

/// Default models directory. Re-exported from `settings` so both concerns
/// have one definition.
pub fn default_models_dir() -> PathBuf {
    crate::settings::default_models_dir()
}

/// Validate a models directory: which required roles are satisfied, and the
/// first concrete reason a generation would fail if any are missing.
#[derive(Debug, Clone, Serialize)]
pub struct Validation {
    pub ok: bool,
    pub models_dir: String,
    pub dir_exists: bool,
    pub free_bytes: u64,
    pub present: Vec<String>,
    pub missing: Vec<String>,
    pub reason: Option<String>,
}

pub fn validate(dir: &Path) -> Validation {
    let dir_exists = dir.is_dir();
    let files = scan_local(dir);
    let present: Vec<String> = files
        .iter()
        .filter(|f| f.present)
        .map(|f| f.filename.clone())
        .collect();

    let role_present = |role: Role| {
        files
            .iter()
            .any(|f| f.role == role && f.present)
    };

    let mut missing = Vec::new();
    if !role_present(Role::Planner) {
        missing.push("planner LM (acestep-5Hz-lm-*)".into());
    }
    if !role_present(Role::TextEncoder) {
        missing.push("text encoder (Qwen3-Embedding-0.6B-*)".into());
    }
    if !role_present(Role::Dit) {
        missing.push("DiT (acestep-v15-*)".into());
    }
    if !role_present(Role::Vae) {
        missing.push("VAE (vae-BF16.gguf)".into());
    }

    let reason = if !dir_exists {
        Some(format!("{} does not exist", dir.display()))
    } else if !missing.is_empty() {
        Some(format!("missing {}", missing.join(", ")))
    } else {
        None
    };

    Validation {
        ok: reason.is_none(),
        models_dir: dir.display().to_string(),
        dir_exists,
        free_bytes: 0,
        present,
        missing,
        reason,
    }
}

/// `curl` line for one weight. The store hands these out rather than fetching,
/// so the user controls when and where bytes land.
pub fn curl_command(filename: &str) -> String {
    format!(
        "curl -L --fail -C - -o \"{filename}\" \
         \"https://huggingface.co/{REPO}/resolve/main/{filename}\""
    )
}
#[cfg(test)]
mod tests {
    use super::*;

    /// Scratch dir that cleans up after itself. Unique per test so a leftover
    /// from a failed run cannot leak into the next one.
    struct Scratch(std::path::PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "aceforge-store-test-{tag}-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("scratch dir");
            Self(dir)
        }

        fn write(&self, name: &str, bytes: usize) {
            std::fs::write(self.0.join(name), vec![0u8; bytes]).expect("write fixture");
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn catalogue_entry(filename: &str, size: u64) -> WeightFile {
        WeightFile {
            filename: filename.to_string(),
            url: format!("https://huggingface.co/{REPO}/resolve/main/{filename}"),
            size_bytes: size,
            role: Role::TextEncoder,
            present: false,
            quant: "Q8_0".to_string(),
            variant: "0.6B".to_string(),
            backend_supported: false,
            partial_bytes: 0,
            size_mismatch: false,
        }
    }

    /// The failure this exists for: a partial download sitting in the models
    /// dir looked identical to a good one, so it was offered in a model picker
    /// and killed the render with a GGUF error instead of a download error.
    #[test]
    fn flags_a_short_file_and_withholds_it_from_pickers() {
        let dir = Scratch::new("short");
        dir.write("Qwen3-Embedding-0.6B-Q8_0.gguf", 90);
        let published = [catalogue_entry("Qwen3-Embedding-0.6B-Q8_0.gguf", 784)];

        let files = scan_local_against(&dir.0, None, &published);
        let f = files
            .iter()
            .find(|f| f.filename == "Qwen3-Embedding-0.6B-Q8_0.gguf")
            .expect("entry");

        assert!(f.size_mismatch, "a 90 byte file claiming 784 must be flagged");
        assert!(!f.present, "a short file is not installed and must not reach a picker");
    }

    #[test]
    fn accepts_a_file_matching_the_catalogue() {
        let dir = Scratch::new("exact");
        dir.write("Qwen3-Embedding-0.6B-Q8_0.gguf", 784);
        let published = [catalogue_entry("Qwen3-Embedding-0.6B-Q8_0.gguf", 784)];

        let files = scan_local_against(&dir.0, None, &published);
        let f = files
            .iter()
            .find(|f| f.filename == "Qwen3-Embedding-0.6B-Q8_0.gguf")
            .expect("entry");

        assert!(!f.size_mismatch);
        assert!(f.present, "a correctly sized file is installed");
    }

    /// Without a published size there is nothing to compare against, and
    /// inventing a mismatch would flag every locally placed model.
    #[test]
    fn accepts_uncatalogued_files_without_guessing() {
        let dir = Scratch::new("uncatalogued");
        dir.write("Qwen3-Embedding-0.6B-Q8_0.gguf", 90);

        let files = scan_local_against(&dir.0, None, &[]);
        let f = files
            .iter()
            .find(|f| f.filename == "Qwen3-Embedding-0.6B-Q8_0.gguf")
            .expect("entry");

        assert!(!f.size_mismatch);
        assert!(f.present);
    }
}
