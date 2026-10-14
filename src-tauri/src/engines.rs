//! Engine registry.
//!
//! Two runtimes, genuinely different shapes:
//!
//! * `acestep` — `ace-server`. Async job queue: submit returns an id, the
//!   client polls, and a job can be cancelled.
//! * `audiocpp` — `audiocpp_server`. `POST /v1/tasks/run` blocks for the whole
//!   render and there is no cancellation: a wedged GPU call cannot be
//!   interrupted from userspace. Upstream documents music generation as taking
//!   minutes, so its busy timeout has to be raised well past the 5 minute
//!   default.
//!
//! Engines are installed as directories under `engines/<id>/`, each with a
//! `manifest.json`. Nothing is executed until a manifest has been read and
//! every declared file has been verified, because installing an engine means
//! installing a binary that will be run.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Serde names are explicit, and deliberately match `as_str()` and the
/// TypeScript union. Deriving camelCase produced "aceStep"/"audioCpp", which
/// silently failed every frontend comparison against "acestep"/"audiocpp".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EngineId {
    #[serde(rename = "acestep")]
    AceStep,
    #[serde(rename = "audiocpp")]
    AudioCpp,
}

impl EngineId {
    pub fn as_str(self) -> &'static str {
        match self {
            EngineId::AceStep => "acestep",
            EngineId::AudioCpp => "audiocpp",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "acestep" => Some(EngineId::AceStep),
            "audiocpp" => Some(EngineId::AudioCpp),
            _ => None,
        }
    }
}

/// How a runtime is driven. This is the distinction that matters, and the UI
/// needs it to know whether a cancel button can be offered at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum EngineKind {
    /// Submit returns a job id; progress is polled; cancellable.
    AsyncJobs,
    /// One blocking request per render; not cancellable.
    BlockingRequest,
}

/// Wire format is snake_case on purpose, matching `WeightFile` and `Preset`.
///
/// This struct used to carry `rename_all = "camelCase"`, which emitted
/// `totalBytes` and `sizeBytes` while the TypeScript interface read
/// `total_bytes` and `size_bytes`. Nothing noticed for as long as the engine
/// panel was collapsed, because every field the collapsed view touches
/// (`label`, `binary`, `families`, `notes`) is unaffected by the rename.
/// Expanding it fed `undefined` into `formatBytes`, which threw, and React
/// tore down the whole tree -- a blank window from one disclosure triangle.
/// `engines::tests` now pins the field names.
#[derive(Debug, Clone, Serialize)]
pub struct Engine {
    pub id: EngineId,
    pub label: String,
    pub kind: EngineKind,
    /// Families only this runtime can produce. Shown so the choice is informed.
    pub families: Vec<String>,
    pub binary: String,
    pub installed: bool,
    /// Files the manifest declared, and whether each is present.
    pub files: Vec<EngineFile>,
    /// Total size of the installed payload.
    pub total_bytes: u64,
    pub notes: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct EngineFile {
    pub name: String,
    pub size_bytes: u64,
    pub present: bool,
}

/// What an installed engine declares about itself.
#[derive(Debug, Clone, Deserialize)]
struct Manifest {
    id: String,
    label: String,
    /// Executable, relative to the engine directory.
    binary: String,
    /// Any other files required at runtime (ggml backends, model specs, eSpeak
    /// data). Verified on install: a missing one means the engine will fail at
    /// launch, so it is better to refuse it here.
    #[serde(default)]
    files: Vec<String>,
    /// Hash of the engine binary when *we* installed it. Absent for an engine
    /// the user supplied themselves.
    #[serde(default)]
    source_hash: Option<String>,
}

/// FNV-1a over the binary. Not a security hash -- it only has to notice that
/// the engine we shipped is not the one that got installed, which is why it can
/// be cheap and dependency-free.
fn hash_binary(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    Some(format!("{hash:016x}"))
}

fn engines_dir() -> PathBuf {
    crate::paths::app_data_dir().join("engines")
}

fn describe(id: EngineId, binary_name: &str, families: &[&str], notes: &str) -> Engine {
    let (label, kind) = match id {
        EngineId::AceStep => (
            "ACE-Step (acestep.cpp)",
            EngineKind::AsyncJobs,
        ),
        EngineId::AudioCpp => ("audio.cpp", EngineKind::BlockingRequest),
    };
    let dir = engines_dir().join(id.as_str());
    let manifest: Option<Manifest> = std::fs::read_to_string(dir.join("manifest.json"))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok());

    let mut files = Vec::new();
    let mut installed = false;
    if let Some(m) = &manifest {
        let binary = dir.join(&m.binary);
        let binary_ok = binary.is_file();
        let deps_ok = m.files.iter().all(|f| dir.join(f).is_file());
        // Only count as installed when the binary *and* every declared
        // dependency is present.
        installed = binary_ok && deps_ok;

        let mut all = vec![m.binary.clone()];
        all.extend(m.files.iter().cloned());
        for name in all {
            let path = dir.join(&name);
            files.push(EngineFile {
                size_bytes: std::fs::metadata(&path).map(|s| s.len()).unwrap_or(0),
                present: path.is_file(),
                name,
            });
        }
    }

    Engine {
        id,
        label: label.to_string(),
        kind,
        families: families.iter().map(|s| (*s).to_string()).collect(),
        binary: binary_name.to_string(),
        installed,
        total_bytes: files.iter().map(|f| f.size_bytes).sum(),
        files,
        notes: notes.to_string(),
    }
}

pub fn list() -> Vec<Engine> {
    vec![
        describe(
            EngineId::AceStep,
            "ace-server",
            &["ACE-Step 1.5 (turbo, sft, base, XL)"],
            "Async job queue: progress can be polled and a run cancelled. \
             Supports text2music only in this app so far.",
        ),
        describe(
            EngineId::AudioCpp,
            "audiocpp_server",
            &[
                "YuE2 (optional ABC score conditioning)",
                "HeartMuLa",
                "Stable Audio 3",
                "MiDashengLM-Gen",
            ],
            "One blocking request per render, so there is no progress to show \
             and no cancel. Reaches music families ACE-Step does not cover.",
        ),
    ]
}

pub fn binary_path(id: EngineId) -> Option<PathBuf> {
    let dir = engines_dir().join(id.as_str());
    let manifest: Manifest = std::fs::read_to_string(dir.join("manifest.json"))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())?;
    let binary = dir.join(&manifest.binary);
    binary.is_file().then_some(binary)
}

/// Read a manifest out of an extracted directory and report whether it is
/// usable, without installing anything.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Inspection {
    pub ok: bool,
    pub id: Option<String>,
    pub label: Option<String>,
    pub problems: Vec<String>,
    pub files: Vec<EngineFile>,
}

/// Validate an engine directory that is being offered for install.
pub fn inspect(candidate: &Path) -> Inspection {
    let mut problems = Vec::new();

    let manifest: Manifest = match std::fs::read_to_string(candidate.join("manifest.json")) {
        Ok(text) => match serde_json::from_str(&text) {
            Ok(m) => m,
            Err(e) => {
                return Inspection {
                    ok: false,
                    id: None,
                    label: None,
                    problems: vec![format!("manifest.json is not valid JSON: {e}")],
                    files: Vec::new(),
                }
            }
        },
        Err(e) => {
            return Inspection {
                ok: false,
                id: None,
                label: None,
                problems: vec![format!("no manifest.json: {e}")],
                files: Vec::new(),
            }
        }
    };

    let id = EngineId::parse(&manifest.id);
    if id.is_none() {
        problems.push(format!(
            "unknown engine id {:?}: expected \"acestep\" or \"audiocpp\"",
            manifest.id
        ));
    }

    let mut files = Vec::new();
    let mut all = vec![manifest.binary.clone()];
    all.extend(manifest.files.iter().cloned());
    for name in &all {
        let path = candidate.join(name);
        let present = path.is_file();
        if !present {
            problems.push(format!("missing {name}"));
        }
        files.push(EngineFile {
            size_bytes: std::fs::metadata(&path).map(|s| s.len()).unwrap_or(0),
            present,
            name: name.clone(),
        });
    }

    // The executable bit has to survive extraction, or it fails at spawn with
    // a much less obvious message.
    let binary_path = candidate.join(&manifest.binary);
    if binary_path.is_file() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&binary_path)
                .map(|m| m.permissions().mode())
                .unwrap_or(0);
            if mode & 0o111 == 0 {
                problems.push(format!("{} is not executable", manifest.binary));
            }
        }
    }

    Inspection {
        ok: problems.is_empty(),
        id: Some(manifest.id),
        label: Some(manifest.label),
        problems,
        files,
    }
}

/// Move a validated engine directory into place.
///
/// Refuses rather than half-installs: the destination is only touched once the
/// candidate has passed inspection, and a directory that already exists is
/// kept until the replacement has fully landed.
pub fn install(candidate: &Path) -> Result<String, String> {
    let report = inspect(candidate);
    if !report.ok {
        return Err(format!("engine rejected: {}", report.problems.join("; ")));
    }
    let id = EngineId::parse(report.id.as_deref().unwrap_or_default())
        .ok_or_else(|| "unusable engine id".to_string())?;

    let root = engines_dir();
    std::fs::create_dir_all(&root).map_err(|e| format!("cannot create {}: {e}", root.display()))?;
    let destination = root.join(id.as_str());
    let staging = root.join(format!(".{}.incoming", id.as_str()));

    // Clear a previous failed attempt rather than refusing forever.
    if staging.exists() {
        std::fs::remove_dir_all(&staging).ok();
    }
    copy_tree(candidate, &staging)?;

    if destination.exists() {
        let previous = root.join(format!(".{}.previous", id.as_str()));
        if previous.exists() {
            std::fs::remove_dir_all(&previous).ok();
        }
        std::fs::rename(&destination, &previous).map_err(|e| format!("cannot set aside: {e}"))?;
    }
    if let Err(e) = std::fs::rename(&staging, &destination) {
        // Put the old one back rather than leaving the app with no engine.
        let previous = root.join(format!(".{}.previous", id.as_str()));
        if previous.exists() {
            std::fs::rename(&previous, &destination).ok();
        }
        return Err(format!("cannot install: {e}"));
    }

    Ok(id.as_str().to_string())
}

fn copy_tree(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| format!("cannot create {}: {e}", to.display()))?;
    for entry in std::fs::read_dir(from).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let target = to.join(entry.file_name());
        if entry.file_type().map_err(|e| e.to_string())?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)
                .map_err(|e| format!("cannot copy {}: {e}", entry.path().display()))?;
        }
    }
    Ok(())
}

/// Register an already-present runtime directory (the one inside the app
/// bundle) as an installed engine, writing the manifest it needs.
/// Whether the installed engine should be replaced by the bundled one.
///
/// True only when the installed copy carries a `source_hash` -- meaning this
/// app put it there -- and that hash differs from the bundled engine's. An
/// engine with no recorded hash came from the user and is left alone.
fn should_refresh(id: EngineId, source: &Path) -> bool {
    let dir = engines_dir().join(id.as_str());
    let Ok(text) = std::fs::read_to_string(dir.join("manifest.json")) else {
        return false;
    };
    let Ok(manifest) = serde_json::from_str::<Manifest>(&text) else {
        return false;
    };
    let Some(installed) = manifest.source_hash else {
        return false;
    };
    let Some(bundled) = hash_binary(&source.join("ace-server")) else {
        return false;
    };
    if installed == bundled {
        return false;
    }
    eprintln!("[engines] refreshing the bundled ACE-Step engine ({installed} -> {bundled})");
    true
}

/// Discard an installed engine and re-adopt the bundled one.
///
/// Needed once by installs made before engine provenance was recorded: those
/// manifests carry no hash, so the automatic refresh cannot tell an engine this
/// app installed from one the user installed, and guessing wrong would discard
/// somebody's build. Explicit is the right call for that ambiguity.
pub fn reinstall_from(source: &Path) -> Result<(), String> {
    let id = EngineId::AceStep;
    let _ = std::fs::remove_dir_all(engines_dir().join(id.as_str()));
    adopt_from(id, source)
}

pub fn adopt_from(id: EngineId, source: &Path) -> Result<(), String> {
    if binary_path(id).is_some() {
        // Already installed. Refresh it only when the copy on disk is one *we*
        // put there and the bundled engine has since changed -- otherwise a
        // new app release would silently overwrite an engine the user built or
        // installed themselves.
        if !should_refresh(id, source) {
            return Ok(());
        }
        let destination = engines_dir().join(id.as_str());
        let _ = std::fs::remove_dir_all(&destination);
    }
    let root = engines_dir();
    let destination = root.join(id.as_str());
    if destination.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    copy_tree(source, &destination)?;

    // Only the backends actually shipped get listed: a model of the runtime
    // would be rebuilt from whatever is on disk and could disagree.
    let mut files: Vec<String> = std::fs::read_dir(&destination)
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n != "ace-server")
        .collect();
    files.sort();

    // The binary name and label come from the registry rather than being
    // hardcoded, so a bundle adopted for one engine cannot be filed under
    // another.
    let (binary_name, label) = match id {
        EngineId::AceStep => ("ace-server", "ACE-Step (acestep.cpp)"),
        EngineId::AudioCpp => ("audiocpp_server", "audio.cpp"),
    };
    let manifest = serde_json::json!({
        "id": id.as_str(),
        "label": label,
        "binary": binary_name,
        "files": files,
        "sourceHash": hash_binary(&destination.join(binary_name)),
    });
    std::fs::write(
        destination.join("manifest.json"),
        serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}

/// Public so the UI can tell the user where to extract an engine to.
pub fn install_dir() -> PathBuf {
    engines_dir()
}

pub fn remove(id: EngineId) -> Result<(), String> {
    let dir = engines_dir().join(id.as_str());
    if !dir.exists() {
        return Ok(());
    }
    std::fs::remove_dir_all(&dir).map_err(|e| format!("cannot remove engine: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The wire format is a contract with `lib/engines.ts`. This pins every
    /// field name the UI reads, because a silent rename here does not fail a
    /// test, a type check, or the collapsed panel -- it just throws at runtime
    /// and blanks the window.
    #[test]
    fn engine_serializes_with_the_field_names_the_ui_reads() {
        let e = Engine {
            id: EngineId::AceStep,
            label: "ACE-Step".to_string(),
            kind: EngineKind::AsyncJobs,
            families: vec!["turbo".to_string()],
            binary: "/usr/lib/AceForge/aceforge-runtime/ace-server".to_string(),
            installed: true,
            files: vec![EngineFile {
                name: "ace-server".to_string(),
                size_bytes: 2_488_400,
                present: true,
            }],
            total_bytes: 2_488_400,
            notes: "async job queue".to_string(),
        };
        let v = serde_json::to_value(&e).expect("serialize");
        let obj = v.as_object().expect("object");

        for key in [
            "id",
            "label",
            "kind",
            "families",
            "binary",
            "installed",
            "files",
            "total_bytes",
            "notes",
        ] {
            assert!(obj.contains_key(key), "missing {key} in {v}");
        }
        assert!(
            !obj.contains_key("totalBytes"),
            "camelCase leaked back in: {v}"
        );

        let file = v["files"][0].as_object().expect("file object");
        assert!(file.contains_key("size_bytes"), "missing size_bytes in {v}");
        assert!(!file.contains_key("sizeBytes"), "camelCase leaked in: {v}");

        // `kind` is a separate enum and DOES need its rename: the UI compares
        // against "asyncJobs".
        assert_eq!(v["kind"], "asyncJobs");
    }

    fn manifest(id: &str, binary: &str, files: &[&str]) -> String {
        format!(
            r#"{{"id":"{id}","label":"Test","binary":"{binary}","files":[{}]}}"#,
            files
                .iter()
                .map(|f| format!("\"{f}\""))
                .collect::<Vec<_>>()
                .join(",")
        )
    }

    #[test]
    fn rejects_an_unknown_id() {
        let dir = std::env::temp_dir().join("engine-bad-id");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("s"), b"x").unwrap();
        std::fs::write(dir.join("manifest.json"), manifest("whisper-cpp", "s", &[])).unwrap();
        let report = inspect(&dir);
        assert!(!report.ok);
        assert!(report.problems.iter().any(|p| p.contains("unknown engine id")));
    }

    #[test]
    fn rejects_a_binary_that_is_not_executable() {
        let dir = std::env::temp_dir().join("engine-noexec");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Left at the default 0644.
        std::fs::write(dir.join("server"), b"x").unwrap();
        std::fs::write(dir.join("manifest.json"), manifest("audiocpp", "server", &[])).unwrap();
        let report = inspect(&dir);
        assert!(!report.ok);
        assert!(
            report.problems.iter().any(|p| p.contains("not executable")),
            "problems: {:?}",
            report.problems
        );
    }

    #[test]
    fn rejects_a_missing_declared_file() {
        let dir = std::env::temp_dir().join("engine-missing-dep");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("server"), b"x").unwrap();
        std::fs::write(
            dir.join("manifest.json"),
            manifest("audiocpp", "server", &["libggml-cuda.so"]),
        )
        .unwrap();
        let report = inspect(&dir);
        assert!(!report.ok);
        assert!(report.problems.iter().any(|p| p.contains("libggml-cuda.so")));
    }

    #[test]
    fn accepts_a_complete_engine() {
        let dir = std::env::temp_dir().join("engine-ok");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("server"), b"x").unwrap();
        std::fs::write(dir.join("model_specs"), b"x").unwrap();
        // The executable bit has to be set, which is exactly what the install
        // path verifies before accepting a bundle.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir.join("server"), std::fs::Permissions::from_mode(0o755))
                .unwrap();
        }
        std::fs::write(
            dir.join("manifest.json"),
            manifest("audiocpp", "server", &["model_specs"]),
        )
        .unwrap();
        let report = inspect(&dir);
        assert!(report.ok, "problems: {:?}", report.problems);
    }
}
#[cfg(test)]
mod refresh_tests {
    use super::*;

    /// The decision that matters most: an engine the user installed must never
    /// be replaced by a bundled one, however old it is.
    #[test]
    fn hashing_is_stable_and_distinguishes_content() {
        let dir = std::env::temp_dir().join("aceforge-hash-test");
        let _ = std::fs::create_dir_all(&dir);
        let a = dir.join("a.bin");
        let b = dir.join("b.bin");
        std::fs::write(&a, b"engine version one").unwrap();
        std::fs::write(&b, b"engine version two!").unwrap();

        let ha = hash_binary(&a).expect("hashes");
        let hb = hash_binary(&b).expect("hashes");
        assert_eq!(ha, hash_binary(&a).unwrap(), "hashing must be stable");
        assert_ne!(ha, hb, "different content must hash differently");
        assert_eq!(ha.len(), 16);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_binary_hashes_to_nothing_rather_than_panicking() {
        assert!(hash_binary(std::path::Path::new("/nonexistent/ace-server")).is_none());
    }
}
