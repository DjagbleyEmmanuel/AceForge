//! An ABC reader, restricted to the dialect SheetSage emits.
//!
//! This exists to turn a lead sheet into lyric timings. The engine composes a
//! score and knows when every vocal note falls on the score clock, but it does
//! not hand that clock back: it returns the ABC text and stops. The reference
//! implementation upstream solves this by parsing the ABC with `abcjs`, which
//! is a full notation library.
//!
//! Porting that is not what this is. This reads the narrow subset SheetSage
//! writes, and is deliberately unable to read arbitrary ABC:
//!
//! ```text
//! X:1
//! M:4/4
//! L:1/8
//! Q:1/4=120
//! V: Vocal clef=treble name="Vocal Melody" snm="Vocal"
//! V: Ins clef=treble name="Ins Melody" snm="Inst."
//! K:C
//! % Verse
//! CDEF/2 | z2 GABc | ...
//! ```
//!
//! Fields, `%` structure markers, plain notes with `/` and digit duration
//! suffixes, `|` bar lines and `Z` multibar rests. No tuplets, no grace notes,
//! no repeats. Anything outside that returns an error rather than a guess,
//! because a wrong lyric timestamp is worse than none: the fallback spreads
//! lines evenly, which is visibly approximate, whereas a mis-parsed score would
//! put words at the wrong moments and look authoritative.
//!
//! `lyric_timings` is the entry point. Everything else here is in service of it
//! and is private.

/// One bar of the score clock.
#[derive(Debug, Clone, Copy)]
struct Bar {
    start_secs: f64,
    bpm: f64,
    beat_whole_notes: f64,
}

#[derive(Debug, Clone)]
struct Section {
    label: String,
    /// 1-indexed, matching the reference implementation.
    start_bar: usize,
}

#[derive(Debug, Clone)]
struct LyricBlock {
    label: String,
    /// Codepoint offsets into the normalised lyrics.
    start: usize,
    end: usize,
    has_lyrics: bool,
}

/// `V: Vocal ... snm="Vocal"` and `name="Vocal ..."`, which is how SheetSage
/// spells it.
fn is_vocal_voice(line: &str) -> bool {
    let rest = &line[2..];
    let first = rest.split_whitespace().next().unwrap_or("");
    first.to_lowercase().contains("vocal") || rest.contains("name=\"Vocal")
}

/// Voice ids declared by `V:` lines, minus the explicitly instrumental one. A
/// voice carrying `w:` lines is vocal by construction.
fn vocal_voice_ids(abc: &str) -> Vec<String> {
    let mut with_lyrics: Vec<String> = Vec::new();
    let mut current = String::new();
    for raw in abc.lines() {
        let line = raw.trim();
        if let Some(rest) = line.strip_prefix("V:") {
            current = rest.split_whitespace().next().unwrap_or("").to_string();
        } else if line.len() > 2
            && line[..2].eq_ignore_ascii_case("w:")
            && !current.is_empty()
            && !is_instrumental(&current)
            && !with_lyrics.contains(&current)
        {
            with_lyrics.push(current.clone());
        }
    }
    with_lyrics
}

fn is_instrumental(label: &str) -> bool {
    let l = label.to_lowercase();
    ["instrumental", "solo", "break", "interlude"]
        .iter()
        .any(|w| l.contains(w))
}

/// `"4/4"` as (4, 4). Returns (0, 0) for anything it cannot read, which every
/// caller treats as a malformed field rather than a default.
fn parse_fraction(text: &str) -> (f64, f64) {
    match text.split_once('/') {
        Some((n, d)) => (
            n.trim().parse().unwrap_or(0.0),
            d.trim().parse().unwrap_or(0.0),
        ),
        None => (0.0, 0.0),
    }
}

/// Chord attachments are quoted text hung off a note; they carry no duration.
fn strip_chords(bar: &str) -> String {
    let mut out = String::with_capacity(bar.len());
    let mut in_quote = false;
    for c in bar.chars() {
        match c {
            '"' => in_quote = !in_quote,
            _ if !in_quote => out.push(c),
            _ => {}
        }
    }
    out
}

/// A bar segment split on `|`, dropping empty ends.
fn bar_segments(line: &str) -> Vec<&str> {
    line.split('|').filter(|s| !s.trim().is_empty()).collect()
}

/// How many score bars a segment covers. `Z` is a multibar rest, and `Zn` spans
/// n of them.
fn bars_in(segment: &str) -> usize {
    let s = strip_chords(segment);
    let s = s.trim();
    if let Some(rest) = s.strip_prefix('Z') {
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        return digits.parse::<usize>().map(|n| n.max(1)).unwrap_or(1);
    }
    1
}

/// Walk the active `M:`/`Q:` fields over the vocal bars and give each one a
/// start and end on the score clock.
fn bar_clock(abc: &str, vocal_ids: &[String]) -> Result<Vec<Bar>, String> {
    let mut bars: Vec<Bar> = Vec::new();
    let mut meter_whole = 0.0f64;
    let mut bpm = 0.0f64;
    let mut beat_whole = 0.0f64;
    let mut in_vocal = false;
    let mut secs = 0.0f64;

    for raw in abc.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('%') {
            continue;
        }
        if let Some(rest) = line.strip_prefix("M:") {
            let (num, den) = parse_fraction(rest.trim());
            if num <= 0.0 || den <= 0.0 {
                return Err("Unsupported score M: meter.".into());
            }
            meter_whole = num / den;
            continue;
        }
        if let Some(rest) = line.strip_prefix("Q:") {
            // `Q:1/4=120` or a bare `Q:120`, which means a quarter note.
            let (unit, bpm_str) = match rest.split_once('=') {
                Some((u, b)) => (u.trim(), b.trim()),
                None => ("", rest.trim()),
            };
            let bpm_val: f64 = bpm_str.parse().unwrap_or(0.0);
            if bpm_val <= 0.0 {
                return Err("Unsupported score Q: tempo.".into());
            }
            // A leading fraction is the beat unit; absent means a quarter note.
            let (num, den) = parse_fraction(unit);
            beat_whole = if num > 0.0 && den > 0.0 { num / den } else { 0.25 };
            bpm = bpm_val;
            continue;
        }
        if line.starts_with("V:") {
            in_vocal = is_vocal_voice(line)
                || vocal_ids.iter().any(|id| {
                    line[2..].split_whitespace().next().unwrap_or("") == id
                });
            continue;
        }
        let is_field = line
            .chars()
            .next()
            .map(|c| c.is_ascii_alphabetic())
            .unwrap_or(false)
            && line.contains(':');
        if is_field || !in_vocal {
            continue;
        }
        if meter_whole <= 0.0 || bpm <= 0.0 || beat_whole <= 0.0 {
            return Err("Vocal score lacks an active M: meter or Q: tempo.".into());
        }
        for segment in bar_segments(line) {
            for _ in 0..bars_in(segment) {
                let dur = meter_whole / beat_whole * 60.0 / bpm;
                bars.push(Bar { start_secs: secs, bpm, beat_whole_notes: beat_whole });
                secs += dur;
            }
        }
    }
    if bars.is_empty() {
        return Err("Score has no Vocal bars.".into());
    }
    Ok(bars)
}

/// The length of one ABC note or rest in whole notes.
///
/// `C` is a whole note, `C/` a half, `C3` a triple, `C/2` a sixteenth. The
/// denominator multiplies and the numerator divides, so `C/2` is a quarter and
/// `C2` is a breve.
fn event_duration(token: &str) -> Option<f64> {
    // Chord attachments are quoted text hung off the note; they are not part of
    // the token, so they are removed here rather than relying on every caller
    // to have done it.
    let token = strip_chords(token);
    let token = token.trim();
    // An octave mark may precede the pitch, but only after a chord has been
    // stripped.
    let token = token.trim_start_matches(['\'', ',']);

    // The full pitch range matters: a score containing a single `e` or `f` would
    // otherwise lose that note's duration and shift every timestamp after it.
    const PITCHES: &str = "AaBbCcDdEeFfGg";
    const RESTS: &str = "xXzZrR";

    // The pitch is the first character. Accidentals follow it, and skipping
    // them beforehand would eat the note itself -- `c` and `f` are pitches, not
    // accidentals, which is an easy mistake to make twice.
    let mut chars = token.chars();
    let head = chars.next()?;
    if !(PITCHES.contains(head) || RESTS.contains(head)) {
        return None;
    }
    // Accidentals, octave marks and anything else that does not change length.
    let tail: String = chars
        .skip_while(|c| "bcfn#s_^<>_".contains(*c))
        .collect();

    let mut num = 1.0f64;
    let mut den = 1.0f64;
    let mut tail_chars = tail.chars().peekable();
    if tail_chars.peek() == Some(&'/') {
        tail_chars.next();
        // `/` halves it. A digit after that divides again -- `C/` is a half,
        // `C/2` a sixteenth -- so it multiplies the denominator rather than
        // replacing it.
        den = 2.0;
        let digits: String = tail_chars.by_ref().take_while(|c| c.is_ascii_digit()).collect();
        if !digits.is_empty() {
            den *= digits.parse::<f64>().unwrap_or(1.0);
        }
        if den == 0.0 {
            return None;
        }
    } else {
        let digits: String = tail_chars.take_while(|c| c.is_ascii_digit()).collect();
        if !digits.is_empty() {
            num = digits.parse::<f64>().unwrap_or(1.0);
            if num == 0.0 {
                return None;
            }
        }
    }
    Some(num / den)
}

/// First sounding vocal event in each bar, in seconds on the score clock.
///
/// Only the vocal voice is walked, and only to the extent the bar grid allows:
/// a score whose notes run past its own bars is rejected rather than clipped,
/// because a clipped grid shifts every later timestamp.
fn vocal_note_onsets(abc: &str, bars: &[Bar], vocal_ids: &[String]) -> Result<Vec<Option<f64>>, String> {
    let mut onsets: Vec<Option<f64>> = vec![None; bars.len()];
    let mut bar_index = 0usize;
    let mut offset = 0.0f64;
    let mut span_bars = 1usize;
    let mut in_vocal = false;

    for raw in abc.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with("V:") {
            in_vocal = is_vocal_voice(line)
                || vocal_ids.iter().any(|id| {
                    line[2..].split_whitespace().next().unwrap_or("") == id
                });
            continue;
        }
        let is_field = line
            .chars()
            .next()
            .map(|c| c.is_ascii_alphabetic())
            .unwrap_or(false)
            && line.contains(':');
        if is_field || !in_vocal || line.starts_with('%') {
            continue;
        }
        for segment in bar_segments(line) {
            // Split into events, treating `|` as a bar boundary. Chords are
            // dropped first so their quoted text cannot be read as notes.
            let clean = strip_chords(segment);
            for token in clean.split_whitespace() {
                if token == "|" {
                    bar_index += span_bars;
                    offset = 0.0;
                    span_bars = 1;
                    continue;
                }
                let Some(duration) = event_duration(token) else {
                    continue;
                };
                if bar_index >= bars.len() {
                    return Err(format!(
                        "ABC notes extend beyond the Vocal bar grid at bar {}.",
                        bar_index + 1
                    ));
                }
                let sounding = !token.starts_with('z')
                    && !token.starts_with('x')
                    && token.starts_with(|c: char| c.is_ascii_alphabetic())
                    && !token.starts_with('Z')
                    && !token.starts_with('X');
                if sounding && onsets[bar_index].is_none() {
                    let bar = bars[bar_index];
                    onsets[bar_index] = Some(
                        bar.start_secs + offset / bar.beat_whole_notes * 60.0 / bar.bpm,
                    );
                }
                if token.starts_with('Z') {
                    span_bars = token[1..]
                        .chars()
                        .take_while(|c| c.is_ascii_digit())
                        .collect::<String>()
                        .parse::<usize>()
                        .unwrap_or(1)
                        .max(1);
                }
                offset += duration;
            }
            bar_index += span_bars;
            offset = 0.0;
            span_bars = 1;
        }
    }
    Ok(onsets)
}

/// `% Verse` markers and the bar each one starts on.
fn score_sections(abc: &str, vocal_ids: &[String]) -> Vec<Section> {
    let mut sections: Vec<Section> = Vec::new();
    let mut in_vocal = false;
    let mut bars = 0usize;
    for raw in abc.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix('%') {
            let label = rest.trim();
            if !label.is_empty() {
                sections.push(Section { label: label.to_string(), start_bar: bars + 1 });
            }
            continue;
        }
        if line.starts_with("V:") {
            in_vocal = is_vocal_voice(line)
                || vocal_ids.iter().any(|id| {
                    line[2..].split_whitespace().next().unwrap_or("") == id
                });
            continue;
        }
        let is_field = line
            .chars()
            .next()
            .map(|c| c.is_ascii_alphabetic())
            .unwrap_or(false)
            && line.contains(':');
        if is_field || !in_vocal {
            continue;
        }
        for segment in bar_segments(line) {
            bars += bars_in(segment);
        }
    }
    sections
}

/// `[Verse 1]` at the head of a line is a section header; anything else is a
/// lyric line.
fn lyric_header(line: &str) -> Option<String> {
    let t = line.trim();
    if !t.starts_with('[') {
        return None;
    }
    let close = t.find(']')?;
    let label = t[1..close].trim();
    if label.is_empty() {
        None
    } else {
        Some(label.to_string())
    }
}

/// The lyrics split into section blocks, with codepoint offsets.
///
/// Codepoints rather than bytes or UTF-16 units, because the offsets in the
/// wire format are codepoint offsets and a mismatch silently mis-places every
/// span after the first accented character.
fn lyric_blocks(lyrics: &str) -> Vec<LyricBlock> {
    let normalised = lyrics.replace("\r\n", "\n").replace('\r', "\n");
    let chars: Vec<char> = normalised.chars().collect();
    let mut blocks: Vec<LyricBlock> = Vec::new();
    let mut offset = 0usize;
    for line in normalised.split('\n') {
        if let Some(label) = lyric_header(line) {
            if let Some(last) = blocks.last_mut() {
                last.end = offset;
            }
            blocks.push(LyricBlock {
                label,
                start: offset,
                end: chars.len(),
                has_lyrics: false,
            });
        }
        offset += line.chars().count() + 1;
    }
    blocks
        .into_iter()
        .map(|mut b| {
            let body: String = chars[b.start.min(chars.len())..b.end.min(chars.len())]
                .iter()
                .collect();
            let stripped = strip_leading_tag(&body);
            b.has_lyrics = stripped.chars().any(|c| c.is_alphanumeric());
            b
        })
        .collect()
}

fn strip_leading_tag(body: &str) -> String {
    let t = body.trim_start();
    if !t.starts_with('[') {
        return t.to_string();
    }
    match t.find(']') {
        // Only a tag on the same line as content is stripped; a line that is
        // nothing but a tag leaves an empty body, which is correct.
        Some(close) => t[close + 1..].to_string(),
        None => t.to_string(),
    }
}

/// `Verse 1` and `verse 3` are the same kind of section; `Pre-Chorus` is its own.
fn section_kind(label: &str) -> String {
    let n: String = label
        .to_lowercase()
        .chars()
        .map(|c| if c == '_' || c == '\u{2013}' || c == '\u{2014}' { '-' } else { c })
        .collect();
    if has_word(&n, "pre-chorus") || has_word(&n, "prechorus") || has_word(&n, "pre chorus") {
        return "pre-chorus".to_string();
    }
    for kind in ["verse", "chorus", "bridge", "intro", "outro", "interlude"] {
        if has_word(&n, kind) {
            return kind.to_string();
        }
    }
    n.trim().to_string()
}

/// Word-boundary match, so "versed" does not read as a verse.
fn has_word(haystack: &str, word: &str) -> bool {
    match haystack.find(word) {
        None => false,
        Some(at) => {
            let before_ok = at == 0
                || !haystack[..at]
                    .chars()
                    .next_back()
                    .map(|c| c.is_alphanumeric())
                    .unwrap_or(false);
            let after = &haystack[at + word.len()..];
            let after_ok = after
                .chars()
                .next()
                .map(|c| !c.is_alphanumeric())
                .unwrap_or(true);
            before_ok && after_ok
        }
    }
}

/// Lyric timings taken from a lead sheet, in the order they are sung.
///
/// Each scored section is paired with the next lyric block of the same kind, in
/// order, and stamped at the first vocal note inside that section's bars.
/// Blocks with no timed section are dropped rather than guessed at.
/// What a composed score turned out to be.
///
/// Exists so a score can be judged before the NAR stage is spent on it. That
/// stage is the long one -- minutes rather than seconds -- and it does not care
/// that it is about to turn a three-bar fragment into eight minutes of audio.
#[derive(Debug, Clone, PartialEq)]
pub struct ScoreHealth {
    /// Vocal bars. The composer's instrumental parts are not counted: a score
    /// with no vocal bars cannot carry lyrics, which is the point of checking.
    pub bars: usize,
    /// Labelled sections the voice moves through.
    pub sections: usize,
}

/// Inspect a composed score without committing to a render.
pub fn score_health(abc: &str) -> Result<ScoreHealth, String> {
    let vocal_ids = vocal_voice_ids(abc);
    let bars = bar_clock(abc, &vocal_ids)?.len();
    let sections = score_sections(abc, &vocal_ids).len();
    Ok(ScoreHealth { bars, sections })
}

pub fn lyric_timings(abc: &str, lyrics: &str) -> Result<Vec<(f64, String)>, String> {
    if !abc.lines().any(|l| l.trim_start().starts_with("Q:")) {
        return Err("Score has no tempo (Q:); a free-tempo cover has no clock.".into());
    }
    let vocal_ids = vocal_voice_ids(abc);
    let bars = bar_clock(abc, &vocal_ids)?;
    let onsets = vocal_note_onsets(abc, &bars, &vocal_ids)?;
    let sections = score_sections(abc, &vocal_ids);
    if sections.is_empty() {
        return Err("Score has no labelled sections.".into());
    }

    let blocks = lyric_blocks(lyrics);
    let mut out: Vec<(f64, String)> = Vec::new();
    let mut next_block = 0usize;
    for (index, section) in sections.iter().enumerate() {
        if is_instrumental(&section.label) {
            continue;
        }
        let wanted = section_kind(&section.label);
        let Some(match_at) = blocks.iter().enumerate().skip(next_block).find(|(_, b)| {
            b.has_lyrics && !is_instrumental(&b.label) && section_kind(&b.label) == wanted
        }) else {
            continue;
        };
        next_block = match_at.0 + 1;

        let end_bar = if index + 1 < sections.len() {
            sections[index + 1].start_bar.saturating_sub(1)
        } else {
            bars.len()
        };
        let onset = onsets
            .iter()
            .skip(section.start_bar.saturating_sub(1))
            .take(end_bar.saturating_sub(section.start_bar).max(1))
            .copied()
            .flatten()
            .next();
        let Some(onset) = onset else { continue };

        let block = &blocks[match_at.0];
        // Trim trailing whitespace so the emitted line is not mostly blanks.
        let mut end = block.end;
        while end > block.start
            && lyrics
                .chars()
                .nth(end - 1)
                .map(|c| c.is_whitespace())
                .unwrap_or(false)
        {
            end -= 1;
        }
        let text: String = lyrics.chars().skip(block.start).take(end - block.start).collect();
        for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
            out.push(((onset * 1000.0).round() / 1000.0, line.to_string()));
        }
    }
    if out.is_empty() {
        return Err("No sung lyric block lined up with a timed score section.".into());
    }
    Ok(out)
}

    /// The health gate runs before the NAR stage, which is the long one, so it
    /// has to be right about what counts as a renderable score.
    #[test]
    fn score_health_counts_vocal_bars_and_sections() {
        let abc = concat!(
            "X:1\nT:\nM:4/4\nL:1/8\nQ:1/4=120\n",
            "V: Vocal clef=treble name=\"Vocal Melody\" snm=\"Vocal\"\n",
            "V: Bass snm=\"Bass\"\n",
            "K:C\n",
            "% Verse\n", "V: Vocal\n", "CDEF/2 | GABc | z4 |\n",
            "% Chorus\n", "V: Vocal\n", "CDEF GABc | cdef gabc | z4 |\n",
        );
        let health = score_health(abc).expect("a healthy score");
        assert_eq!(health.bars, 6, "instrumental bars must not be counted");
        assert_eq!(health.sections, 2);
    }

    /// Every reason the gate exists: a score it waves through is a score the
    /// user waits minutes to hear and then wishes they had not.
    #[test]
    fn score_health_rejects_a_score_nothing_can_be_heard_in() {
        for abc in [
            "",
            "not abc at all",
            // A well-formed score with no vocal voice at all.
            "X:1\nM:4/4\nQ:1/4=120\nV: Piano\nK:C\nCDEF|GABc|\n",
            // Vocal voice, but no notes under it.
            "X:1\nM:4/4\nQ:1/4=120\nV: Vocal\nK:C\n",
        ] {
            assert!(score_health(abc).is_err(), "accepted an unusable score: {abc:?}");
        }
    }

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact shape SheetSage writes. A regression here means the parser
    /// stopped reading real output, and every timing downstream is wrong.
    ///
    /// `V:` really is a voice switch in ABC, so each staff's music is preceded
    /// by its own `V:` line -- which is what the writer emits, one pair per
    /// measure group. Testing against a score with both voice declarations
    /// stacked and music after them would be a score nothing produces.
    const SHEETSAGE: &str = concat!(
        "X:1\nT:\nM:4/4\nL:1/8\nQ:1/4=120\n",
        "V: Vocal clef=treble name=\"Vocal Melody\" snm=\"Vocal\"\n",
        "V: Ins clef=treble name=\"Ins Melody\" snm=\"Inst.\"\n",
        "K:C\n",
        "% Verse\n",
        "V: Vocal\nCDEF/2 | GABc |\n",
        "V: Ins\nCEFG/2 | Gabc |\n",
        "% Chorus\n",
        "V: Vocal\nCDEF GABc | cdef gabc |\n",
        "V: Ins\nCEFG GABc | cdef gabc |\n",
    );

    #[test]
    fn reads_the_fields_sheetsage_emits() {
        let ids = vocal_voice_ids(SHEETSAGE);
        let bars = bar_clock(SHEETSAGE, &ids).expect("clock");
        // 4/4 at 120 bpm is two seconds a bar; four bars are written, so the
        // bar starts alone say both the count and the metre.
        assert_eq!(bars.len(), 4);
        for (i, bar) in bars.iter().enumerate() {
            assert!((bar.start_secs - i as f64 * 2.0).abs() < 1e-9, "bar {i} at {}", bar.start_secs);
        }
    }

    /// Every note length SheetSage writes has to decode, because the whole
    /// clock is built from their sum.
    #[test]
    fn decodes_abc_note_lengths() {
        assert_eq!(event_duration("C"), Some(1.0));
        assert_eq!(event_duration("C/"), Some(0.5));
        assert_eq!(event_duration("C/2"), Some(0.25));
        assert_eq!(event_duration("C3"), Some(3.0));
        assert_eq!(event_duration("c"), Some(1.0));
        assert_eq!(event_duration("C#"), Some(1.0));
        assert_eq!(event_duration("\"c\"e"), Some(1.0));
        assert_eq!(event_duration("x"), Some(1.0), "an invisible rest is a rest");
        assert_eq!(event_duration("|"), None);
    }

    /// A multibar rest spans bars. Getting this wrong walks every later onset.
    #[test]
    fn a_multibar_rest_spans_bars() {
        assert_eq!(bars_in("CDEF"), 1);
        assert_eq!(bars_in("Z"), 1);
        assert_eq!(bars_in("Z4"), 4);
        assert_eq!(bars_in("\"c\"Z2"), 2);
    }

    #[test]
    fn section_kinds_match_across_numbering() {
        assert_eq!(section_kind("Verse 1"), "verse");
        assert_eq!(section_kind("verse 3"), "verse");
        assert_eq!(section_kind("CHORUS"), "chorus");
        assert_eq!(section_kind("Pre-Chorus"), "pre-chorus");
        assert_eq!(section_kind("Prechorus"), "pre-chorus");
        // A word merely containing "verse" is not a verse section.
        assert_ne!(section_kind("Reversed"), "verse");
    }

    #[test]
    fn instrumental_sections_are_not_matched_to_lyrics() {
        assert!(is_instrumental("Instrumental"));
        assert!(is_instrumental("Guitar solo"));
        assert!(!is_instrumental("Verse 1"));
    }

    /// The end to end claim: a score plus lyrics gives timestamps on the score
    /// clock, in order, and the first is the first vocal note of its section.
    #[test]
    fn lyric_lines_are_stamped_from_the_score_clock() {
        let lyrics = "[Verse 1]\nHey baby\nI don't want much\n[Chorus]\nNo no no";
        let timed = lyric_timings(SHEETSAGE, lyrics).expect("timings");
        let texts: Vec<&str> = timed.iter().map(|(_, t)| t.as_str()).collect();
        // Section markers are stamped too, which is what the ACE-Step sidecar
        // does -- `[00:00.00][Verse 1]` -- and what the library's parser expects.
        assert_eq!(
            texts,
            vec!["[Verse 1]", "Hey baby", "I don't want much", "[Chorus]", "No no no"],
            "every line of every sung block, headers included, in order"
        );
        // The verse starts at the first vocal note: the top of bar 1, t=0.
        assert!((timed[0].0 - 0.0).abs() < 1e-6, "first line at {}", timed[0].0);
        // The chorus is labelled after bar 2, so it cannot start before bar 3.
        let chorus = timed.last().unwrap().0;
        assert!(chorus >= 4.0, "chorus stamped into the verse: {chorus}");
        // Strictly ordered, or the player would scrub backwards.
        for pair in timed.windows(2) {
            assert!(pair[0].0 <= pair[1].0, "stamps went backwards: {timed:?}");
        }
    }

    /// Every failure has to be an error. A score with no tempo has no clock,
    /// and returning a plausible-looking time would be worse than refusing.
    #[test]
    fn an_unusable_score_is_refused_rather_than_guessed() {
        let no_tempo = "X:1\nM:4/4\nK:C\nV: Vocal\nCDEF |\n";
        assert!(lyric_timings(no_tempo, "[Verse]\nhi").is_err());

        let no_sections = "X:1\nM:4/4\nQ:1/4=120\nV: Vocal\nCDEF |\n";
        assert!(lyric_timings(no_sections, "[Verse]\nhi").is_err());

        let no_vocal_bars = "X:1\nM:4/4\nQ:1/4=120\nV: Ins\nCDEF |\n";
        assert!(lyric_timings(no_vocal_bars, "[Verse]\nhi").is_err());

        let unmatched = "X:1\nM:4/4\nQ:1/4=120\nV: Vocal\n% Verse\nCDEF |\n";
        assert!(lyric_timings(unmatched, "[Bridge]\nhi").is_err(), "a bridge with no bridge section");
    }

    /// A plain-text style score, and one with no lyrics at all, must both fail
    /// rather than produce timings for words that are not there.
    #[test]
    fn nonsense_in_is_refused() {
        assert!(lyric_timings("not a score at all", "[Verse]\nhi").is_err());
        assert!(lyric_timings(SHEETSAGE, "").is_err());
    }
}