//! User-defined parameter presets.
//!
//! Distinct from `presets::all()`, which is the curated set shipped with the
//! app and never changes. These are the bundles a user saves from whatever the
//! form currently holds, so the storage has to survive an upgrade and survive
//! a parameter being added later -- hence storing whole `GenerateParams`
//! values rather than a field list.

use serde::{Deserialize, Serialize};

use crate::generate::GenerateParams;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserPreset {
    pub id: String,
    pub label: String,
    /// The full parameter set to restore.
    pub params: GenerateParams,
    /// Epoch seconds, for ordering the list.
    pub created_at: u64,
}

fn store_path() -> std::path::PathBuf {
    crate::paths::app_data_dir().join("param_presets.json")
}

/// A missing or unreadable file is an empty list, never an error: losing saved
/// presets must not stop the app from starting.
fn load_from(path: &std::path::Path) -> Vec<UserPreset> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

fn save_to(path: &std::path::Path, all: &[UserPreset]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(all).map_err(|e| e.to_string())?;
    std::fs::write(path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

fn load() -> Vec<UserPreset> {
    load_from(&store_path())
}

fn save(all: &[UserPreset]) -> Result<(), String> {
    save_to(&store_path(), all)
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Slug for the filename-free id. Presets are addressed by id, so it only has
/// to be stable and unique within the list.
pub fn slugify_id(label: &str) -> String {
    let mut out = String::new();
    for ch in label.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "preset".to_string()
    } else {
        trimmed
    }
}

pub fn list() -> Vec<UserPreset> {
    let mut all = load();
    all.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    all
}

/// Save the current form as a named preset. Re-using an existing label
/// overwrites it rather than creating a near-duplicate, because "turbo fast"
/// typed twice should stay one entry.
pub fn save_preset(label: &str, params: GenerateParams) -> Result<UserPreset, String> {
    let label = label.trim();
    if label.is_empty() {
        return Err("name your preset first".to_string());
    }
    let mut all = load();
    let id = slugify_id(label);

    let mut preset = UserPreset {
        id: id.clone(),
        label: label.to_string(),
        params,
        created_at: now(),
    };

    match all.iter_mut().find(|p| p.id == id) {
        Some(existing) => {
            // Keep the original timestamp so overwriting does not float the
            // entry to the top and shuffle the list under the cursor.
            preset.created_at = existing.created_at;
            *existing = preset.clone();
        }
        None => all.push(preset.clone()),
    }

    save(&all)?;
    Ok(preset)
}

pub fn delete(id: &str) -> Result<(), String> {
    let mut all = load();
    let before = all.len();
    all.retain(|p| p.id != id);
    if all.len() == before {
        return Err(format!("no preset with id {id}"));
    }
    save(&all)
}

/// Fill in fields a preset does not carry.
///
/// Older files predate parameters that were added later; deserialising them
/// leaves those fields at their default rather than absent, so this only has to
/// cover the future case where a field is genuinely missing.
pub fn merged(preset: &UserPreset, current: &GenerateParams) -> GenerateParams {
    let mut out = preset.params.clone();
    // A preset should never repaint or cover by accident: those need source
    // audio the user is not pointing at from a saved bundle.
    out.source_audio = String::new();
    if out.task_type != "text2music" {
        out.task_type = "text2music".to_string();
    }
    out.repainting_start = 0.0;
    out.repainting_end = -1.0;
    // The caption is what the user was thinking about, not part of the
    // settings; restoring it would overwrite the one they are about to type.
    out.caption = current.caption.clone();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Store(std::path::PathBuf);

    impl Store {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "aceforge-userpresets-{tag}-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("scratch");
            Self(dir.join("param_presets.json"))
        }
    }

    impl Drop for Store {
        fn drop(&mut self) {
            if let Some(parent) = self.0.parent() {
                let _ = std::fs::remove_dir_all(parent);
            }
        }
    }

    fn params(steps: i32) -> GenerateParams {
        GenerateParams {
            caption: "saved caption".to_string(),
            inference_steps: steps,
            ..Default::default()
        }
    }

    /// Round-trips through the on-disk store, so the field names the UI reads
    /// are pinned: a mismatch here deserialises as `undefined` and sorts
    /// nothing, silently.
    #[test]
    fn user_preset_uses_camel_case_wire_names() {
        let p = UserPreset {
            id: "x".to_string(),
            label: "X".to_string(),
            params: params(8),
            created_at: 7,
        };
        let v = serde_json::to_value(&p).expect("serialize");
        assert!(v.get("createdAt").is_some(), "missing createdAt in {v}");
        assert!(v.get("created_at").is_none(), "snake_case leaked: {v}");
        assert!(v.get("params").is_some());
        let back: UserPreset = serde_json::from_value(v).expect("round trip");
        assert_eq!(back.created_at, 7);
    }

    #[test]
    fn ids_are_slugified() {
        assert_eq!(slugify_id("Turbo Fast"), "turbo-fast");
        assert_eq!(slugify_id("  spaced  out  "), "spaced-out");
        assert_eq!(slugify_id("!!!"), "preset");
        assert_eq!(slugify_id("Q4_K_M only"), "q4-k-m-only");
    }

    /// The whole point: what the user saved comes back exactly.
    #[test]
    fn presets_round_trip_through_the_store() {
        let store = Store::new("roundtrip");
        assert!(load_from(&store.0).is_empty(), "starts empty");

        let written = UserPreset {
            id: "turbo-fast".to_string(),
            label: "Turbo Fast".to_string(),
            params: params(8),
            created_at: 42,
        };
        save_to(&store.0, std::slice::from_ref(&written)).expect("save");

        let back = load_from(&store.0);
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].label, "Turbo Fast");
        assert_eq!(back[0].params.inference_steps, 8);
        assert_eq!(back[0].params.caption, "saved caption");
    }

    /// Saving under the same name twice must update, not accumulate
    /// near-duplicates the user then has to tell apart.
    #[test]
    fn saving_the_same_label_twice_updates_in_place() {
        let store = Store::new("update");
        let first = UserPreset {
            id: "x".to_string(),
            label: "X".to_string(),
            params: params(8),
            created_at: 100,
        };
        let mut all = vec![first.clone()];
        all[0].params.inference_steps = 25;
        all[0].created_at = 200;
        save_to(&store.0, &all).expect("save");

        let back = load_from(&store.0);
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].params.inference_steps, 25);
    }

    /// A half-written or hand-edited file must read as empty, not crash the app
    /// on startup.
    #[test]
    fn a_corrupt_store_reads_as_empty() {
        let store = Store::new("corrupt");
        std::fs::write(&store.0, b"{ not json").expect("write");
        assert!(load_from(&store.0).is_empty());
    }

    #[test]
    fn an_absent_store_reads_as_empty() {
        let store = Store::new("absent");
        assert!(load_from(&store.0).is_empty());
    }

    /// Merging keeps the caption the user is typing and refuses to restore
    /// source-audio state, which would point a saved preset at a track that
    /// may since have been deleted.
    #[test]
    fn merging_keeps_the_caption_but_drops_the_source() {
        let mut current = GenerateParams::default();
        current.caption = "what I am typing".to_string();

        let mut stored = params(25);
        stored.task_type = "repaint".to_string();
        stored.source_audio = "/library/old.mp3".to_string();
        stored.repainting_start = 3.0;
        stored.repainting_end = 9.0;

        let preset = UserPreset {
            id: "p".to_string(),
            label: "P".to_string(),
            params: stored,
            created_at: 0,
        };
        let merged = super::merged(&preset, &current);

        assert_eq!(merged.inference_steps, 25, "the preset's settings apply");
        assert_eq!(merged.caption, "what I am typing", "the caption is theirs");
        assert!(merged.source_audio.is_empty(), "no stale source path");
        assert_eq!(merged.task_type, "text2music", "no accidental repaint");
        assert_eq!(merged.repainting_start, 0.0);
        assert_eq!(merged.repainting_end, -1.0);
    }
}
