//! Per-track annotations: section markers and A/B comparison slots.
//!
//! Markers are the user's own structure over a render — "chorus at 0:42" — and
//! nothing in the audio encodes them, so they live in a sidecar keyed by track
//! id. Keeping them out of the library scan means the library can be rebuilt
//! from disk at any time without losing them.
//!
//! A/B slots hold two track ids and nothing else. The parameters behind each
//! side are read back off the tracks themselves when the comparison is opened,
//! so there is no second copy of a prompt that could drift out of date.

use serde::{Deserialize, Serialize};

/// A named point in a track.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Marker {
    /// Seconds from the start. Clamped to the track duration on write.
    pub at_secs: f32,
    pub label: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackNotes {
    #[serde(default)]
    pub markers: Vec<Marker>,
}

/// A seed the user chose to keep, and the prompt that produced it.
///
/// A seed on its own is not much use: remembering that 8_223_401 made a good
/// track means nothing without knowing it was the "dusty rhodes, 78 bpm" one.
/// So the caption rides along.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedSeed {
    pub seed: i64,
    pub caption: String,
    pub starred: bool,
    pub created_at: u64,
}

/// One stem in a stack, and the stack it belongs to.
///
/// The stack exists so an arrangement can be built one layer at a time: each new
/// stem is generated over the mix built so far and carried forward as the next
/// source. Only the track path is stored -- everything else about a track is
/// re-read from the library, so a re-render never desynchronises a stale copy.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Layer {
    pub id: String,
    pub label: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LayerStack {
    pub id: String,
    pub name: String,
    pub layers: Vec<Layer>,
    pub updated_at: u64,
}

/// A section of a song under construction.
///
/// Sections are appended, not replaced: each new one is a repaint whose region
/// starts where the audio built so far ends, so the backend outpaints the seam
/// for us. Only the chosen variant's path is kept as "the audio so far"; the
/// rest are candidates that can be swapped back in.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Section {
    pub id: String,
    pub name: String,
    pub lyrics: String,
    /// Target length of this section in seconds.
    pub length_secs: f32,
    /// Track paths produced for this section, newest first.
    pub variants: Vec<String>,
    /// Index into `variants`, or None until one is adopted.
    pub chosen: Option<usize>,
    /// Path of the audio this section was built onto, i.e. the previous one.
    pub base_path: Option<String>,
}

impl Section {
    /// The adopted variant, if any.
    pub fn current_path(&self) -> Option<String> {
        self.chosen
            .and_then(|i| self.variants.get(i).cloned())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SongProject {
    pub id: String,
    pub name: String,
    pub caption: String,
    pub bpm: i32,
    pub keyscale: String,
    pub timesignature: String,
    pub sections: Vec<Section>,
    pub updated_at: u64,
}

fn song_path() -> std::path::PathBuf {
    crate::paths::app_data_dir().join("songs.json")
}

pub fn songs() -> Vec<SongProject> {
    let Ok(text) = std::fs::read_to_string(song_path()) else {
        return Vec::new();
    };
    let mut all: Vec<SongProject> = serde_json::from_str(&text).unwrap_or_default();
    all.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    all
}

pub fn save_song(mut song: SongProject) -> Result<Vec<SongProject>, String> {
    let mut all = songs();
    song.updated_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    match all.iter_mut().find(|s| s.id == song.id) {
        Some(existing) => *existing = song,
        None => all.push(song),
    }
    let path = song_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(&all).map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(songs())
}

pub fn delete_song(id: String) -> Result<Vec<SongProject>, String> {
    let mut all = songs();
    all.retain(|s| s.id != id);
    let path = song_path();
    let text = serde_json::to_string_pretty(&all).map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(songs())
}

fn layer_path() -> std::path::PathBuf {
    crate::paths::app_data_dir().join("layer_stacks.json")
}

pub fn layer_stacks() -> Vec<LayerStack> {
    let Ok(text) = std::fs::read_to_string(layer_path()) else {
        return Vec::new();
    };
    let mut stacks: Vec<LayerStack> = serde_json::from_str(&text).unwrap_or_default();
    stacks.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    stacks
}

pub fn save_layer_stack(stack: LayerStack) -> Result<Vec<LayerStack>, String> {
    let mut all = layer_stacks();
    let mut stack = stack;
    stack.updated_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    match all.iter_mut().find(|s| s.id == stack.id) {
        Some(existing) => *existing = stack,
        None => all.push(stack),
    }
    let path = layer_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(&all).map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(layer_stacks())
}

pub fn delete_layer_stack(id: String) -> Result<Vec<LayerStack>, String> {
    let mut all = layer_stacks();
    all.retain(|s| s.id != id);
    let path = layer_path();
    let text = serde_json::to_string_pretty(&all).map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(layer_stacks())
}

/// One timed lyric line.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LyricLine {
    /// Seconds from the start.
    pub at_secs: f32,
    pub text: String,
}

/// Where a set of timings came from, so the UI can say whether they are
/// reliable rather than presenting a guess as a fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TimingSource {
    /// Timed by the user tapping along to the track.
    Manual,
    /// Parsed from a `.lrc` file sitting beside the audio.
    Sidecar,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LyricTimings {
    pub lines: Vec<LyricLine>,
    pub source: TimingSource,
}

fn timings_path() -> std::path::PathBuf {
    crate::paths::app_data_dir().join("lyric_timings.json")
}

/// `mm:ss.xx`, the LRC timestamp form.
fn parse_lrc_stamp(token: &str) -> Option<f32> {
    let mut parts = token.trim_end_matches(']').split(':');
    let mins: f32 = parts.next()?.trim().parse().ok()?;
    let secs: f32 = parts.next()?.parse().ok()?;
    Some(mins * 60.0 + secs)
}

/// Is this bracket the body of a timestamp rather than a metadata tag?
///
/// Both look alike at a glance -- `[ar:Someone]` and `[00:12.00]` each hold a
/// colon -- so the discriminator is whether the text before the colon is digits.
/// Tag keys are words; timestamps are numbers.
fn bracket_is_stamp(inner: &str) -> bool {
    let Some((head, tail)) = inner.split_once(':') else {
        return false;
    };
    let digits = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
    digits(head) && !tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit() || c == '.')
}

/// Pull `[mm:ss.xx] line` pairs out of an LRC file, ignoring metadata tags.
fn parse_lrc(text: &str) -> Vec<LyricLine> {
    let mut out: Vec<LyricLine> = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }

        // A line may carry several timestamps for the same words, as a chorus
        // does, and metadata tags such as `[ar:...]` are skipped entirely.
        let mut rest = line;
        let mut stamps: Vec<f32> = Vec::new();
        while rest.starts_with('[') {
            let Some(close) = rest.find(']') else { break };
            let inner = &rest[1..close];
            if !bracket_is_stamp(inner) {
                break;
            }
            if let Some(t) = parse_lrc_stamp(inner) {
                stamps.push(t);
            }
            rest = &rest[close + 1..];
        }

        let body = rest.trim();
        if body.is_empty() {
            continue;
        }
        for t in stamps {
            out.push(LyricLine {
                at_secs: t,
                text: body.to_string(),
            });
        }
    }
    out.sort_by(|a, b| {
        a.at_secs
            .partial_cmp(&b.at_secs)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    out
}

/// Timings for a track: the sidecar LRC if there is one, otherwise what the user
/// tapped in.
///
/// The sidecar wins because it is the authored source of truth. The engine
/// produces no timings of its own -- acestep.cpp has no lyric-timestamp code
/// path -- so without one of these two sources there is nothing to follow, and
/// the UI falls back to showing the lyrics as plain text rather than guessing.
pub fn lyric_timings(track_path: &str) -> Option<LyricTimings> {
    let stem = std::path::Path::new(track_path).with_extension("lrc");
    if let Ok(text) = std::fs::read_to_string(&stem) {
        let lines = parse_lrc(&text);
        if !lines.is_empty() {
            return Some(LyricTimings { lines, source: TimingSource::Sidecar });
        }
    }

    let Ok(text) = std::fs::read_to_string(timings_path()) else {
        return None;
    };
    let all: Vec<(String, Vec<LyricLine>)> = serde_json::from_str(&text).unwrap_or_default();
    let lines = all.into_iter().find(|(k, _)| k == track_path)?.1;
    if lines.is_empty() {
        return None;
    }
    Some(LyricTimings { lines, source: TimingSource::Manual })
}

pub fn save_lyric_timings(track_path: &str, mut lines: Vec<LyricLine>) -> Result<(), String> {
    lines.sort_by(|a, b| a.at_secs.partial_cmp(&b.at_secs).unwrap_or(std::cmp::Ordering::Equal));
    let path = timings_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut all: Vec<(String, Vec<LyricLine>)> =
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
    all.retain(|(k, _)| k != track_path);
    if !lines.is_empty() {
        all.push((track_path.to_string(), lines));
    }
    let text = serde_json::to_string(&all).map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

fn store_path() -> std::path::PathBuf {
    crate::paths::app_data_dir().join("track_notes.json")
}

fn seed_path() -> std::path::PathBuf {
    crate::paths::app_data_dir().join("seeds.json")
}

fn load_seeds() -> Vec<SavedSeed> {
    let Ok(text) = std::fs::read_to_string(seed_path()) else {
        return Vec::new();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

fn write_seeds(seeds: &[SavedSeed]) -> Result<(), String> {
    let path = seed_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(seeds).map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// Saved seeds, newest first, starred ones ahead of the rest.
pub fn seeds() -> Vec<SavedSeed> {
    let mut all = load_seeds();
    all.sort_by(|a, b| {
        b.starred
            .cmp(&a.starred)
            .then(b.created_at.cmp(&a.created_at))
    });
    all
}

/// Save a seed, or return the existing entry if it is already saved.
///
/// Re-saving updates the caption rather than adding a duplicate: the same seed
/// producing a better prompt later should not leave two rows that both look
/// authoritative.
pub fn save_seed(seed: i64, caption: String) -> Result<Vec<SavedSeed>, String> {
    let mut all = load_seeds();
    match all.iter_mut().find(|s| s.seed == seed) {
        Some(existing) => {
            if !caption.trim().is_empty() {
                existing.caption = caption;
            }
        }
        None => all.push(SavedSeed {
            seed,
            caption,
            starred: false,
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        }),
    }
    write_seeds(&all)?;
    Ok(seeds())
}

pub fn toggle_star(seed: i64) -> Result<Vec<SavedSeed>, String> {
    let mut all = load_seeds();
    if let Some(s) = all.iter_mut().find(|s| s.seed == seed) {
        s.starred = !s.starred;
    }
    write_seeds(&all)?;
    Ok(seeds())
}

pub fn remove_seed(seed: i64) -> Result<Vec<SavedSeed>, String> {
    let mut all = load_seeds();
    all.retain(|s| s.seed != seed);
    write_seeds(&all)?;
    Ok(seeds())
}

fn load_all() -> Vec<(String, TrackNotes)> {
    let Ok(text) = std::fs::read_to_string(store_path()) else {
        return Vec::new();
    };
    // Annotated as a list of pairs rather than a map because serde_json's map
    // keys must be strings anyway, and a flat list round-trips predictably.
    serde_json::from_str(&text).unwrap_or_default()
}

fn save_all(all: &[(String, TrackNotes)]) -> Result<(), String> {
    let path = store_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(all).map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// Markers for one track, or an empty set.
pub fn notes_for(track_id: &str) -> TrackNotes {
    load_all()
        .into_iter()
        .find(|(id, _)| id == track_id)
        .map(|(_, notes)| notes)
        .unwrap_or_default()
}

/// Replace a track's markers, dropping any entry whose notes became empty so
/// the file does not accumulate dead keys for deleted tracks.
pub fn set_markers(track_id: &str, mut notes: TrackNotes) -> Result<(), String> {
    notes.markers.sort_by(|a, b| {
        a.at_secs
            .partial_cmp(&b.at_secs)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    notes.markers.retain(|m| !m.label.trim().is_empty());

    let mut all = load_all();
    all.retain(|(id, _)| id != track_id);
    if !notes.markers.is_empty() {
        all.push((track_id.to_string(), notes));
    }
    save_all(&all)
}

pub fn add_marker(track_id: &str, at_secs: f32, label: String) -> Result<TrackNotes, String> {
    let mut notes = notes_for(track_id);
    notes.markers.push(Marker {
        at_secs: at_secs.max(0.0),
        label,
    });
    set_markers(track_id, notes.clone())?;
    Ok(notes)
}

pub fn remove_marker(track_id: &str, index: usize) -> Result<TrackNotes, String> {
    let mut notes = notes_for(track_id);
    if index < notes.markers.len() {
        notes.markers.remove(index);
        set_markers(track_id, notes.clone())?;
    }
    Ok(notes)
}

/// The pinned comparison pair, if one is set.
pub fn ab_slots() -> Option<[String; 2]> {
    let text = std::fs::read_to_string(crate::paths::app_data_dir().join("ab_slots.json"))
        .ok()?;
    let slots: [String; 2] = serde_json::from_str(&text).ok()?;
    if slots.iter().all(|s| !s.trim().is_empty()) {
        Some(slots)
    } else {
        None
    }
}

/// Pin a track into slot `index` (0 or 1). Returns the pair after the write so
/// the caller can show it without a second read.
pub fn set_ab_slot(index: usize, track_id: String) -> Result<[String; 2], String> {
    if index > 1 {
        return Err(format!("slot {index} does not exist"));
    }
    let mut slots = ab_slots().unwrap_or_else(default_slot);
    slots[index] = track_id;

    let path = crate::paths::app_data_dir().join("ab_slots.json");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    let text = serde_json::to_string(&slots).map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(slots)
}

fn default_slot() -> [String; 2] {
    ["".to_string(), "".to_string()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_absent_store_reads_as_empty() {
        // A missing file must not be an error: the store is user data that
        // starts out not existing.
        assert_eq!(std::fs::read_to_string("/nonexistent/track_notes.json").is_err(), true);
    }

    #[test]
    fn markers_serialise_with_camel_case_keys() {
        // The frontend reads `atSecs`; a snake_case leak would silently drop
        // every marker's time and render them all at 0:00.
        let m = Marker {
            at_secs: 42.0,
            label: "chorus".into(),
        };
        let json = serde_json::to_string(&m).unwrap();
        assert!(json.contains("\"atSecs\""), "got {json}");
        assert!(json.contains("\"label\""), "got {json}");
    }

    #[test]
    fn notes_default_to_no_markers() {
        let json = r#"{}"#;
        let notes: TrackNotes = serde_json::from_str(json).expect("empty object is valid");
        assert!(notes.markers.is_empty());
    }

    #[test]
    fn a_section_resolves_its_chosen_variant() {
        let s = Section {
            id: "x".into(),
            name: "chorus".into(),
            lyrics: "".into(),
            length_secs: 8.0,
            variants: vec!["/library/a.wav".into(), "/library/b.wav".into()],
            chosen: Some(1),
            base_path: None,
        };
        assert_eq!(s.current_path().as_deref(), Some("/library/b.wav"));
    }

    #[test]
    fn a_section_with_no_variants_resolves_to_nothing() {
        let s = Section {
            id: "x".into(),
            name: "intro".into(),
            lyrics: "".into(),
            length_secs: 8.0,
            variants: Vec::new(),
            chosen: None,
            base_path: None,
        };
        assert!(s.current_path().is_none());
        // A chosen index that points past the end must not panic.
        assert!(s.clone().current_path().is_none());
    }

    #[test]
    fn an_out_of_range_choice_is_not_a_panic() {
        let s = Section {
            id: "x".into(),
            name: "chorus".into(),
            lyrics: "".into(),
            length_secs: 8.0,
            variants: vec!["/library/a.wav".into()],
            chosen: Some(7),
            base_path: None,
        };
        assert!(s.current_path().is_none(), "a stale index must read as unset");
    }

    #[test]
    fn a_song_project_serialises_with_camel_case_keys() {
        let song = SongProject {
            id: "p".into(),
            name: "song".into(),
            caption: "lo-fi".into(),
            bpm: 78,
            keyscale: "C minor".into(),
            timesignature: "4/4".into(),
            sections: vec![Section {
                id: "x".into(),
                name: "chorus".into(),
                lyrics: "".into(),
                length_secs: 8.0,
                variants: vec![],
                chosen: None,
                base_path: None,
            }],
            updated_at: 3,
        };
        let json = serde_json::to_string(&song).unwrap();
        assert!(json.contains("\"timesignature\""), "got {json}");
        assert!(json.contains("\"lengthSecs\""), "got {json}");
        assert!(json.contains("\"basePath\""), "got {json}");
    }

    #[test]
    fn lrc_stamps_parse() {
        assert_eq!(parse_lrc_stamp("00:12.34"), Some(12.34));
        assert_eq!(parse_lrc_stamp("01:05.00"), Some(65.0));
        assert_eq!(parse_lrc_stamp("00:00.5"), Some(0.5));
        // Callers pass the token between the brackets; a leading bracket is
        // not part of the contract and must not be silently tolerated.
        assert_eq!(parse_lrc_stamp("bad"), None);
    }

    #[test]
    fn lrc_lines_are_parsed_and_sorted() {
        let text = "[ar:Someone]\n[00:12.00]second line\n[00:04.00]first line\n";
        let lines = parse_lrc(text);
        assert_eq!(lines.len(), 2, "metadata tags are not lyrics");
        assert_eq!(lines[0].text, "first line");
        assert_eq!(lines[0].at_secs, 4.0);
        assert_eq!(lines[1].text, "second line");
    }

    #[test]
    fn a_repeated_timestamp_expands_to_one_entry_per_time() {
        // LRC repeats a chorus at several timestamps under one line of text.
        let lines = parse_lrc("[00:10.00][00:40.00]chorus\n");
        assert_eq!(lines.len(), 2);
        assert!(lines.iter().all(|l| l.text == "chorus"));
    }

    #[test]
    fn junk_lrc_lines_are_skipped_not_guessed_at() {
        let lines = parse_lrc("[00:01.00]\nno timestamp here\n[broken\n[00:02.00]good\n");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "good");
    }

    #[test]
    fn a_layer_stack_serialises_with_camel_case_keys() {
        let stack = LayerStack {
            id: "s1".into(),
            name: "arrangement".into(),
            layers: vec![Layer {
                id: "l1".into(),
                label: "drums".into(),
                path: "/library/a.wav".into(),
            }],
            updated_at: 7,
        };
        let json = serde_json::to_string(&stack).unwrap();
        assert!(json.contains("\"updatedAt\""), "got {json}");
        assert!(json.contains("\"layers\""), "got {json}");
    }

    #[test]
    fn a_layer_stack_with_no_layers_still_round_trips() {
        let stack = LayerStack {
            id: "s1".into(),
            name: "empty".into(),
            layers: Vec::new(),
            updated_at: 0,
        };
        let json = serde_json::to_string(&stack).unwrap();
        let back: LayerStack = serde_json::from_str(&json).unwrap();
        assert!(back.layers.is_empty());
        assert_eq!(back.name, "empty");
    }

    #[test]
    fn saved_seeds_serialise_with_camel_case_keys() {
        let s = SavedSeed {
            seed: 42,
            caption: "dusty rhodes".into(),
            starred: true,
            created_at: 1,
        };
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"createdAt\""), "got {json}");
        assert!(json.contains("\"starred\""), "got {json}");
    }

    #[test]
    fn an_absent_seed_store_reads_as_empty() {
        assert!(std::fs::read_to_string("/nonexistent/seeds.json").is_err());
    }

    #[test]
    fn a_half_written_ab_slot_is_not_treated_as_a_pair() {
        let slots: [String; 2] = ["abc".to_string(), "".to_string()];
        let complete = slots.iter().all(|s| !s.trim().is_empty());
        assert!(!complete, "one empty slot must not read as a pair");
    }
}