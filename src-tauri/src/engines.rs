//! Engine registry.
//!
//! Two runtimes, genuinely different shapes:
//!
//! * `acestep` — `ace-server`. Async job queue: submit returns an id, the
//!   client polls, and a job can be cancelled.
//! * `hotstep` — `ace-server`, from HOT-Step CPP. A further-developed fork of
//!   the same codebase: same binary name, same HTTP surface, a different ggml
//!   and forty more request fields. Kept in its own directory for that reason.
//!
//! A third-party engine that blocks for the whole render has no cancellation: a
//! wedged GPU call cannot be interrupted from userspace. Upstream documents
//! music generation as taking minutes, so its busy timeout has to be raised well
//! past the 5 minute
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
/// silently failed every frontend comparison against "acestep"/"hotstep".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EngineId {
    #[serde(rename = "acestep")]
    AceStep,
    #[serde(rename = "hotstep")]
    HotStep,
}

impl EngineId {
    pub fn as_str(self) -> &'static str {
        match self {
            EngineId::AceStep => "acestep",
            EngineId::HotStep => "hotstep",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "acestep" => Some(EngineId::AceStep),
            "hotstep" => Some(EngineId::HotStep),
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
    ///
    /// The rename is load-bearing and was missing for a long time. The manifest
    /// is written with serde_json's own key, `sourceHash`, while this field was
    /// left as `source_hash` with no rename -- so every read found `None`, the
    /// refresh check bailed on its first line, and the bundled engine was never
    /// refreshed automatically for anyone, silently, on any platform.
    #[serde(default, rename = "sourceHash")]
    source_hash: Option<String>,
}

fn engines_dir() -> PathBuf {
    crate::paths::app_data_dir().join("engines")
}

/// Files bigger than this are fingerprinted by name and length rather than by
/// content. The 113 MB CUDA library would otherwise be read in full on every
/// launch to learn nothing, and a change to it is not what this hash is for.
const FINGERPRINT_MAX_READ: u64 = 8 * 1024 * 1024;

fn fnv(hash: u64, bytes: &[u8]) -> u64 {
    let mut hash = hash;
    for b in bytes {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Fingerprint a whole engine directory, not just its binary.
///
/// HOT-Step resolves its sampler out of the Lua plugins sitting beside
/// `ace-server`, and those started shipping in a release *after* the binary
/// they belong to. Hashing the binary alone therefore reported "unchanged" on
/// every install that already had one, `should_refresh` declined to refresh, the
/// plugins were never copied, and the engine logged "Loaded 0 solvers" with
/// nothing anywhere saying why.
///
/// Relative paths are folded in, so a file that moves counts as a change even
/// when its bytes do not, and entries are visited in sorted order so the result
/// never depends on the order the filesystem happens to hand back.
fn hash_runtime(dir: &Path) -> Option<String> {
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    collect_files(dir, dir, &mut files).ok()?;
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for (rel, path) in &files {
        hash = fnv(hash, rel.as_bytes());
        hash = fnv(hash, b"\0");
        let meta = std::fs::metadata(path).ok()?;
        if meta.len() > FINGERPRINT_MAX_READ {
            hash = fnv(hash, &meta.len().to_le_bytes());
        } else {
            hash = fnv(hash, &std::fs::read(path).ok()?);
        }
        hash = fnv(hash, b"\0");
    }
    Some(format!("{hash:016x}"))
}

fn collect_files(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            collect_files(root, &path, out)?;
        } else {
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .into_owned();
            out.push((rel, path));
        }
    }
    Ok(())
}

fn describe(id: EngineId, binary_name: &str, families: &[&str], notes: &str) -> Engine {
    let (label, kind) = match id {
        EngineId::AceStep => (
            "ACE-Step (acestep.cpp)",
            EngineKind::AsyncJobs,
        ),
        EngineId::HotStep => ("HOT-Step CPP", EngineKind::AsyncJobs),
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
        // Presence, not file-ness. An entry naming a directory is satisfied by
        // the directory: HOT-Step's manifest lists `plugins`, and requiring a
        // directory to be a file is how an engine with all of its payload
        // present came to be reported as "not installed". A manifest written by
        // an older build still reads correctly this way.
        let deps_ok = m.files.iter().all(|f| dir.join(f).exists());
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
            EngineId::HotStep,
            "ace-server",
            &[
                "ACE-Step 1.5 (turbo, SFT, XL, plus PP-VAE and ScragVAE decoders)",
                "MiniMax-Music3",
                "YuE2 (AR composer plus NAR renderer, own adapter slots)",
            ],
            "A further-developed acestep.cpp. Same HTTP surface as the bundled \
             engine, so progress and cancel work exactly as they do there, but \
             it carries extra solvers, schedulers, guidance modes, adapters and \
             about forty more request fields. Needs its own models folder.",
        ),
    ]
}

pub fn binary_path(id: EngineId) -> Option<PathBuf> {
    binary_path_in(&engines_dir(), id)
}

/// The install lookup, with the root passed in.
///
/// The engine directory is process-wide, and a test that pointed it somewhere
/// scratch through an environment variable raced every other test in the
/// binary that touched it. Passing it in keeps the logic testable and the
/// global in one place.
fn binary_path_in(root: &Path, id: EngineId) -> Option<PathBuf> {
    let dir = root.join(id.as_str());
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
            "unknown engine id {:?}: expected \"acestep\" or \"hotstep\"",
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
fn should_refresh_in(root: &Path, id: EngineId, source: &Path) -> bool {
    let dir = root.join(id.as_str());
    let Ok(text) = std::fs::read_to_string(dir.join("manifest.json")) else {
        return false;
    };
    let Ok(manifest) = serde_json::from_str::<Manifest>(&text) else {
        return false;
    };
    let Some(installed) = manifest.source_hash else {
        return false;
    };
    let Some(bundled) = hash_runtime(source) else {
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
    adopt_into(&engines_dir(), id, source)
}

fn adopt_into(root: &Path, id: EngineId, source: &Path) -> Result<(), String> {
    let destination = root.join(id.as_str());
    if binary_path_in(root, id).is_some() {
        // Already installed. Refresh it only when the copy on disk is one *we*
        // put there and the bundled engine has since changed -- otherwise a
        // new app release would silently overwrite an engine the user built or
        // installed themselves.
        if !should_refresh_in(root, id, source) {
            return Ok(());
        }
        let _ = std::fs::remove_dir_all(&destination);
    } else if destination.exists() {
        // A directory with no manifest is not an install. `binary_path` cannot
        // see it, so treating it as one left the app reporting "not installed"
        // against a folder full of files, and refusing to copy over it -- a
        // half-finished adopt, or an interrupted refresh, with nothing here
        // able to repair it. Either way nothing of value is recorded, so it is
        // cleared and adopted properly.
        let _ = std::fs::remove_dir_all(&destination);
    }
    std::fs::create_dir_all(root).map_err(|e| e.to_string())?;
    copy_tree(source, &destination)?;

    // Only regular files get listed, and only the backends actually shipped: a
    // model of the runtime would be rebuilt from whatever is on disk and could
    // disagree. Directories are structure rather than dependencies -- HOT-Step
    // ships a `plugins/` tree, and listing it here made every install report
    // itself broken, because the check below was asking whether a directory was
    // a file.
    let mut files: Vec<String> = std::fs::read_dir(&destination)
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n != "ace-server")
        .collect();
    files.sort();

    // The binary name and label come from the registry rather than being
    // hardcoded, so a bundle adopted for one engine cannot be filed under
    // another.
    let (binary_name, label) = match id {
        EngineId::AceStep => ("ace-server", "ACE-Step (acestep.cpp)"),
        // Same binary name as the other engine on purpose: HOT-Step ships an
        // `ace-server` too, and the manifest plus the destination directory are
        // what keep the two apart.
        EngineId::HotStep => ("ace-server", "HOT-Step CPP"),
    };
    let manifest = serde_json::json!({
        "id": id.as_str(),
        "label": label,
        "binary": binary_name,
        "files": files,
        "sourceHash": hash_runtime(&destination),
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
        std::fs::write(dir.join("manifest.json"), manifest("hotstep", "server", &[])).unwrap();
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
            manifest("hotstep", "server", &["libggml-cuda.so"]),
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
            manifest("hotstep", "server", &["model_specs"]),
        )
        .unwrap();
        let report = inspect(&dir);
        assert!(report.ok, "problems: {:?}", report.problems);
    }
}
#[cfg(test)]
mod adopt_tests {
    use super::*;

    /// Scratch engines directory. Passed in explicitly rather than set through
    /// `ACEFORGE_DATA_DIR`: that variable is process-wide, so a test writing it
    /// races every other test in the binary that reads it -- including the queue
    /// suite, which points it at its own folder exactly once.
    fn scratch(label: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("aceforge-adopt-{}-{label}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("engines")).unwrap();
        dir
    }

    fn bundle(root: &Path, body: &str) -> PathBuf {
        let src = root.join("bundle");
        std::fs::create_dir_all(src.join("plugins/solvers")).unwrap();
        std::fs::write(src.join("ace-server"), body).unwrap();
        std::fs::write(src.join("plugins/solvers/one.lua"), "return 1").unwrap();
        src
    }

    /// A plain adopt installs the engine and records it.
    #[test]
    fn a_first_adopt_installs_and_records_the_engine() {
        let data = scratch("first");
        let src = bundle(&data, "engine one");
        let root = data.join("engines");
        adopt_into(&root, EngineId::HotStep, &src).expect("adopt");

        assert!(binary_path_in(&root, EngineId::HotStep).is_some(), "not recorded");
        assert_eq!(
            std::fs::read_to_string(root.join("hotstep/plugins/solvers/one.lua")).unwrap(),
            "return 1",
            "nested files must come across, not just the top level"
        );
        let _ = std::fs::remove_dir_all(&data);
    }

    /// The state a manual copy or an interrupted refresh leaves behind: the
    /// directory is full of files and has no manifest. `binary_path` returns
    /// None, so the app reports the engine as not installed -- and because the
    /// directory exists, an adopt that trusted it would refuse to copy or record
    /// anything, leaving nothing able to repair the folder.
    #[test]
    fn a_directory_without_a_manifest_is_repaired_rather_than_trusted() {
        let data = scratch("orphan");
        let src = bundle(&data, "engine one");
        let root = data.join("engines");
        let destination = root.join("hotstep");
        std::fs::create_dir_all(&destination).unwrap();
        std::fs::write(destination.join("ace-server"), "half-copied").unwrap();
        assert!(binary_path_in(&root, EngineId::HotStep).is_none(), "precondition");

        adopt_into(&root, EngineId::HotStep, &src).expect("adopt");

        assert!(
            binary_path_in(&root, EngineId::HotStep).is_some(),
            "the engine is still unrecorded after an adopt"
        );
        assert_eq!(
            std::fs::read_to_string(destination.join("ace-server")).unwrap(),
            "engine one",
            "the stranded copy must be replaced, not adopted as-is"
        );
        assert!(
            destination.join("plugins/solvers/one.lua").is_file(),
            "repair must install the payload, not just the manifest"
        );
        let _ = std::fs::remove_dir_all(&data);
    }

    /// An engine this app installed is left alone when the bundled copy has not
    /// moved on. The whole point of the fingerprint.
    #[test]
    fn an_up_to_date_install_is_not_touched_again() {
        let data = scratch("stable");
        let src = bundle(&data, "engine one");
        let root = data.join("engines");
        adopt_into(&root, EngineId::HotStep, &src).expect("first adopt");
        let binary = root.join("hotstep/ace-server");
        std::fs::write(&binary, "locally touched").unwrap();

        adopt_into(&root, EngineId::HotStep, &src).expect("second adopt");

        assert_eq!(
            std::fs::read_to_string(&binary).unwrap(),
            "locally touched",
            "an unchanged bundle must not overwrite the installed engine"
        );
        let _ = std::fs::remove_dir_all(&data);
    }

    /// A changed bundle does replace the install -- this is what carries a new
    /// plugin set across an upgrade.
    #[test]
    fn a_changed_bundle_replaces_the_install() {
        let data = scratch("changed");
        let src = bundle(&data, "engine one");
        let root = data.join("engines");
        adopt_into(&root, EngineId::HotStep, &src).expect("first adopt");
        std::fs::write(src.join("ace-server"), "engine two").unwrap();
        std::fs::write(src.join("plugins/solvers/two.lua"), "return 2").unwrap();

        eprintln!("DBG src hash: {:?}", hash_runtime(&src));
        eprintln!("DBG should_refresh: {}", should_refresh_in(&root, EngineId::HotStep, &src));
        adopt_into(&root, EngineId::HotStep, &src).expect("second adopt");

        let installed = root.join("hotstep");
        assert_eq!(std::fs::read_to_string(installed.join("ace-server")).unwrap(), "engine two");
        assert!(installed.join("plugins/solvers/two.lua").is_file(), "new plugin must arrive");
        let _ = std::fs::remove_dir_all(&data);
    }

    /// The manifest is written with serde_json's own keys and read back into a
    /// struct, so every key has to be spelled the same way on both sides. The
    /// fingerprint was written as `sourceHash` and read as `source_hash` for the
    /// whole life of the refresh feature, which meant it was always absent and
    /// the refresh never once ran. Nothing failed; it just declined to do
    /// anything, forever.
    #[test]
    fn the_manifest_records_its_fingerprint_under_a_key_it_can_read_back() {
        let data = scratch("wire");
        let src = bundle(&data, "engine one");
        let root = data.join("engines");
        adopt_into(&root, EngineId::HotStep, &src).expect("adopt");

        let written: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(root.join("hotstep/manifest.json")).unwrap(),
        )
        .expect("manifest parses");
        assert!(
            written.get("sourceHash").and_then(|v| v.as_str()).is_some(),
            "the fingerprint must be written as sourceHash: {written}"
        );

        let read: Manifest =
            serde_json::from_value(written).expect("manifest deserialises");
        assert!(
            read.source_hash.is_some(),
            "the fingerprint must survive the round trip"
        );
        let _ = std::fs::remove_dir_all(&data);
    }

    /// The manifest must describe files, not structure. HOT-Step ships a
    /// `plugins/` tree, and listing that directory among the required files is
    /// what made the engine report "not installed" while sitting on a complete,
    /// working copy of itself.
    #[test]
    fn the_manifest_does_not_list_directories_as_required_files() {
        let data = scratch("dirs");
        let src = bundle(&data, "engine one");
        let root = data.join("engines");
        adopt_into(&root, EngineId::HotStep, &src).expect("adopt");

        let written: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(root.join("hotstep/manifest.json")).unwrap(),
        )
        .expect("manifest parses");
        let listed: Vec<String> = written["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert!(
            !listed.iter().any(|f| f.contains('/') || f == "plugins"),
            "directories must not be listed as dependencies: {listed:?}"
        );
        let _ = std::fs::remove_dir_all(&data);
    }

    /// And a manifest that lists one anyway -- every install from before this
    /// fix has exactly that -- must still count as installed. The dependency
    /// check asks whether the path is present, so a directory satisfies it and
    /// those installs recover without needing another re-adopt.
    #[test]
    fn a_manifest_naming_a_directory_still_counts_as_installed() {
        let data = scratch("legacy");
        let src = bundle(&data, "engine one");
        let root = data.join("engines");
        adopt_into(&root, EngineId::HotStep, &src).expect("adopt");
        let manifest_path = root.join("hotstep/manifest.json");

        let mut written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&manifest_path).unwrap()).unwrap();
        written["files"].as_array_mut().unwrap().push(serde_json::json!("plugins"));
        std::fs::write(&manifest_path, written.to_string()).unwrap();

        let read: Manifest =
            serde_json::from_str(&std::fs::read_to_string(&manifest_path).unwrap()).unwrap();
        let dir = root.join("hotstep");
        let deps_ok = read.files.iter().all(|f| dir.join(f).exists());
        assert!(deps_ok, "a directory entry must not read as a missing file");
        let _ = std::fs::remove_dir_all(&data);
    }

    /// An engine with no recorded hash came from the user and must survive even
    /// when the bundled copy has moved on.
    #[test]
    fn an_engine_with_no_recorded_hash_is_left_alone() {
        let data = scratch("foreign");
        let src = bundle(&data, "engine one");
        let root = data.join("engines");
        let destination = root.join("hotstep");
        std::fs::create_dir_all(&destination).unwrap();
        std::fs::write(destination.join("ace-server"), "user build").unwrap();
        // A manifest with no sourceHash: installed by hand, or by a build of
        // this app old enough not to record one.
        std::fs::write(
            destination.join("manifest.json"),
            r#"{"id":"hotstep","label":"HOT-Step CPP","binary":"ace-server","files":[]}"#,
        )
        .unwrap();

        assert!(binary_path_in(&root, EngineId::HotStep).is_some(), "precondition");
        std::fs::write(src.join("ace-server"), "bundled two").unwrap();
        adopt_into(&root, EngineId::HotStep, &src).expect("adopt");

        assert_eq!(
            std::fs::read_to_string(destination.join("ace-server")).unwrap(),
            "user build",
            "an engine of unknown provenance must never be overwritten"
        );
        let _ = std::fs::remove_dir_all(&data);
    }
}

#[cfg(test)]
mod refresh_tests {
    use super::*;

    /// A missing runtime fingerprints to nothing rather than panicking, so a
    /// half-copied bundle reads as "do not refresh" instead of taking the app
    /// down on launch.
    #[test]
    fn a_missing_runtime_hashes_to_nothing_rather_than_panicking() {
        assert!(hash_runtime(std::path::Path::new("/nonexistent/ace-server")).is_none());
    }

    /// A scratch runtime. The label is part of the path: these tests run in
    /// parallel and a shared directory would have them delete each other's
    /// fixtures mid-run, which fails as an intermittent nonsense assertion.
    fn runtime_with_binary(label: &str, body: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("aceforge-runtime-hash-{}-{label}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("plugins/solvers")).unwrap();
        std::fs::write(dir.join("ace-server"), body).unwrap();
        dir
    }

    /// The regression. HOT-Step's plugins shipped in a release whose
    /// `ace-server` was byte-identical to the one before it, so a binary-only
    /// fingerprint called every existing install unchanged, the refresh was
    /// skipped, and the engine started up with no solvers and no explanation.
    /// Adding a plugin has to register as a change in the bundled runtime.
    #[test]
    fn adding_a_plugin_file_changes_the_runtime_fingerprint() {
        let dir = runtime_with_binary("added", "same binary both times");
        let before = hash_runtime(&dir).expect("hashes");

        std::fs::create_dir_all(dir.join("plugins/solvers")).unwrap();
        std::fs::write(dir.join("plugins/solvers/md_confluence_v4.lua"), "return {}").unwrap();
        let after = hash_runtime(&dir).expect("hashes");

        assert_ne!(before, after, "a new plugin must force a refresh");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The mirror image, and the one that keeps a refresh from happening on
    /// every launch: an unchanged tree must hash to the same value, however
    /// the filesystem orders its directory entries.
    #[test]
    fn an_unchanged_runtime_is_stable() {
        let dir = runtime_with_binary("stable-order", "stable");
        std::fs::write(dir.join("plugins/solvers/a.lua"), "a").unwrap();
        std::fs::write(dir.join("plugins/solvers/b.lua"), "bb").unwrap();
        let first = hash_runtime(&dir).expect("hashes");
        assert_eq!(first, hash_runtime(&dir).expect("hashes"));
        assert_eq!(first.len(), 16);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Editing a plugin's contents counts too, not just its arrival.
    #[test]
    fn editing_a_plugin_changes_the_fingerprint() {
        let dir = runtime_with_binary("stable-edit", "stable");
        let path = dir.join("plugins/solvers/a.lua");
        std::fs::write(&path, "version one").unwrap();
        let before = hash_runtime(&dir).expect("hashes");
        std::fs::write(&path, "version two").unwrap();
        assert_ne!(before, hash_runtime(&dir).expect("hashes"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Large files are fingerprinted by name and length. Otherwise every launch
    /// would read 113 MB of CUDA library to reach the same answer.
    #[test]
    fn a_large_file_is_fingerprinted_without_reading_it() {
        let dir = std::env::temp_dir().join("aceforge-runtime-hash-large");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let big = dir.join("libggml-cuda.so");
        let file = std::fs::File::create(&big).unwrap();
        file.set_len(FINGERPRINT_MAX_READ + 1).unwrap();
        drop(file);

        let by_length = hash_runtime(&dir).expect("hashes");
        assert_eq!(by_length.len(), 16);
        // Truncating it changes the length, which is all this path reads.
        assert_ne!(by_length, hash_runtime_after_truncate(&dir, &big));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn hash_runtime_after_truncate(dir: &Path, file: &Path) -> String {
        let f = std::fs::OpenOptions::new().write(true).open(&file).unwrap();
        f.set_len(FINGERPRINT_MAX_READ / 2).unwrap();
        drop(f);
        hash_runtime(dir).expect("hashes")
    }
}
