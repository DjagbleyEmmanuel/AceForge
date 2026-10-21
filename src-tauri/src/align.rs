//! Forced alignment of a finished YuE2 render to the lyrics it was sung from.
//!
//! YuE2's DiT has no lyric-alignment head, so there is no cross-attention to
//! read a timestamp out of the way ACE-Step does. What YuE2 has instead is an
//! MMS_FA acoustic model with a CTC head, exposed by the engine as
//! `POST /yue2/align`: hand it the finished audio and the EXACT lyrics string
//! the render was given, and it returns one span per sung word, with codepoint
//! offsets back into that string.
//!
//! This is a different kind of answer from the score-derived timings in
//! `crate::abc`. Those read where the notes sit in the lead sheet, which is an
//! estimate of when a word starts. This listens to the recording. When both are
//! available the aligner wins, and the score is the fallback for a machine
//! without the model -- which is most of them, since it is 1.26 GB and not
//! shipped with the app.
//!
//! The engine loads the model per request and frees it again, and says why:
//! "1.26 GB of F32 that nothing else in a render wants resident, and alignment
//! happens once per track at the very end, after the LM and the VAE are done
//! with the card." So this is called after a track is finished, never during.

use serde::Deserialize;
use std::path::Path;

/// Align on the CPU, using a second engine instance.
///
/// The aligner's backend is chosen when the process starts -- `GGML_BACKEND` is
/// read from the environment at that point and there is no per-request knob --
/// so the only way to get a CPU aligner is a second engine. The running
/// engine keeps its GPU, which is what it needs for the render; this one does
/// nothing but align.
///
/// The CPU path is slow: measured at 399 seconds for three minutes of audio on
/// four threads, against a second and a half on the GPU. It is here because
/// the GPU path runs out of room on a card this size -- the compute buffer
/// grows about 22 MB per second of audio, and the model wants another 1203,
/// so four gigabytes is gone by the two-minute mark -- and a track with no
/// timings at all is worse than one that took ten minutes.
pub fn align_on_cpu(
    binary: &Path,
    models_dir: &Path,
    audio: &Path,
    lyrics: &str,
    threads: u32,
) -> Result<Vec<AlignedWord>, String> {
    let port = free_port()?;
    let mut child = std::process::Command::new(binary)
        .arg("--models")
        .arg(models_dir)
        .arg("--host")
        .arg("127.0.0.1")
        .arg("--port")
        .arg(port.to_string())
        .env("GGML_BACKEND", "CPU")
        .env("GGML_N_THREADS", threads.to_string())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("cannot start a CPU engine: {e}"))?;

    let result = (|| {
        wait_for_port(port, std::time::Duration::from_secs(120))?;
        align(port, audio, lyrics)
    })();

    // The child is a whole engine, loaded with a gigabyte of weights. It goes
    // away with the align, either way.
    let _ = child.kill();
    let _ = child.wait();
    result
}

/// A port nothing is listening on.
///
/// Bound and released rather than guessed: a random high port can be anything,
/// including something another process is already on.
fn free_port() -> Result<u16, String> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")
        .map_err(|e| format!("cannot find a free port: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("cannot read the port: {e}"))?
        .port();
    drop(listener);
    Ok(port)
}

/// Wait until the engine answers, or give up.
///
/// The engine loads its registry before it listens, and on a cold page cache
/// that takes a few seconds.
fn wait_for_port(port: u16, timeout: std::time::Duration) -> Result<(), String> {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if std::net::TcpStream::connect_timeout(
            &format!("127.0.0.1:{port}").parse().map_err(|e| format!("bad port: {e}"))?,
            std::time::Duration::from_millis(500),
        )
        .is_ok()
        {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    Err("the CPU engine did not start listening in time".into())
}

/// One word the aligner timed.
#[derive(Debug, Clone, Deserialize)]
pub struct AlignedWord {
    /// Seconds from the start of the track.
    pub start: f64,
    pub end: f64,
    /// The aligner's own confidence in this span.
    pub score: f64,
    /// Half-open range of CODEPOINTS in the lyrics string that was sent.
    ///
    /// Codepoints, not bytes and not UTF-16 units: the engine counts the way
    /// Python does, so an offset computed any other way drifts by one unit per
    /// character on a lyric with an emoji or an astral character -- the kind of
    /// drift nobody notices until the karaoke line is a line behind.
    pub char0: usize,
    pub char1: usize,
}

#[derive(Debug, Deserialize)]
struct AlignResponse {
    #[serde(default)]
    words: Vec<AlignedWord>,
}

/// Whether the aligner model is installed.
///
/// The engine looks for `mms-fa-f32.gguf` in its YuE2 search dirs, else any
/// `mms-fa-*.gguf`, and its search dirs are `<models_dir>/yue2` then
/// `<models_dir>`. This checks the same places, in the same order.
///
/// Not asked of the engine: `/yue2/props` reports the language model, the VAE
/// and the adapters, and says nothing about the aligner at all. So the only
/// way to know is to look.
pub fn model_present(models_dir: &Path) -> bool {
    for dir in [models_dir.join("yue2"), models_dir.to_path_buf()] {
        if dir.join("mms-fa-f32.gguf").is_file() {
            return true;
        }
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.starts_with("mms-fa-") && name.ends_with(".gguf") {
                    return true;
                }
            }
        }
    }
    false
}

/// Align a finished track to the lyrics it was sung from.
///
/// `lyrics` must be the exact string the render was given. The offsets in the
/// answer index into it and nothing re-derives them, so a normalised or
/// re-wrapped version would put every word in the wrong place.
pub fn align(port: u16, audio: &Path, lyrics: &str) -> Result<Vec<AlignedWord>, String> {
    if lyrics.trim().is_empty() {
        return Err("no lyrics to align against".into());
    }
    let boundary = format!("----aceforge-align{}", now_nanos());
    let mut body: Vec<u8> = Vec::with_capacity(lyrics.len() + 4096);

    // The lyrics part first, so a request that is truncated by a bad path
    // still says which lyrics it was for.
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        b"Content-Disposition: form-data; name=\"lyrics\"\r\n\r\n",
    );
    body.extend_from_slice(lyrics.as_bytes());

    body.extend_from_slice(format!("\r\n--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        b"Content-Disposition: form-data; name=\"audio\"; filename=\"track\"\r\n\r\n",
    );
    let audio_bytes = std::fs::read(audio)
        .map_err(|e| format!("cannot read {}: {e}", audio.display()))?;
    body.extend_from_slice(&audio_bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

    let url = format!("http://127.0.0.1:{port}/yue2/align");
    let response = match ureq::post(&url)
        // Generous, because the first call loads 1.26 GB of weights and then
        // runs a forward pass over the whole track.
        .timeout(std::time::Duration::from_secs(600))
        .set(
            "Content-Type",
            &format!("multipart/form-data; boundary={boundary}"),
        )
        .send(&body[..])
    {
        Ok(r) => r,
        Err(ureq::Error::Status(code, r)) => {
            let detail = r
                .into_string()
                .ok()
                .and_then(|b| {
                    serde_json::from_str::<serde_json::Value>(&b)
                        .ok()
                        .and_then(|v| v.get("error")?.as_str().map(str::to_string))
                })
                .unwrap_or_default();
            let hint = if detail.is_empty() { String::new() } else { format!(": {detail}") };
            return Err(format!("/yue2/align refused the request ({code}){hint}"));
        }
        Err(e) => return Err(format!("/yue2/align failed: {e}")),
    };

    let text = response
        .into_string()
        .map_err(|e| format!("align response unreadable: {e}"))?;
    let parsed: AlignResponse = serde_json::from_str(&text)
        .map_err(|e| format!("align response is not the expected shape: {e}"))?;
    if parsed.words.is_empty() {
        return Err("the aligner timed no words at all".into());
    }
    Ok(parsed.words)
}

/// One line of the lyrics, located in codepoints.
struct LyricLine {
    c0: usize,
    c1: usize,
    text: String,
    /// The section marker in force at this line, if any.
    section: Option<String>,
}

/// Split the lyrics into locatable lines, carrying each one's section label.
///
/// A line whose trimmed form opens with `[` and closes with `]` is a section
/// marker, not a lyric: it names the lines that follow it. That is the same
/// test the engine's own line splitter uses.
fn lyric_lines(chars: &[char]) -> Vec<LyricLine> {
    let mut out = Vec::new();
    let mut section: Option<String> = None;
    let mut start = 0usize;
    for i in 0..=chars.len() {
        if i < chars.len() && chars[i] != '\n' {
            continue;
        }
        let text: String = chars[start..i].iter().collect();
        let trimmed = text.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') && trimmed.len() > 1 {
            section = Some(trimmed[1..trimmed.len() - 1].trim().to_string());
        } else if !trimmed.is_empty() {
            out.push(LyricLine {
                c0: start,
                c1: i,
                text: trimmed.to_string(),
                section: section.clone(),
            });
        }
        start = i + 1;
    }
    out
}

/// Build an LRC sidecar from the aligner's word spans.
///
/// The shape matches what the other engine writes, so the player reads both the
/// same way: a section marker on its own line, then the words.
///
/// A line the aligner timed nothing for is left out rather than guessed at.
/// The bar showing nothing is honest; a fabricated timestamp is not.
pub fn aligned_lrc(lyrics: &str, words: &[AlignedWord]) -> String {
    let chars: Vec<char> = lyrics.chars().collect();
    let lines = lyric_lines(&chars);
    if lines.is_empty() {
        return String::new();
    }
    let mut sorted: Vec<&AlignedWord> = words.iter().collect();
    sorted.sort_by_key(|w| w.char0);

    let mut out = String::new();
    let mut w = 0usize;
    // The section marker in force, and whether it has been written yet. It is
    // stamped with the first line that follows it, because a section is not
    // sung and so has no time of its own.
    let mut pending_section: Option<String> = None;
    let mut written_sections: Vec<String> = Vec::new();
    let mut last_at = 0.0f64;

    for line in &lines {
        // A new section starts here.
        if let Some(section) = &line.section {
            if !written_sections.iter().any(|s| s == section) {
                pending_section = Some(section.clone());
            }
        }

        while w < sorted.len() && sorted[w].char0 < line.c0 {
            w += 1;
        }
        let mut picked: Vec<&AlignedWord> = Vec::new();
        while w < sorted.len() && sorted[w].char0 < line.c1 {
            picked.push(sorted[w]);
            w += 1;
        }
        if picked.is_empty() {
            continue;
        }

        // The Viterbi path cannot go backwards, but a bad response could, and a
        // line the player scrubs to and never leaves is worse than a clamp. The
        // reference implementation leans on the engine for this; leaning on a
        // remote response is not the same thing.
        let at = picked.iter().map(|p| p.start).fold(f64::INFINITY, f64::min);
        let at = if at.is_finite() { at.max(0.0).max(last_at) } else { last_at };

        if let Some(section) = pending_section.take() {
            out.push_str(&stamp(at));
            out.push_str(&format!("[{section}]\n"));
            written_sections.push(section);
        }
        out.push_str(&stamp(at));
        out.push_str(&format!("{}\n", line.text));
        last_at = at;
    }
    out
}

/// `[mm:ss.xx]`, the same shape the rest of the app writes.
fn stamp(at: f64) -> String {
    let mins = (at / 60.0) as u32;
    let secs = at - (mins as f64 * 60.0);
    format!("[{mins:02}:{secs:05.2}]")
}

fn now_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(start: f64, char0: usize, char1: usize) -> AlignedWord {
        AlignedWord {
            start,
            end: start + 0.4,
            score: 0.9,
            char0,
            char1,
        }
    }

    /// The whole point: the timings come from the words, and the section
    /// marker is stamped with the first line that follows it.
    #[test]
    fn sections_and_lines_are_stamped_from_the_words() {
        let lyrics = "[Verse 1]\nhello there\n[Chorus]\nagain now";
        // "hello" at 1.0, "there" at 1.5, "again" at 5.0, "now" at 5.4
        let words = vec![
            word(1.0, 11, 16),
            word(1.5, 17, 22),
            word(5.0, 32, 37),
            word(5.4, 38, 41),
        ];
        let lrc = aligned_lrc(lyrics, &words);
        assert!(lrc.contains("[00:01.00][Verse 1]"), "{lrc}");
        assert!(lrc.contains("[00:01.00]hello there"), "{lrc}");
        assert!(lrc.contains("[00:05.00][Chorus]"), "{lrc}");
        assert!(lrc.contains("[00:05.00]again now"), "{lrc}");
    }

    /// A line the aligner timed nothing for is left out, not invented.
    #[test]
    fn a_line_with_no_timed_word_is_left_out() {
        let lyrics = "[Verse 1]\nhello there\nsilence\n";
        let words = vec![word(1.0, 11, 16), word(1.5, 17, 22)];
        let lrc = aligned_lrc(lyrics, &words);
        assert!(lrc.contains("hello there"));
        assert!(!lrc.contains("silence"), "a line with no timing was invented: {lrc}");
    }

    /// Offsets are codepoints, not bytes. A lyric with an emoji before the
    /// words would be misplaced by a byte-counting implementation.
    #[test]
    fn offsets_are_counted_in_codepoints() {
        let lyrics = "\u{1F3B5} hello";
        // The emoji is one codepoint, so "hello" starts at codepoint 2.
        let words = vec![word(2.0, 2, 7)];
        let lrc = aligned_lrc(lyrics, &words);
        assert!(lrc.contains("hello"), "{lrc}");
        assert!(lrc.contains("[00:02.00]"), "{lrc}");
    }

    /// A word that lies outside every line is skipped rather than attached to
    /// a neighbour.
    #[test]
    fn a_span_outside_every_line_is_skipped() {
        let lyrics = "one\ntwo";
        let words = vec![word(1.0, 0, 3), word(2.0, 900, 903)];
        let lrc = aligned_lrc(lyrics, &words);
        // "two" has no timed word of its own, so it is left out rather than
        // given the stray span's time.
        assert_eq!(lrc.matches('[').count(), 1, "only the timed line: {lrc}");
        assert!(lrc.contains("one"));
    }

    /// Times must not go backwards, or the player scrubs to a line and never
    /// leaves it.
    #[test]
    fn stamps_never_go_backwards() {
        let lyrics = "one\ntwo\nthree";
        let words = vec![word(5.0, 0, 3), word(1.0, 4, 7), word(3.0, 8, 13)];
        let lrc = aligned_lrc(lyrics, &words);
        let mut last = f64::NEG_INFINITY;
        for line in lrc.lines() {
            let inner = &line[1..line.find(']').unwrap_or(1)];
            let mut parts = inner.split(':');
            let mins: f64 = parts.next().unwrap().parse().unwrap();
            let secs: f64 = parts.next().unwrap().parse().unwrap();
            let at = mins * 60.0 + secs;
            assert!(at >= last, "stamps went backwards in {lrc:?}");
            last = at;
        }
    }

    /// Real answer from the engine, against a real 5 second track and the
    /// lyrics below. Every offset in it is the engine's own, so this pins the
    /// line splitting to the engine's counting rather than to mine.
    ///
    ///   [Verse 1]\nhello there my friend\n[Chorus]\nsing it loud now
    ///
    /// "hello" is codepoints 10..15, "there" 16..21, "my" 22..24,
    /// "friend" 25..31, "sing" 41..45, "it" 46..48, "loud" 49..53,
    /// "now" 54..57.
    /// The check has to look where the engine looks, or it will say "no
    /// aligner" on a machine that has one.
    #[test]
    fn the_model_check_finds_the_file_where_the_engine_looks() {
        let dir = std::env::temp_dir().join(format!("aceforge-mmsfa-{}", std::process::id()));
        let yue2 = dir.join("yue2");
        std::fs::create_dir_all(&yue2).unwrap();
        assert!(!model_present(&dir), "nothing installed yet");

        std::fs::write(yue2.join("mms-fa-f32.gguf"), b"x").unwrap();
        assert!(model_present(&dir), "the preferred name must be found");

        std::fs::remove_file(yue2.join("mms-fa-f32.gguf")).unwrap();
        std::fs::write(yue2.join("mms-fa-f16.gguf"), b"x").unwrap();
        assert!(model_present(&dir), "any mms-fa-*.gguf must be found");

        std::fs::remove_file(yue2.join("mms-fa-f16.gguf")).unwrap();
        std::fs::write(dir.join("mms-fa-f32.gguf"), b"x").unwrap();
        assert!(model_present(&dir), "the models dir is searched too");

        std::fs::remove_dir_all(&dir).ok();
    }
    /// The CPU aligner needs a port of its own, and the port must not be one
    /// something else is already on. Binding and releasing gives a port the
    /// kernel says is free, rather than a random number that might not be.
    #[test]
    fn a_free_port_is_bound_rather_than_guessed() {
        let port = free_port().expect("a port");
        assert!(port > 1024, "got a privileged port: {port}");
        // It must be free now, which is the whole point of asking the kernel.
        let taken = std::net::TcpListener::bind(("127.0.0.1", port)).is_ok();
        assert!(taken, "the port handed out was already in use");
    }
    #[test]
    fn the_engines_own_offsets_map_to_the_right_lines() {
        let lyrics = "[Verse 1]\nhello there my friend\n[Chorus]\nsing it loud now";
        let words: Vec<AlignedWord> = vec![
            (0.0, 10, 15),
            (1.044176697731018, 16, 21),
            (1.1646586656570435, 22, 24),
            (1.2048193216323853, 25, 31),
            (1.5863454341888428, 41, 45),
            (2.690762996673584, 46, 48),
            (3.614457845687866, 49, 53),
            (4.799196720123291, 54, 57),
        ]
        .into_iter()
        .map(|(start, c0, c1)| AlignedWord {
            start,
            end: start + 0.5,
            score: 0.005,
            char0: c0,
            char1: c1,
        })
        .collect();

        let lrc = aligned_lrc(lyrics, &words);
        let lines: Vec<&str> = lrc.lines().collect();
        assert_eq!(lines.len(), 4, "{lrc}");
        assert_eq!(lines[0], "[00:00.00][Verse 1]");
        assert_eq!(lines[1], "[00:00.00]hello there my friend");
        assert_eq!(lines[2], "[00:01.59][Chorus]");
        assert_eq!(lines[3], "[00:01.59]sing it loud now");
    }

    /// The engine answers with more than the words, and the extra keys must not
    /// stop the response from parsing.
    #[test]
    fn the_extra_keys_the_engine_returns_are_ignored() {
        let body = r#"{"audio_s":5.0,"frames":249,"model":"mms-fa-f32.gguf",
            "words":[{"start":0.0,"end":1.0,"score":0.004,"char0":0,"char1":3}]}"#;
        let parsed: AlignResponse = serde_json::from_str(body).expect("the real shape parses");
        assert_eq!(parsed.words.len(), 1);
        assert_eq!(parsed.words[0].char0, 0);
    }
    #[test]
    fn empty_lyrics_produce_nothing() {
        assert_eq!(aligned_lrc("", &[]), "");
        assert_eq!(aligned_lrc("   \n  \n", &[]), "");
    }


}
