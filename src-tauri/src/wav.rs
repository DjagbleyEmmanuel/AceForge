//! WAV reading and writing for the mastering chain.
//!
//! ACE-Step writes 48 kHz stereo PCM, but anything the user imports may be
//! 16-bit, 24-bit, 32-bit int or 32-bit float, so the reader handles all four
//! and the writer emits 32-bit float. Float output is lossless for material
//! that has already been through a float stage and avoids the quantisation
//! step on every save.

/// Decoded audio, interleaved samples in -1.0..=1.0.
#[derive(Debug, Clone, PartialEq)]
pub struct Wav {
    pub sample_rate: u32,
    pub channels: u16,
    /// Interleaved: `[l, r, l, r, ...]`.
    pub samples: Vec<f32>,
}

impl Wav {
    pub fn frames(&self) -> usize {
        if self.channels == 0 {
            0
        } else {
            self.samples.len() / self.channels as usize
        }
    }

    pub fn duration_secs(&self) -> f32 {
        if self.sample_rate == 0 {
            0.0
        } else {
            self.frames() as f32 / self.sample_rate as f32
        }
    }

    /// Keep only `[from_secs, to_secs)`. Returns `None` if the window is empty
    /// or inverted, rather than writing a zero-length file that no player can
    /// open.
    pub fn trim(&self, from_secs: f32, to_secs: f32) -> Option<Wav> {
        let rate = self.sample_rate as f32;
        if rate <= 0.0 {
            return None;
        }
        let start = (from_secs.max(0.0) * rate).floor().max(0.0) as usize;
        let end = ((to_secs.max(0.0) * rate).ceil() as usize).min(self.frames());
        if end <= start {
            return None;
        }
        let channels = self.channels as usize;
        Some(Wav {
            sample_rate: self.sample_rate,
            channels: self.channels,
            samples: self.samples[start * channels..end * channels].to_vec(),
        })
    }

    /// Remove `[from_secs, to_secs)`, joining what is left.
    ///
    /// Cutting a hole out of a track is a splice, not a fade: the two sides are
    /// butted together so the result is the same length as the parts that were
    /// kept. Returns `None` when the range covers everything, because that
    /// would leave nothing to write.
    pub fn delete_range(&self, from_secs: f32, to_secs: f32) -> Option<Wav> {
        let rate = self.sample_rate as f32;
        if rate <= 0.0 {
            return None;
        }
        let channels = self.channels as usize;
        let frames = self.frames();

        let cut_from = ((from_secs.max(0.0) * rate).floor() as usize).min(frames);
        let cut_to = ((to_secs.max(0.0) * rate).ceil() as usize).min(frames);
        if cut_to <= cut_from {
            return None;
        }
        if cut_from == 0 && cut_to >= frames {
            return None;
        }

        let mut samples = Vec::with_capacity(self.samples.len());
        samples.extend_from_slice(&self.samples[..cut_from * channels]);
        samples.extend_from_slice(&self.samples[cut_to * channels..]);

        Some(Wav {
            sample_rate: self.sample_rate,
            channels: self.channels,
            samples,
        })
    }

    /// Highest absolute sample value, i.e. the peak before any gain change.
    pub fn peak(&self) -> f32 {
        self.samples.iter().fold(0.0f32, |m, s| m.max(s.abs()))
    }
}

fn rd_u16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes([*b.get(at)?, *b.get(at + 1)?]))
}

fn rd_u32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes([
        *b.get(at)?,
        *b.get(at + 1)?,
        *b.get(at + 2)?,
        *b.get(at + 3)?,
    ]))
}

/// Read a WAV or an MP3 into interleaved float samples.
///
/// Renders default to MP3, so a reader that only spoke RIFF failed on exactly
/// the files this app produces: crop and normalise both reported "not a
/// RIFF/WAVE file" on every MP3 in the library. MP3 now decodes here rather
/// than being handed over the IPC bridge as samples -- a five minute stereo
/// track is nearly thirty million floats, which is not something to serialise
/// into JSON.
pub fn read(bytes: &[u8]) -> Result<Wav, String> {
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WAVE" {
        return read_wav(bytes);
    }
    if let Ok(mp3) = read_mp3(bytes) {
        return Ok(mp3);
    }
    if bytes.len() < 44 {
        return Err("file is too short to be audio".to_string());
    }
    Err("not a WAV or MP3 file".to_string())
}

fn read_wav(bytes: &[u8]) -> Result<Wav, String> {
    if bytes.len() < 44 {
        return Err("file is too short to be a WAV".to_string());
    }

    // Chunk walking, because a WAV may carry LIST/fact/cue chunks before the
    // data. Assuming a fixed 44-byte header is the classic way to misread a
    // perfectly valid file as empty.
    let mut fmt: Option<(u16, u32, u16, u16)> = None; // (tag, rate, channels, bits)
    let mut data: Option<(usize, usize)> = None; // (offset, length)

    let mut pos = 12usize;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = rd_u32(bytes, pos + 4).ok_or("truncated chunk header")? as usize;
        let body = pos + 8;

        if id == b"fmt " {
            let tag = rd_u16(bytes, body).ok_or("truncated fmt chunk")?;
            let channels = rd_u16(bytes, body + 2).ok_or("truncated fmt chunk")?;
            let rate = rd_u32(bytes, body + 4).ok_or("truncated fmt chunk")?;
            // Bits per sample lives at +14 in the fmt chunk and is what decides
            // the sample width. Deriving it from the format tag is wrong: PCM
            // (tag 1) covers 8, 16, 24 and 32 bits alike.
            let bits = rd_u16(bytes, body + 14).ok_or("truncated fmt chunk")?;
            // 0xFFFE is WAVE_FORMAT_EXTENSIBLE: the real tag is the first two
            // bytes of the sub-format GUID at +24.
            let resolved = if tag == 0xFFFE {
                rd_u16(bytes, body + 24).unwrap_or(0)
            } else {
                tag
            };
            fmt = Some((resolved, rate, channels, bits));
        } else if id == b"data" {
            // A streamed WAV can declare a size of 0 or 0xFFFFFFFF; clamp to
            // what is actually present rather than rejecting the file.
            let avail = bytes.len().saturating_sub(body);
            data = Some((body, size.min(avail)));
        }

        // Chunks are word-aligned: an odd size is followed by a pad byte.
        pos = body + size + (size & 1);
    }

    let (format, sample_rate, channels, bits) =
        fmt.ok_or_else(|| "no fmt chunk".to_string())?;
    let (offset, length) = data.ok_or_else(|| "no data chunk".to_string())?;

    if channels == 0 {
        return Err("file declares zero channels".to_string());
    }
    if sample_rate == 0 {
        return Err("file declares a sample rate of zero".to_string());
    }

    let is_float = matches!(format, 3);
    match format {
        1 | 3 | 6 | 7 => {}
        other => return Err(format!("unsupported WAV format tag {other}")),
    }
    let bps = (bits / 8) as usize;
    if !matches!(bps, 1 | 2 | 3 | 4) {
        return Err(format!("unsupported sample width: {bits} bits"));
    }
    if is_float && !matches!(bps, 4 | 8) {
        return Err(format!("unsupported float width: {bits} bits"));
    }

    let frame_bytes = bps * channels as usize;
    if frame_bytes == 0 {
        return Err("invalid frame size".to_string());
    }
    let frames = length / frame_bytes;
    let mut samples = Vec::with_capacity(frames * channels as usize);

    for f in 0..frames {
        for c in 0..channels as usize {
            let at = offset + f * frame_bytes + c * bps;
            let v = if is_float {
                match bps {
                    4 => f32::from_le_bytes([
                        *bytes.get(at).ok_or("truncated float sample")?,
                        *bytes.get(at + 1).ok_or("truncated float sample")?,
                        *bytes.get(at + 2).ok_or("truncated float sample")?,
                        *bytes.get(at + 3).ok_or("truncated float sample")?,
                    ]),
                    _ => f64::from_le_bytes([
                        *bytes.get(at).ok_or("truncated float sample")?,
                        *bytes.get(at + 1).ok_or("truncated float sample")?,
                        *bytes.get(at + 2).ok_or("truncated float sample")?,
                        *bytes.get(at + 3).ok_or("truncated float sample")?,
                        *bytes.get(at + 4).ok_or("truncated float sample")?,
                        *bytes.get(at + 5).ok_or("truncated float sample")?,
                        *bytes.get(at + 6).ok_or("truncated float sample")?,
                        *bytes.get(at + 7).ok_or("truncated float sample")?,
                    ]) as f32,
                }
            } else {
                match bps {
                    1 => {
                        // 8-bit PCM is unsigned with a 128 bias.
                        let raw = *bytes.get(at).ok_or("truncated 8-bit sample")?;
                        (raw as f32 - 128.0) / 128.0
                    }
                    2 => {
                        let raw = rd_u16(bytes, at).ok_or("truncated 16-bit sample")?;
                        (raw as i16) as f32 / 32768.0
                    }
                    3 => {
                        let b0 = *bytes.get(at).ok_or("truncated 24-bit sample")? as i32;
                        let b1 = *bytes.get(at + 1).ok_or("truncated 24-bit sample")? as i32;
                        let b2 = *bytes.get(at + 2).ok_or("truncated 24-bit sample")? as i32;
                        // Assemble little-endian, then sign-extend from bit 23.
                        let raw = b0 | (b1 << 8) | (b2 << 16);
                        let signed = if raw & 0x0080_0000 != 0 {
                            raw | !0x00ff_ffff
                        } else {
                            raw
                        };
                        signed as f32 / 8_388_608.0
                    }
                    _ => {
                        let raw = rd_u32(bytes, at).ok_or("truncated 32-bit sample")?;
                        (raw as i32) as f32 / 2_147_483_648.0
                    }
                }
            };
            samples.push(v);
        }
    }

    Ok(Wav {
        sample_rate,
        channels,
        samples,
    })
}

/// Encode as 32-bit float PCM, which preserves everything the mastering chain
/// produced.
pub fn write(wav: &Wav) -> Vec<u8> {
    let data_len = wav.samples.len() * 4;
    let mut out = Vec::with_capacity(44 + data_len);

    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&((36 + data_len) as u32).to_le_bytes());
    out.extend_from_slice(b"WAVE");

    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // PCM fmt chunk size
    out.extend_from_slice(&3u16.to_le_bytes()); // IEEE float
    out.extend_from_slice(&wav.channels.to_le_bytes());
    out.extend_from_slice(&wav.sample_rate.to_le_bytes());
    let byte_rate = wav.sample_rate * wav.channels as u32 * 4;
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&((wav.channels * 4) as u16).to_le_bytes()); // block align
    out.extend_from_slice(&32u16.to_le_bytes()); // bits per sample

    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data_len as u32).to_le_bytes());
    for s in &wav.samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// Decode MP3 to interleaved float samples with minimp3 (CC0, public domain).
///
/// Vendored rather than pulled in as a crate: the app builds offline against a
/// fixed dependency set, and this is the same decoder the engine already uses,
/// so the app and the engine agree on what a render sounds like.
fn read_mp3(bytes: &[u8]) -> Result<Wav, String> {
    let mut decoder = Mp3Decoder::new();
    let mut pcm: Vec<i16> = Vec::new();
    let mut rate = 0u32;
    let mut channels = 0u16;

    let mut offset = 0usize;
    while offset < bytes.len() {
        let info = decoder.decode_frame(&bytes[offset..]);
        if info.frame_bytes == 0 {
            break;
        }
        offset += info.frame_bytes;
        // The first frame that reports a format is the one to trust; skip the
        // padding some encoders put in front of the stream.
        if info.channels == 0 || info.sample_rate == 0 {
            continue;
        }
        rate = info.sample_rate as u32;
        channels = info.channels as u16;
        pcm.extend_from_slice(&info.samples);
    }

    if pcm.is_empty() || channels == 0 || rate == 0 {
        return Err("no MP3 frames decoded".to_string());
    }

    // i16 to float: 32768 rather than 32767 so that full negative scale maps to
    // -1.0 exactly instead of one step inside it.
    let samples = pcm
        .into_iter()
        .map(|v| v as f32 / 32768.0)
        .collect::<Vec<f32>>();
    Ok(Wav {
        sample_rate: rate,
        channels,
        samples,
    })
}

/// FFI surface of the vendored minimp3 decoder.
mod mp3 {
    use std::os::raw::{c_int, c_uchar};

    /// Opaque decoder state. Its layout belongs to the C side, so it is sized
    /// here rather than described: getting this wrong makes the decoder write
    /// past the struct and return garbage frame lengths rather than failing.
    #[repr(C)]
    pub struct Mp3dec {
        _private: [u8; 6668],
    }

    #[repr(C)]
    #[derive(Default)]
    pub struct Mp3decFrameInfo {
        pub frame_bytes: c_int,
        pub frame_offset: c_int,
        pub channels: c_int,
        /// Sample rate in Hz.
        pub hz: c_int,
        pub layer: c_int,
        pub bitrate_kbps: c_int,
    }

    /// The decoder hands back PCM for the frame, at most this many samples.
    pub const MAX_SAMPLES_PER_FRAME: usize = 1152 * 2;

    extern "C" {
        pub fn mp3dec_init(dec: *mut Mp3dec);
        pub fn mp3dec_decode_frame(
            dec: *mut Mp3dec,
            mp3: *const c_uchar,
            mp3_bytes: c_int,
            pcm: *mut i16,
            info: *mut Mp3decFrameInfo,
        ) -> c_int;
    }
}

use mp3::{
    mp3dec_decode_frame, mp3dec_init, Mp3dec, Mp3decFrameInfo, MAX_SAMPLES_PER_FRAME,
};

struct FrameInfo {
    frame_bytes: usize,
    samples: Vec<i16>,
    channels: usize,
    sample_rate: usize,
}

/// The FFI structs mirror C layout, and a mismatch there is memory corruption
/// rather than a compile error. These catch a drift in minimp3 at build time.
const _: () = {
    assert!(std::mem::size_of::<Mp3dec>() == 6668);
    assert!(std::mem::size_of::<Mp3decFrameInfo>() == 24);
    assert!(std::mem::size_of::<i16>() == 2);
};

struct Mp3Decoder {
    state: Mp3dec,
    pcm: [i16; MAX_SAMPLES_PER_FRAME],
}

impl Mp3Decoder {
    fn new() -> Self {
        // Every field in mp3dec_t is a plain integer, so all-zero is a valid
        // starting point, which is what mp3dec_init documents being handed.
        let mut state: Mp3dec = unsafe { std::mem::zeroed() };
        // Safety: mp3dec_init only writes to the struct it is given.
        unsafe { mp3dec_init(&mut state) };
        Mp3Decoder {
            state,
            pcm: [0; MAX_SAMPLES_PER_FRAME],
        }
    }

    fn decode_frame(&mut self, data: &[u8]) -> FrameInfo {
        let mut info = Mp3decFrameInfo::default();
        // Safety: `data` is a live slice for the duration of the call and the
        // output buffer is sized to the maximum a frame can produce, which is
        // the contract mp3dec_decode_frame documents.
        let produced = unsafe {
            mp3dec_decode_frame(
                &mut self.state,
                data.as_ptr(),
                data.len() as std::os::raw::c_int,
                self.pcm.as_mut_ptr(),
                &mut info,
            )
        };
        // The return is samples *per channel*; the buffer it filled holds that
        // many per channel, interleaved. Slicing it as if it were already
        // interleaved keeps only half the audio and pairs left samples with
        // right ones, which sounds like it is playing at half speed with the
        // channels swapped.
        let channels = info.channels.max(1) as usize;
        let produced = (produced.max(0) as usize).saturating_mul(channels);
        let produced = produced.min(MAX_SAMPLES_PER_FRAME);
        FrameInfo {
            frame_bytes: info.frame_bytes.max(0) as usize,
            samples: self.pcm[..produced].to_vec(),
            channels: info.channels.max(0) as usize,
            sample_rate: info.hz.max(0) as usize,
        }
    }
}

#[cfg(test)]
mod mp3_reads {
    /// 1.2s of 440 Hz stereo at 44.1 kHz, written by ffmpeg as a stand-in for
    /// what a render leaves in the library.
    const TONE: &[u8] = include_bytes!("testdata/tone.mp3");

    /// Renders default to MP3 and `read` only spoke RIFF, so crop and normalise
    /// reported "not a RIFF/WAVE file" on every track in the library.
    #[test]
    fn reads_mp3() {
        let w = super::read(TONE).expect("an MP3 must decode");
        assert_eq!(w.channels, 2);
        assert_eq!(w.sample_rate, 44100);
        // Encoder delay means the decoded length runs a little past the
        // nominal 1.2s; what matters is that it is not truncated to half.
        assert!(
            w.duration_secs() > 1.15 && w.duration_secs() < 1.45,
            "decoded {}s, expected about 1.2",
            w.duration_secs()
        );
        assert!(w.peak() > 0.6, "tone decoded at peak {}", w.peak());
    }

    /// The decoder reports samples *per channel* while filling an interleaved
    /// buffer. Slicing the buffer as if it were already interleaved keeps half
    /// the audio and pairs left samples with right ones -- a track that plays
    /// at half speed with swapped channels, which still looks like plausible
    /// length data and is easy to miss.
    #[test]
    fn keeps_both_channels_and_the_full_length() {
        let w = super::read(TONE).expect("decode");
        let ch = w.channels as usize;
        let frames = w.samples.len() / ch;
        assert!(frames > 50000, "only {frames} frames survived");

        // A 440 Hz tone is identical in both channels, so they must agree
        // closely. If the interleave were wrong the pairs would be offset and
        // this correlation would collapse.
        let mut dot = 0f64;
        let mut ll = 0f64;
        let mut rr = 0f64;
        for i in 0..frames {
            let l = w.samples[i * ch] as f64;
            let r = w.samples[i * ch + 1] as f64;
            dot += l * r;
            ll += l * l;
            rr += r * r;
        }
        let corr = dot / (ll.sqrt() * rr.sqrt());
        assert!(corr > 0.95, "channels disagree, correlation {corr}");
    }

    /// The loudness chain has to see the same levels the engine produced.
    #[test]
    fn full_scale_mp3_reports_a_sane_peak() {
        let w = super::read(TONE).expect("decode");
        assert!(w.peak() <= 1.0, "i16 scaled by 32768 cannot exceed 1.0");
        assert!(w.peak() > 0.6, "peak was only {}", w.peak());
    }

    #[test]
    fn rejects_bytes_that_are_neither() {
        assert!(super::read(b"this is not audio at all").is_err());
        assert!(super::read(&[0u8; 8]).is_err());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(frames: usize, channels: u16, rate: u32) -> Wav {
        let mut samples = Vec::new();
        for i in 0..frames {
            let v = (i as f32 * 0.01).sin() * 0.5;
            for _ in 0..channels {
                samples.push(v);
            }
        }
        Wav {
            sample_rate: rate,
            channels,
            samples,
        }
    }

    #[test]
    fn round_trips_float_wav() {
        let original = tone(1000, 2, 48000);
        let bytes = write(&original);
        let decoded = read(&bytes).expect("our own output must read back");
        assert_eq!(decoded.sample_rate, original.sample_rate);
        assert_eq!(decoded.channels, original.channels);
        assert_eq!(decoded.samples.len(), original.samples.len());
        for (a, b) in original.samples.iter().zip(&decoded.samples) {
            assert!((a - b).abs() < 1e-6, "{a} vs {b}");
        }
    }

    #[test]
    fn reads_16_bit_pcm_at_full_scale() {
        // Two frames of stereo: -32768 and 32767.
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&36u32.to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes()); // PCM
        b.extend_from_slice(&2u16.to_le_bytes()); // channels
        b.extend_from_slice(&44100u32.to_le_bytes());
        b.extend_from_slice(&176400u32.to_le_bytes());
        b.extend_from_slice(&4u16.to_le_bytes());
        b.extend_from_slice(&16u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&8u32.to_le_bytes());
        for v in [i16::MIN, i16::MAX, 0, 12345] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        let wav = read(&b).expect("16-bit PCM should decode");
        assert_eq!(wav.sample_rate, 44100);
        assert_eq!(wav.channels, 2);
        assert_eq!(wav.samples.len(), 4);
        assert!((wav.samples[0] + 1.0).abs() < 1e-6, "{}", wav.samples[0]);
        assert!((wav.samples[1] - 32767.0 / 32768.0).abs() < 1e-6);
    }

    /// A canonical 16-bit stereo fmt chunk, so the header-building tests do not
    /// each hand-assemble one and get the layout wrong independently.
    fn fmt_chunk(rate: u32, channels: u16, bits: u16, tag: u16, tag_bytes: &[u8]) -> Vec<u8> {
        let mut f = Vec::new();
        f.extend_from_slice(b"fmt ");
        f.extend_from_slice(&(16 + tag_bytes.len() as u32).to_le_bytes());
        f.extend_from_slice(&tag.to_le_bytes());
        f.extend_from_slice(&channels.to_le_bytes());
        f.extend_from_slice(&rate.to_le_bytes());
        f.extend_from_slice(&(rate * channels as u32 * (bits as u32 / 8)).to_le_bytes());
        f.extend_from_slice(&((channels * (bits / 8)) as u16).to_le_bytes());
        f.extend_from_slice(&bits.to_le_bytes());
        f.extend_from_slice(tag_bytes);
        f
    }

    #[test]
    fn finds_data_past_an_extra_chunk() {
        // A LIST chunk before `data` is common in exported files. A parser that
        // assumes a 44-byte header reads zero samples here.
        let body: Vec<u8> = tone(64, 2, 48000).samples.iter().flat_map(|v| v.to_le_bytes()).collect();
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&((36 + 18 + body.len()) as u32).to_le_bytes());
        b.extend_from_slice(b"WAVE");
        b.extend_from_slice(&fmt_chunk(48000, 2, 32, 3, &[]));
        b.extend_from_slice(b"LIST");
        b.extend_from_slice(&10u32.to_le_bytes());
        b.extend_from_slice(b"INFOhello!");
        b.extend_from_slice(b"data");
        b.extend_from_slice(&(body.len() as u32).to_le_bytes());
        b.extend_from_slice(&body);
        let wav = read(&b).expect("extra chunk should be skipped");
        assert_eq!(wav.samples.len(), 128);
        assert_eq!(wav.sample_rate, 48000);
    }

    #[test]
    fn odd_sized_chunk_is_word_aligned() {
        // A chunk with an odd length is followed by a pad byte. Misreading it
        // puts the next chunk header off by one and loses the audio.
        let wav = tone(32, 2, 48000);
        let body: Vec<u8> = wav.samples.iter().flat_map(|v| v.to_le_bytes()).collect();
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&((36 + 12 + body.len()) as u32).to_le_bytes());
        b.extend_from_slice(b"WAVE");
        b.extend_from_slice(&fmt_chunk(48000, 2, 32, 3, &[]));
        b.extend_from_slice(b"junk");
        b.extend_from_slice(&3u32.to_le_bytes());
        b.extend_from_slice(b"abc");
        b.push(0); // pad byte
        b.extend_from_slice(b"data");
        b.extend_from_slice(&(body.len() as u32).to_le_bytes());
        b.extend_from_slice(&body);
        let decoded = read(&b).expect("padded chunk should parse");
        assert_eq!(decoded.samples.len(), wav.samples.len());
    }

    #[test]
    fn extensible_format_resolves_its_subformat() {
        // 0xFFFE carries the real tag in the first two bytes of the
        // sub-format GUID. Without resolving it, every extensible file reads as
        // an unknown format.
        let wav = tone(16, 2, 48000);
        let body: Vec<u8> = wav.samples.iter().flat_map(|v| v.to_le_bytes()).collect();
        // cbSize(2) + validBits(2) + channelMask(4) + 16-byte GUID.
        let mut ext = 22u16.to_le_bytes().to_vec();
        ext.extend_from_slice(&32u16.to_le_bytes());
        ext.extend_from_slice(&3u32.to_le_bytes());
        ext.extend_from_slice(&3u16.to_le_bytes()); // KSDATAFORMAT_SUBTYPE_IEEE_FLOAT
        ext.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00]);
        ext.extend_from_slice(&[0x00, 0xaa, 0x00, 0x38, 0x9b, 0x71, 0x00, 0x00]);

        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&((36 + 16 + ext.len() + 8 + body.len()) as u32).to_le_bytes());
        b.extend_from_slice(b"WAVE");
        b.extend_from_slice(&fmt_chunk(48000, 2, 32, 0xFFFE, &ext));
        b.extend_from_slice(b"data");
        b.extend_from_slice(&(body.len() as u32).to_le_bytes());
        b.extend_from_slice(&body);

        let decoded = read(&b).expect("extensible float should decode");
        assert_eq!(decoded.sample_rate, 48000);
        assert_eq!(decoded.samples.len(), wav.samples.len());
    }

    #[test]
    fn truncated_data_is_read_not_rejected() {
        // A data size larger than the bytes present is what an interrupted
        // write leaves behind. Reading what is there beats refusing the file.
        let wav = tone(100, 2, 48000);
        let full = write(&wav);
        let data_size_at = full.len() - (wav.samples.len() * 4);
        let mut b = full[..data_size_at + 40].to_vec(); // chop 40 bytes of audio
        b[data_size_at..data_size_at + 4]
            .copy_from_slice(&((wav.samples.len() * 4) as u32).to_le_bytes());

        let decoded = read(&b).expect("truncated file should still read");
        assert!(decoded.frames() < wav.frames() && decoded.frames() > 0);
    }

    #[test]
    fn reads_24_bit_pcm_with_sign_extension() {
        // 24-bit is the format a lot of hardware exports use, and its top bit
        // lives in byte 2: ignoring it turns every negative sample positive.
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&45u32.to_le_bytes());
        b.extend_from_slice(b"WAVE");
        b.extend_from_slice(&fmt_chunk(48000, 1, 24, 1, &[]));
        b.extend_from_slice(b"data");
        b.extend_from_slice(&9u32.to_le_bytes());
        // -8388608 (most negative), 0, 8388607 (most positive)
        b.extend_from_slice(&[0x00, 0x00, 0x80]);
        b.extend_from_slice(&[0x00, 0x00, 0x00]);
        b.extend_from_slice(&[0xff, 0xff, 0x7f]);

        let wav = read(&b).expect("24-bit PCM should decode");
        assert!((wav.samples[0] + 1.0).abs() < 1e-6, "{}", wav.samples[0]);
        assert!(wav.samples[1].abs() < 1e-9);
        assert!((wav.samples[2] - 0.999_999_88).abs() < 1e-6, "{}", wav.samples[2]);
    }

    #[test]
    fn trim_keeps_only_the_requested_window() {
        let w = tone(48000, 2, 48000); // one second
        let trimmed = w.trim(0.25, 0.75).expect("a valid window must trim");
        assert_eq!(trimmed.channels, w.channels);
        // Half a second, within one frame of rounding at each edge.
        assert!((trimmed.frames() as i64 - 24000).abs() <= 1, "{}", trimmed.frames());
        assert!((trimmed.duration_secs() - 0.5).abs() < 1e-4);
    }

    #[test]
    fn trim_preserves_the_samples_it_keeps() {
        // Each frame is its own value, so the trimmed audio can be checked
        // against the source rather than just its length.
        let mut samples = Vec::new();
        for i in 0..100 {
            samples.push(i as f32 / 1000.0);
        }
        let w = Wav {
            sample_rate: 1000,
            channels: 1,
            samples,
        };
        let trimmed = w.trim(0.010, 0.013).unwrap();
        assert_eq!(trimmed.samples.len(), 3);
        assert!((trimmed.samples[0] - 0.010).abs() < 1e-6, "{}", trimmed.samples[0]);
        assert!((trimmed.samples[2] - 0.012).abs() < 1e-6, "{}", trimmed.samples[2]);
    }

    #[test]
    fn an_empty_or_inverted_window_is_refused() {
        let w = tone(1000, 2, 48000);
        assert!(w.trim(2.0, 1.0).is_none(), "inverted");
        assert!(w.trim(1.0, 1.0).is_none(), "zero width");
        assert!(w.trim(99.0, 100.0).is_none(), "entirely past the end");
    }

    #[test]
    fn trimming_past_the_end_clamps_rather_than_failing() {
        let w = tone(48000, 2, 48000); // one second
        let trimmed = w.trim(0.5, 99.0).expect("a wide window still has content");
        assert_eq!(trimmed.frames(), w.frames() / 2);
    }

    #[test]
    fn trim_output_survives_a_write_and_read() {
        let w = tone(48000, 2, 48000);
        let trimmed = w.trim(0.1, 0.4).unwrap();
        let back = read(&write(&trimmed)).expect("a trimmed file must be valid");
        assert_eq!(back.frames(), trimmed.frames());
    }

    #[test]
    fn delete_range_joins_the_parts_that_were_kept() {
        // 2 s at 1 kHz, mono, one frame per millisecond, each frame its own
        // value so the join can be checked and not just counted.
        let samples: Vec<f32> = (0..2000).map(|i| i as f32).collect();
        let w = Wav {
            sample_rate: 1000,
            channels: 1,
            samples,
        };
        let cut = w.delete_range(0.5, 1.0).expect("a middle cut is valid");
        assert_eq!(cut.frames(), 1500);
        assert!((cut.duration_secs() - 1.5).abs() < 1e-6);
        // The seam must be the cut point: frame 500 follows frame 999.
        assert!((cut.samples[500] - 1000.0).abs() < 1e-6, "{}", cut.samples[500]);
        assert!((cut.samples[499] - 499.0).abs() < 1e-6, "{}", cut.samples[499]);
    }

    #[test]
    fn delete_range_at_either_end() {
        let w = Wav {
            sample_rate: 1000,
            channels: 1,
            samples: (0..1000).map(|i| i as f32).collect(),
        };
        let head = w.delete_range(0.0, 0.4).unwrap();
        assert_eq!(head.frames(), 600);
        assert!((head.samples[0] - 400.0).abs() < 1e-6);

        let tail = w.delete_range(0.6, 1.0).unwrap();
        assert_eq!(tail.frames(), 600);
    }

    #[test]
    fn deleting_everything_or_nothing_is_refused() {
        let w = Wav {
            sample_rate: 1000,
            channels: 1,
            samples: vec![0.0; 1000],
        };
        assert!(w.delete_range(0.0, 10.0).is_none(), "covers the whole track");
        assert!(w.delete_range(0.5, 0.5).is_none(), "zero width");
        assert!(w.delete_range(1.0, 0.5).is_none(), "inverted");
    }

    #[test]
    fn a_deleted_section_survives_a_write_and_read() {
        let w = Wav {
            sample_rate: 1000,
            channels: 2,
            samples: (0..2000).map(|i| (i as f32) / 2000.0).collect(),
        };
        let cut = w.delete_range(0.25, 0.75).unwrap();
        let back = read(&write(&cut)).expect("a spliced file must be valid");
        assert_eq!(back.frames(), cut.frames());
        assert_eq!(back.channels, 2);
    }

    #[test]
    fn duration_and_peak_are_derived_not_stored() {
        let wav = tone(48000, 2, 48000);
        assert!((wav.duration_secs() - 1.0).abs() < 1e-6);
        assert!(wav.peak() <= 0.5 + 1e-6);
    }
}
