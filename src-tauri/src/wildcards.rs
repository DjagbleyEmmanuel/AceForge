//! Seeded wildcards: `{drums|bass|full band}` in a caption or lyrics expands to
//! one alternative.
//!
//! Expansion happens in Rust, before the request is submitted, rather than in
//! the UI, for two reasons. The backend resolves a negative seed internally with
//! `std::random_device` and never reports the value it chose, so a wildcard
//! expanded in the browser could not be tied to the seed that produced the
//! audio. Doing it here also means the expanded text is what gets stored on the
//! job, so a retry renders the same prompt rather than a fresh roll of it.

/// mulberry32: small, fast, and fully specified. `std`'s RNG is not guaranteed
/// stable across releases, and a wildcard that reshuffles after a toolchain
/// bump would silently break reproducibility.
struct Rng(u32);

impl Rng {
    fn new(seed: i64) -> Self {
        // Mix the seed first. Seeds are often small or sequential (1, 2, 3) and
        // mulberry32's first output for low seeds is visibly correlated.
        let mut z = (seed as u64 as u32) ^ 0x9e37_79b9;
        z = z.wrapping_mul(0x85eb_ca6b);
        z ^= z >> 13;
        Rng(z)
    }

    fn next_u32(&mut self) -> u32 {
        self.0 = self.0.wrapping_add(0x6d2b_79f5);
        let mut t = self.0;
        t = (t ^ (t >> 15)).wrapping_mul(t | 1);
        t ^= t.wrapping_add((t ^ (t >> 7)).wrapping_mul(t | 61));
        t ^ (t >> 14)
    }

    /// Unbiased index in `0..n` via rejection sampling. `n % bound` on the raw
    /// output would bias low indices, which is visible once a wildcard has a
    /// handful of options and you render a few hundred times.
    fn below(&mut self, n: usize) -> usize {
        if n <= 1 {
            return 0;
        }
        let bound = n as u32;
        let limit = u32::MAX - (u32::MAX % bound);
        loop {
            let v = self.next_u32();
            if v < limit {
                return (v % bound) as usize;
            }
        }
    }
}

/// Replace a negative (random) seed with a concrete one and return it.
///
/// Every render becomes reproducible, and the value is recorded on the job so
/// the same prompt can be rebuilt later.
pub fn resolve_seed(seed: i64) -> i64 {
    if seed >= 0 {
        return seed;
    }
    // `as u64` on the low bits then into i64 keeps this positive: a negative
    // seed is an explicit sentinel, not a value the engine should ever see.
    (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as i64 ^ d.as_secs() as i64)
        .unwrap_or(0)
        .unsigned_abs() as i64)
        & 0x7fff_ffff
}

/// Expand every `{a|b|c}` in `text`, choosing with `rng`.
///
/// Escaped with a backslash: `\{` and `\}` are literal braces. A brace group
/// with no `|` is left untouched, so lyrics and captions that contain ordinary
/// braces are not silently rewritten.
fn expand_with(text: &str, rng: &mut Rng) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;

    while i < bytes.len() {
        match bytes[i] {
            b'\\' if i + 1 < bytes.len() && (bytes[i + 1] == b'{' || bytes[i + 1] == b'}') => {
                out.push(bytes[i + 1] as char);
                i += 2;
            }
            b'{' => {
                // Scan to the matching close, collecting alternatives. UTF-8
                // multi-byte sequences never contain ASCII bytes, so walking
                // byte-by-byte cannot split a character.
                let start = i + 1;
                let mut end = start;
                while end < bytes.len() && bytes[end] != b'}' {
                    end += 1;
                }
                if end >= bytes.len() {
                    // Unterminated: emit the rest verbatim rather than guessing.
                    out.push_str(&text[i..]);
                    return out;
                }
                let inner = &text[start..end];
                let options: Vec<&str> = inner.split('|').collect();
                if options.len() < 2 {
                    out.push_str(&text[i..=end]);
                } else {
                    let pick = options[rng.below(options.len())];
                    out.push_str(pick);
                }
                i = end + 1;
            }
            _ => {
                // Copy one whole UTF-8 character.
                let start = i;
                i += 1;
                while i < bytes.len() && (bytes[i] & 0xc0) == 0x80 {
                    i += 1;
                }
                out.push_str(&text[start..i]);
            }
        }
    }
    out
}

/// Expand all wildcards in one string from one seed.
pub fn expand(text: &str, seed: i64) -> String {
    expand_with(text, &mut Rng::new(seed))
}

/// Apply wildcards across the prompt fields that can contain them.
///
/// Both fields are expanded from a single `Rng` so that the same seed picks
/// consistently across the whole prompt rather than restarting per field.
pub fn expand_prompt(caption: &str, lyrics: &str, seed: i64) -> (String, String) {
    let mut rng = Rng::new(seed);
    (expand_with(caption, &mut rng), expand_with(lyrics, &mut rng))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expansion_is_deterministic_for_a_seed() {
        let text = "{drums|bass|full band} in {C minor|A minor}";
        assert_eq!(expand(text, 42), expand(text, 42));
        assert_eq!(expand_prompt(text, "", 42).0, expand(text, 42));
    }

    #[test]
    fn different_seeds_can_pick_differently() {
        let text = "{a|b|c|d|e|f|g|h}";
        let picks: std::collections::HashSet<String> =
            (0..40).map(|s| expand(text, s)).collect();
        assert!(picks.len() > 1, "40 seeds produced one result: {picks:?}");
    }

    #[test]
    fn every_option_is_reachable() {
        let text = "{a|b|c}";
        let mut seen = std::collections::HashSet::new();
        for s in 0..200 {
            seen.insert(expand(text, s));
        }
        assert_eq!(seen.len(), 3, "expected all three options, got {seen:?}");
    }

    #[test]
    fn multiple_wildcards_in_one_string() {
        let out = expand("{x|y} and {p|q}", 7);
        let parts: Vec<&str> = out.split(" and ").collect();
        assert_eq!(parts.len(), 2);
        assert!(["x", "y"].contains(&parts[0]), "bad pick {}", parts[0]);
        assert!(["p", "q"].contains(&parts[1]), "bad pick {}", parts[1]);
    }

    #[test]
    fn single_option_is_left_alone() {
        assert_eq!(expand("use {braces} here", 3), "use {braces} here");
        assert_eq!(expand("{}", 3), "{}");
    }

    #[test]
    fn escaped_braces_are_literal() {
        assert_eq!(expand(r"\{a|b\}", 5), "{a|b}");
        assert_eq!(expand(r"\{not a wildcard\}", 5), "{not a wildcard}");
    }

    #[test]
    fn unterminated_brace_is_preserved_not_dropped() {
        assert_eq!(expand("intro {a|b outro", 9), "intro {a|b outro");
        assert_eq!(expand("tail }", 9), "tail }");
    }

    #[test]
    fn multibyte_options_survive_intact() {
        // Delimiters are ASCII and UTF-8 continuation bytes never look ASCII, so
        // a byte-wise scan must not split a character.
        let text = "{café|naïve|日本語}";
        for s in 0..30 {
            let out = expand(text, s);
            assert!(
                ["café", "naïve", "日本語"].contains(&out.as_str()),
                "seed {s} produced {out:?}"
            );
        }
    }

    #[test]
    fn empty_option_is_selectable() {
        // `{|text}` is a real idiom: it means "sometimes omit this word".
        let text = "intro {|big }chords";
        let mut saw_empty = false;
        for s in 0..60 {
            let out = expand(text, s);
            assert!(out == "intro big chords" || out == "intro chords", "{out:?}");
            if out == "intro chords" {
                saw_empty = true;
            }
        }
        assert!(saw_empty, "the empty option was never chosen");
    }

    #[test]
    fn caption_and_lyrics_draw_from_one_stream() {
        // Same seed, one prompt: the second field must not restart the stream,
        // or repeating a caption could pair with mismatched lyric choices.
        let (c, l) = expand_prompt("{a|b} cap", "{c|d} lyr", 11);
        assert_eq!((c.clone(), l.clone()), expand_prompt("{a|b} cap", "{c|d} lyr", 11));
        assert!(["a cap", "b cap"].contains(&c.as_str()), "caption was {c:?}");
        assert!(["c lyr", "d lyr"].contains(&l.as_str()), "lyrics were {l:?}");
    }

    #[test]
    fn resolve_seed_turns_the_random_sentinel_into_a_positive_value() {
        assert_eq!(resolve_seed(1234), 1234, "an explicit seed is untouched");
        for sentinel in [-1i64, -99, i64::MIN] {
            let s = resolve_seed(sentinel);
            assert!(s >= 0, "resolved {sentinel} to {s}, which is still a sentinel");
            assert!(s <= 0x7fff_ffff, "{s} is out of the engine's positive range");
        }
    }

    #[test]
    fn resolve_seed_does_not_repeat() {
        let a = resolve_seed(-1);
        std::thread::sleep(std::time::Duration::from_millis(2));
        let b = resolve_seed(-1);
        assert_ne!(a, b, "two random seeds collided at {a}");
    }
}