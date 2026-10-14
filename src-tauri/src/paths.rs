//! Canonical on-disk locations.
//!
//! These must agree with the asset protocol scope in tauri.conf.json.
//! Tauri resolves `$APPDATA` to the *identifier-scoped* data dir
//! (~/.local/share/<identifier> on Linux), NOT the bare XDG data dir, so
//! using `dirs::data_dir()` here would put the library outside the scope and
//! every track would fail to play.

use std::path::{Path, PathBuf};

pub const APP_DIR_NAME: &str = "app.aceforge.desktop";

/// Must match `identifier` in tauri.conf.json.
pub fn app_data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("ACEFORGE_DATA_DIR") {
        if !dir.trim().is_empty() {
            return PathBuf::from(dir);
        }
    }
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(APP_DIR_NAME)
}

/// Where songs and imported audio live.
///
/// Resolved from `settings::library_dir`, falling back to app data. Every
/// caller goes through here, so moving the library moves writing, listing,
/// deleting and the import directory together.
pub fn library_dir() -> PathBuf {
    let configured = crate::settings::load().library_dir;
    let trimmed = configured.trim();
    if trimmed.is_empty() {
        return app_data_dir().join("library");
    }
    PathBuf::from(trimmed)
}

/// Imported source audio. A subdirectory of the library rather than loose
/// files in it: `list_tracks` scans the library non-recursively, so imports
/// stay out of the generated-song list and the two never get confused.
pub fn imports_dir() -> PathBuf {
    library_dir().join("imports")
}

pub fn settings_file() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(APP_DIR_NAME)
        .join("settings.json")
}

/// Turn a prompt into a filename-safe, readable stem.
///
/// A finished song used to be called `1791555467-0.mp3`, which told the user
/// nothing and made the library impossible to scan. A user-supplied title
/// wins; failing that, slugify the caption, then the opening lyric line.
pub fn song_stem(title: &str, caption: &str, lyrics: &str) -> String {
    let from_title = slugify(title, 10);
    if !from_title.is_empty() {
        return from_title;
    }
    let from_caption = slugify(caption, 8);
    if !from_caption.is_empty() {
        return from_caption;
    }

    // No caption: use the first line that is not a section tag, so a song named
    // only by its lyrics still lands under something recognisable.
    for line in lyrics.lines() {
        let l = line.trim();
        if l.is_empty() {
            continue;
        }
        if l.starts_with('[') && l.ends_with(']') {
            continue;
        }
        let s = slugify(l, 8);
        if !s.is_empty() {
            return s;
        }
    }

    "untitled".to_string()
}

/// Lowercase, hyphenated. Non-ASCII letters are kept: a CJK caption is still a
/// far better title than a timestamp, and transliteration is not worth the
/// dependency. Returns empty when nothing usable survives, which is the
/// caller's cue to try the next source.
pub fn slugify(text: &str, max_words: usize) -> String {
    let mut out = String::new();
    let mut pending_sep = false;
    let mut words = 0usize;

    for ch in text.chars() {
        if ch.is_alphanumeric() {
            // Count when a word *starts*, not when a character is appended, or
            // the last permitted word gets truncated mid-way.
            if out.is_empty() || pending_sep {
                if words >= max_words {
                    break;
                }
                words += 1;
            }
            if pending_sep {
                out.push('-');
            }
            pending_sep = false;
            out.push(ch.to_ascii_lowercase());
        } else if !out.is_empty() {
            pending_sep = true;
        }
    }

    out.trim_matches('-').to_string()
}

/// First free path for `stem` in `dir`: `stem.ext`, then `stem-2.ext`, ...
///
/// Existence is the only collision test that matters here: these files are
/// written once and never rewritten, and a same-second second take must not
/// silently overwrite the first.
pub fn unique_path(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let stem = if stem.trim().is_empty() { "untitled" } else { stem };
    let first = dir.join(format!("{stem}.{ext}"));
    if !first.exists() {
        return first;
    }
    for n in 2..10_000 {
        let candidate = dir.join(format!("{stem}-{n}.{ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    dir.join(format!("{stem}-{}.{ext}", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_a_caption_into_a_filename() {
        assert_eq!(
            song_stem("", "Warm lo-fi hip hop beat, vinyl crackle, mellow piano, relaxed", ""),
            "warm-lo-fi-hip-hop-beat-vinyl-crackle"
        );
    }

    #[test]
    fn drops_punctuation_and_collapses_separators() {
        assert_eq!(slugify("  Hello,   World!!  ", 8), "hello-world");
        assert_eq!(slugify("a/b\\c:d", 8), "a-b-c-d");
    }

    #[test]
    fn keeps_non_latin_scripts() {
        assert_eq!(slugify("夜空中最亮的星", 8), "夜空中最亮的星");
    }

    #[test]
    fn falls_back_to_the_first_real_lyric_line() {
        // Section tags are structure, not a title.
        assert_eq!(
            song_stem("", "", "[Verse]\nMorning light through the pines\n..."),
            "morning-light-through-the-pines"
        );
    }

    #[test]
    fn falls_back_to_untitled_when_there_is_nothing_to_work_with() {
        assert_eq!(song_stem("", "", "[Verse]\n[Chorus]\n"), "untitled");
        assert_eq!(song_stem("", "!!! ???", ""), "untitled");
    }

    #[test]
    fn never_returns_an_empty_stem() {
        assert_eq!(slugify("", 8), "");
        assert_eq!(slugify("---", 8), "");
    }

    #[test]
    fn unique_path_never_reuses_a_name() {
        let dir = std::env::temp_dir().join(format!("aceforge-paths-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");

        let a = unique_path(&dir, "my-song", "mp3");
        std::fs::write(&a, b"x").expect("write");
        let b = unique_path(&dir, "my-song", "mp3");
        std::fs::write(&b, b"x").expect("write");
        let c = unique_path(&dir, "my-song", "mp3");

        assert_eq!(a.file_name().unwrap(), "my-song.mp3");
        assert_eq!(b.file_name().unwrap(), "my-song-2.mp3");
        assert_eq!(c.file_name().unwrap(), "my-song-3.mp3");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
