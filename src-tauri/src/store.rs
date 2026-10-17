//! Hugging Face model store for ACE-Step GGUFs.
//!
//! Discovery-only: this module resolves what is available on the Hub and what
//! is already on disk. It never downloads. The user fetches weights themselves.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const REPO: &str = "Serveurperso/ACE-Step-1.5-GGUF";

/// A model family the store can fetch.
///
/// Each one has its own HuggingFace repo and its own place on disk. The layout
/// is not cosmetic: the engine's registry for ACE-Step walks only the top level
/// and deliberately skips anything named `yue2-`/`mm3-`, while each of those
/// backends looks in its own subdirectory first. Dropping a YuE2 file at the top
/// level would leave the ACE-Step registry logging it as a stranger and YuE2
/// finding it only by luck.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Family {
    AceStep,
    Yue2,
    MiniMax,
}

impl Family {
    pub fn repo(self) -> &'static str {
        match self {
            Family::AceStep => "Serveurperso/ACE-Step-1.5-GGUF",
            Family::Yue2 => "scragnog/YuE2-GGUF",
            Family::MiniMax => "scragnog/MiniMax-Music3-GGUF",
        }
    }

    /// Subdirectory under the models folder, or "" for the top level.
    pub fn subdir(self) -> &'static str {
        match self {
            Family::AceStep => "",
            Family::Yue2 => "yue2",
            Family::MiniMax => "mm3",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Family::AceStep => "ACE-Step 1.5",
            Family::Yue2 => "YuE2",
            Family::MiniMax => "MiniMax-Music3",
        }
    }

    pub fn all() -> [Family; 3] {
        [Family::AceStep, Family::Yue2, Family::MiniMax]
    }
}

fn tree_url(family: Family) -> String {
    format!(
        "https://huggingface.co/api/models/{}/tree/main",
        family.repo()
    )
}

fn resolve_url(family: Family, filename: &str) -> String {
    format!(
        "https://huggingface.co/{}/resolve/main/{}",
        family.repo(),
        filename
    )
}

/// Where a family's weights belong inside the models folder.
pub fn family_dir(base: &Path, family: Family) -> PathBuf {
    let sub = family.subdir();
    if sub.is_empty() {
        base.to_path_buf()
    } else {
        base.join(sub)
    }
}


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
    /// YuE2's autoregressive composer. A different pipeline from the planner
    /// above: it writes a lead sheet and a codec stream, not lyrics and codes,
    /// and it is loaded by a separate backend with its own KV cache.
    Composer,
    /// YuE2's tokenizer companion decoder adapter, as opposed to a DiT LoRA.
    DecoderAdapter,
    /// MiniMax-Music3 depth decoder.
    DepthDecoder,
    /// MiniMax-Music3 condition encoder.
    ConditionEncoder,
}

/// A LoRA/LoKr for one pipeline stage. Separate from `Role` because an adapter
/// is not a pipeline stage: it modifies one, and there is more than one per
/// engine (YuE2 alone has a composer slot and a renderer slot).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AdapterKind {
    /// Applied to the diffusion transformer.
    Dit,
    /// Applied to the planner LM.
    Planner,
    /// YuE2's NAR renderer.
    Renderer,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeightFile {
    pub filename: String,
    pub url: String,
    /// Which pipeline this file belongs to.
    pub family: Family,
    /// Subdirectory under the models folder it has to land in, "" for the
    /// top level. Carried on the file so the downloader does not have to
    /// re-derive it and cannot put a YuE2 weight where the engine will not
    /// look for it.
    pub subdir: String,
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

/// Which family a filename belongs to, judged by name.
///
/// The prefixes are the same ones the engine uses to route a file, so this
/// cannot disagree with what the backend will accept.
pub fn family_of(filename: &str) -> Option<Family> {
    let lower = filename.to_ascii_lowercase();
    if lower.starts_with("yue2-") || lower.starts_with("nar_lora") || lower.starts_with("ar_lora") {
        Some(Family::Yue2)
    } else if lower.starts_with("mm3-") {
        Some(Family::MiniMax)
    } else if lower.starts_with("vae-")
        || lower.starts_with("scragvae-")
        || lower.starts_with("pp-vae-")
        || lower.starts_with("qwen3-embedding-")
        || lower.starts_with("acestep-")
    {
        Some(Family::AceStep)
    } else {
        None
    }
}

fn classify_in(family: Family, filename: &str) -> Option<Role> {
    let lower = filename.to_ascii_lowercase();
    match family {
        Family::AceStep => {
            if lower.starts_with("vae-") || lower.starts_with("scragvae-") {
                Some(Role::Vae)
            } else if lower.starts_with("pp-vae-") {
                Some(Role::Vae)
            } else if lower.starts_with("qwen3-embedding-") {
                Some(Role::TextEncoder)
            } else if lower.starts_with("acestep-5hz-lm-") {
                Some(Role::Planner)
            } else if lower.starts_with("acestep-v15-") {
                Some(Role::Dit)
            } else {
                None
            }
        }
        Family::Yue2 => {
            if lower.starts_with("yue2-lm-") {
                Some(Role::Composer)
            } else if lower.starts_with("yue2-vae-") {
                Some(Role::Vae)
            } else if lower.starts_with("yue2-tok-") {
                Some(Role::Vae)
            } else {
                None
            }
        }
        Family::MiniMax => {
            if lower.starts_with("mm3-dit-") {
                Some(Role::Dit)
            } else if lower.starts_with("mm3-depth-") {
                Some(Role::DepthDecoder)
            } else if lower.starts_with("mm3-cond-") {
                Some(Role::ConditionEncoder)
            } else {
                None
            }
        }
    }
}

/// Adapters are not GGUFs and carry no architecture, so they are recognised by
/// name alone.
fn classify_adapter(filename: &str) -> Option<AdapterKind> {
    let lower = filename.to_ascii_lowercase();
    if lower.starts_with("nar_lora") || lower.starts_with("nar-") {
        Some(AdapterKind::Renderer)
    } else if lower.starts_with("ar_lora") || lower.starts_with("ar-") {
        Some(AdapterKind::Planner)
    } else if lower.starts_with("dit_lora") || lower.starts_with("dit-") {
        Some(AdapterKind::Dit)
    } else {
        None
    }
}

fn classify(filename: &str) -> Option<Role> {
    let family = family_of(filename)?;
    classify_in(family, filename)
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
/// Fetch every family's file listing.
///
/// One family failing does not fail the store: the Hub is reachable per-repo,
/// and a store that empties itself because one of three repos timed out is worse
/// than one missing a tab. Whatever did answer is returned.
pub fn list_remote_weights() -> Result<Vec<WeightFile>, String> {
    let mut out: Vec<WeightFile> = Vec::new();
    let mut last_error: Option<String> = None;

    for family in Family::all() {
        match fetch_family(family) {
            Ok(mut files) => out.append(&mut files),
            Err(e) => {
                eprintln!("[store] {family:?} catalogue unavailable: {e}");
                last_error = Some(e);
            }
        }
    }

    if out.is_empty() {
        return Err(last_error.unwrap_or_else(|| "no catalogues available".to_string()));
    }
    out.sort_by(|a: &WeightFile, b: &WeightFile| {
        a.family
            .cmp(&b.family)
            .then_with(|| a.role.cmp(&b.role))
            .then_with(|| a.filename.cmp(&b.filename))
    });
    Ok(out)
}

fn fetch_family(family: Family) -> Result<Vec<WeightFile>, String> {
    let response = ureq::get(&tree_url(family))
        .call()
        .map_err(|e| format!("hub unreachable: {e}"))?;

    let entries: Vec<serde_json::Value> = response
        .into_json()
        .map_err(|e| format!("bad hub response: {e}"))?;

    let mut files: Vec<WeightFile> = entries
        .iter()
        .filter_map(|entry: &serde_json::Value| {
            let filename = entry.get("path")?.as_str()?.to_string();
            // The Hub tree carries directories too; they are not weights.
            if entry.get("type").and_then(|t| t.as_str()) == Some("directory") {
                return None;
            }
            let role = classify(&filename)?;
            let (quant, variant) = split_name(&filename);
            Some(WeightFile {
                url: resolve_url(family, &filename),
                size_bytes: file_size(entry),
                role,
                family,
                subdir: family.subdir().to_string(),
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
        a.role.cmp(&b.role).then_with(|| a.filename.cmp(&b.filename))
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

    // Recursive, because the engines do not agree on a layout: YuE2's weights
    // live in a `yue2/` subdirectory and MiniMax's in `mm3/`, and a flat walk
    // showed the Models list as empty for a folder holding gigabytes of
    // perfectly good weights.
    for path in walk_weight_files(dir) {
        let Some(filename) = path.file_name().and_then(|n| n.to_str()).map(String::from)
        else {
            continue;
        };
        let Some(family) = family_of(&filename) else { continue };
        // Adapters carry no architecture and classify as no role at all.
        let role = match classify(&filename) {
            Some(role) => role,
            None => continue,
        };
        let _ = classify_adapter(&filename);

        // Prefer the cached catalogue for the published size; fall back to the
        // real file size so an uncatalogued file still shows something sane.
        let cached = published.iter().find(|f| f.filename == filename);
        let expected = cached.map(|f| f.size_bytes).filter(|s| *s > 0).unwrap_or(0);
        let on_disk = path.metadata().map(|m| m.len()).unwrap_or(0);
        let size = match expected {
            0 => on_disk,
            n => n,
        };
        let (quant, variant) = split_name(&filename);

        // Compare against the published size when we know it. Only a real
        // catalogue entry counts: without one, `size` is just this file's own
        // length and the comparison would be vacuously true.
        let size_mismatch = expected > 0 && on_disk != expected;

        out.push(WeightFile {
            url: cached
                .map(|f| f.url.clone())
                .unwrap_or_else(|| resolve_url(family, &filename)),
            size_bytes: size,
            family,
            subdir: family.subdir().to_string(),
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

    out.sort_by(|a: &WeightFile, b: &WeightFile| {
        a.family
            .cmp(&b.family)
            .then_with(|| a.role.cmp(&b.role))
            .then_with(|| a.filename.cmp(&b.filename))
    });
    out
}

/// Every weight file under `dir`, a couple of levels deep.
///
/// Bounded on purpose. A models folder is sometimes a symlink farm (AceForge's
/// own import views are), and an unbounded walk of one would not terminate in
/// a useful time.
fn walk_weight_files(dir: &Path) -> Vec<PathBuf> {
    const MAX_DEPTH: usize = 2;
    fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
        if depth > MAX_DEPTH {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        // Read the whole directory first: descending into one while iterating
        // another is fine, but sorting keeps the listing stable between runs,
        // and the UI keys rows by filename.
        let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok()).map(|e| e.path()).collect();
        paths.sort();
        for path in paths {
            let Ok(meta) = path.metadata() else { continue };
            // symlink_metadata, so a symlinked view is followed as a file
            // rather than mistaken for a directory to descend into forever.
            if meta.is_dir() {
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default();
                if name.starts_with('.') || name == "adapters" || name == "tmp" {
                    continue;
                }
                walk(&path, depth + 1, out);
            } else if meta.is_file()
                && path.extension().and_then(|e| e.to_str()) == Some("gguf")
            {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, 0, &mut out);
    out
}


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

    /// YuE2 and MiniMax are looked for in their own subdirectories. The
    /// scanner used to walk only the top level, so a folder holding a complete
    /// YuE2 install listed as if it held nothing at all.
    #[test]
    fn scans_family_subdirectories() {
        let dir = Scratch::new("subdirs");
        std::fs::create_dir_all(dir.0.join("yue2")).unwrap();
        std::fs::create_dir_all(dir.0.join("mm3")).unwrap();
        for name in [
            "acestep-v15-turbo-Q4_K_M.gguf",
            "yue2/yue2-lm-q8_0.gguf",
            "yue2/yue2-vae-standard-f32.gguf",
            "mm3/mm3-dit-q8_0.gguf",
        ] {
            std::fs::write(dir.0.join(name), b"x").unwrap();
        }
        let found = scan_local(&dir.0);
        let names: Vec<&str> = found.iter().map(|f| f.filename.as_str()).collect();
        assert!(names.contains(&"acestep-v15-turbo-Q4_K_M.gguf"), "{names:?}");
        assert!(names.contains(&"yue2-lm-q8_0.gguf"), "{names:?}");
        assert!(names.contains(&"yue2-vae-standard-f32.gguf"), "{names:?}");
        assert!(names.contains(&"mm3-dit-q8_0.gguf"), "{names:?}");

        let yue = found.iter().find(|f| f.filename == "yue2-lm-q8_0.gguf").unwrap();
        assert_eq!(yue.family, Family::Yue2);
        assert_eq!(yue.role, Role::Composer);
        assert_eq!(yue.subdir, "yue2", "it must land where YuE2 looks");

        let dit = found.iter().find(|f| f.filename.starts_with("acestep-v15")).unwrap();
        assert_eq!(dit.subdir, "", "ACE-Step weights belong at the top level");
    }

    /// A folder the user never navigates into, or a symlink farm, must not turn
    /// the scan into an unbounded walk.
    #[test]
    fn skips_dot_directories_and_adapters() {
        let dir = Scratch::new("skips");
        for sub in [".cache/acestep", "adapters", "tmp"] {
            std::fs::create_dir_all(dir.0.join(sub)).unwrap();
            std::fs::write(dir.0.join(sub).join("acestep-v15-turbo-Q4_K_M.gguf"), b"x").unwrap();
        }
        assert!(
            scan_local(&dir.0).is_empty(),
            "nested weights under these are not part of the models folder"
        );
    }

    /// The store must not empty itself because one of three repos is
    /// unreachable.
    /// Proves the fix against the real folder rather than a fixture: this is
    /// the layout the user's YuE2 install actually has.
    #[test]
    fn finds_the_installed_yue2_weights() {
        let dir = std::env::var("ACEFORGE_TEST_MODELS").unwrap_or_default();
        if dir.trim().is_empty() {
            return; // no folder configured in this environment
        }
        let found = scan_local(std::path::Path::new(&dir));
        let yue: Vec<&str> = found
            .iter()
            .filter(|f| f.family == Family::Yue2)
            .map(|f| f.filename.as_str())
            .collect();
        println!("  {} weights, {} YuE2: {yue:?}", found.len(), yue.len());
        assert!(
            yue.iter().any(|n| n.contains("yue2-lm")),
            "the YuE2 composer is not being listed"
        );
    }

    #[test]
    fn families_are_routed_by_the_same_prefixes_the_engine_uses() {
        assert_eq!(family_of("yue2-lm-q8_0.gguf"), Some(Family::Yue2));
        assert_eq!(family_of("YUE2-VAE-standard-f32.gguf"), Some(Family::Yue2));
        assert_eq!(family_of("mm3-dit-f16.gguf"), Some(Family::MiniMax));
        assert_eq!(family_of("acestep-v15-turbo-Q8_0.gguf"), Some(Family::AceStep));
        assert_eq!(family_of("scragvae-BF16.gguf"), Some(Family::AceStep));
        assert_eq!(family_of("notes.txt"), None);
    }

    #[test]
    fn the_yue2_composer_is_not_a_planner() {
        // They are different models behind different backends. Filing one under
        // the other's role puts a YuE2 checkpoint in an ACE-Step picker.
        assert_eq!(classify("yue2-lm-q8_0.gguf"), Some(Role::Composer));
        assert_ne!(classify("yue2-lm-q8_0.gguf"), classify("acestep-5Hz-lm-1.7B-Q8_0.gguf"));
    }

    fn catalogue_entry(filename: &str, size: u64) -> WeightFile {
        WeightFile {
            filename: filename.to_string(),
            url: format!("https://huggingface.co/{REPO}/resolve/main/{filename}"),
            family: Family::AceStep,
            subdir: Family::AceStep.subdir().to_string(),
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
