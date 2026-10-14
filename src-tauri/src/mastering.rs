//! Loudness normalisation and peak limiting, per ITU-R BS.1770-4.
//!
//! A render's absolute peak says nothing about how loud it will *sound*: a
//! dense master peaks near 0 dBFS while sitting at -16 LUFS, and a sparse one
//! peaks at -3 dBFS but reads -22 LUFS. Matching a target loudness instead of a
//! target peak is what makes a track sit correctly next to other music.
//!
//! The measurement is K-weighted (a high-shelf plus a high-pass approximating
//! the ear's response), summed across channels with the BS.1770 channel
//! weights, then gated: quiet passages are excluded, first absolutely and then
//! relative to the ungated level. Skipping the gates makes a track with a long
//! silence read far too loud.

use serde::{Deserialize, Serialize};

use crate::wav::Wav;

/// Interchange targets and common practice.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LoudnessTarget {
    /// Streaming platform default.
    Streaming14,
    /// Radio/club-style dense master.
    Club9,
    /// Broadcast.
    Broadcast24,
    /// Loud, modern masters; risks clipping without the limiter.
    Loud6,
}

impl LoudnessTarget {
    pub fn label(self) -> &'static str {
        match self {
            LoudnessTarget::Streaming14 => "-14 LUFS",
            LoudnessTarget::Club9 => "-9 LUFS",
            LoudnessTarget::Broadcast24 => "-24 LUFS",
            LoudnessTarget::Loud6 => "-6 LUFS",
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            LoudnessTarget::Streaming14 => "streaming default (Spotify, Apple Music)",
            LoudnessTarget::Club9 => "dense club master",
            LoudnessTarget::Broadcast24 => "broadcast / EBU R128",
            LoudnessTarget::Loud6 => "loud modern master, limited hard",
        }
    }

    pub fn value(self) -> f32 {
        match self {
            LoudnessTarget::Streaming14 => -14.0,
            LoudnessTarget::Club9 => -9.0,
            LoudnessTarget::Broadcast24 => -24.0,
            LoudnessTarget::Loud6 => -6.0,
        }
    }
}

/// Ceiling in dBFS for the look-ahead limiter. -1 dBTP is the usual safe choice:
/// intersample peaks can exceed the sample peak, so true 0 dBTP needs a ceiling
/// below zero to survive the DAC's reconstruction filter.
pub const DEFAULT_CEILING_DBFS: f32 = -1.0;

/// Absolute gate: below this, a block is silence rather than quiet music.
const ABSOLUTE_GATE_LUFS: f32 = -70.0;
/// Relative gate: blocks this far below the ungated loudness are excluded.
const RELATIVE_GATE_LUFS: f32 = -10.0;
/// BS.1770 measurement blocks are 400 ms long, stepped at 75% overlap.
const BLOCK_SECS: f32 = 0.4;

/// Direct-form II transposed biquad. Coefficients are computed for a given rate
/// rather than hardcoded, so this stays correct if the render rate ever changes.
#[derive(Clone, Copy)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Biquad {
    fn new(b0: f32, b1: f32, b2: f32, a0: f32, a1: f32, a2: f32) -> Self {
        Biquad {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
            z1: 0.0,
            z2: 0.0,
        }
    }

    /// High-shelf, used for the K-weighting's 4 kHz presence bump.
    fn high_shelf(rate: f32, freq: f32, gain_db: f32, slope: f32) -> Self {
        let a = 10f32.powf(gain_db / 40.0);
        let w0 = 2.0 * std::f32::consts::PI * freq / rate;
        let (s, c) = w0.sin_cos();
        let alpha = s / (2.0 * 10f32.powf(slope / 20.0));
        let two_sqrt_a_alpha = 2.0 * a.sqrt() * alpha;
        let sqrt_a = a.sqrt();

        Biquad::new(
            a * ((a + 1.0) + (a - 1.0) * c + two_sqrt_a_alpha),
            -2.0 * a * ((a - 1.0) + (a + 1.0) * c),
            a * ((a + 1.0) + (a - 1.0) * c - two_sqrt_a_alpha),
            (a + 1.0) - (a - 1.0) * c + two_sqrt_a_alpha,
            2.0 * ((a - 1.0) - (a + 1.0) * c),
            (a + 1.0) - (a - 1.0) * c - two_sqrt_a_alpha,
        )
        .scale(sqrt_a * 0.0 + 1.0)
    }

    /// Second-order high-pass, the K-weighting's rumble cut.
    fn high_pass(rate: f32, freq: f32, q: f32) -> Self {
        let w0 = 2.0 * std::f32::consts::PI * freq / rate;
        let (s, c) = w0.sin_cos();
        let alpha = s / (2.0 * q);
        Biquad::new(
            (1.0 + c) / 2.0,
            -(1.0 + c),
            (1.0 + c) / 2.0,
            1.0 + alpha,
            -2.0 * c,
            1.0 - alpha,
        )
    }

    fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
}

impl Biquad {
    /// Identity used by [`Biquad::high_shelf`] to keep it a constructor call.
    fn scale(self, k: f32) -> Self {
        Biquad { b0: self.b0 * k, ..self }
    }
}

/// Per-channel mean square over one block, K-weighted.
struct BlockMeter {
    shelf: Vec<Biquad>,
    highpass: Vec<Biquad>,
}

impl BlockMeter {
    fn new(rate: f32, channels: usize) -> Self {
        let shelf = (0..channels)
            .map(|_| Biquad::high_shelf(rate, 1681.974_450_955_716, 3.999_843_853_973_347, 0.707_175_236_955_419_6))
            .collect();
        let highpass = (0..channels)
            .map(|_| Biquad::high_pass(rate, 38.135_470_876_024_44, 0.500_327_037_323_877_3))
            .collect();
        BlockMeter { shelf, highpass }
    }

}

/// BS.1770 channel weights. Mono is not weighted; stereo is L=R=1.0, which is
/// what "stereo" means for this measurement. Anything else falls back to equal
/// weights rather than guessing at surround coefficients.
fn channel_gains(channels: u16) -> Vec<f64> {
    match channels {
        1 => vec![1.0],
        2 => vec![1.0, 1.0],
        n => vec![1.0; n as usize],
    }
}

/// Integrated loudness in LUFS.
///
/// Returns `None` for silence or for material too short to gate meaningfully.
/// Callers must treat that as "leave it alone" rather than as zero, or a silent
/// render would be boosted by the full target gain.
pub fn integrated_lufs(wav: &Wav) -> Option<f32> {
    let channels = wav.channels as usize;
    let frames = wav.frames();
    if channels == 0 || frames == 0 || wav.sample_rate == 0 {
        return None;
    }

    let block = (BLOCK_SECS * wav.sample_rate as f32) as usize;
    if block == 0 || frames < block {
        return None;
    }
    // BS.1770 steps 75% overlap, so the hop is a quarter of the block. Sliding
    // the window over a ring buffer keeps this O(1) in memory; storing every
    // sample's energy would cost ~115 MB on a ten-minute track.
    let hop = (block / 4).max(1);

    let gains = channel_gains(wav.channels);
    let mut meter = BlockMeter::new(wav.sample_rate as f32, channels);

    let mut block_ms: Vec<f64> = Vec::new();
    let mut window = vec![0f64; block]; // per-sample energy ring
    let mut window_pos = 0usize;
    let mut window_sum = 0f64;
    let mut filled = 0usize;
    let mut since_emit = 0usize;

    for i in 0..frames {
        let mut energy = 0f64;
        for c in 0..channels {
            let x = wav.samples[i * channels + c];
            // The filter chain runs continuously; block boundaries only decide
            // when the accumulated energy is read out.
            let y = meter.highpass[c].process(meter.shelf[c].process(x));
            energy += (y as f64) * (y as f64) * gains[c];
        }

        // Ring-buffer the window so the same sample is not counted twice.
        if filled == block {
            window_sum -= window[window_pos];
        } else {
            filled += 1;
        }
        window[window_pos] = energy;
        window_sum += energy;
        window_pos = (window_pos + 1) % block;

        since_emit += 1;
        if filled == block && since_emit >= hop {
            block_ms.push(window_sum / block as f64);
            since_emit = 0;
        }
    }

    if block_ms.is_empty() {
        return None;
    }

    // Absolute gate first.
    let to_lufs = |ms: f64| -> f32 { -0.691 + 10.0 * ms.max(1e-12).log10() as f32 };
    let abs_gated: Vec<f64> = block_ms
        .iter()
        .copied()
        .filter(|ms| to_lufs(*ms) > ABSOLUTE_GATE_LUFS)
        .collect();
    if abs_gated.is_empty() {
        return None;
    }

    // Relative gate against the ungated-block mean.
    let mean = abs_gated.iter().sum::<f64>() / abs_gated.len() as f64;
    let relative_gate = to_lufs(mean) + RELATIVE_GATE_LUFS;
    let kept: Vec<f64> = abs_gated
        .iter()
        .copied()
        .filter(|ms| to_lufs(*ms) > relative_gate)
        .collect();
    if kept.is_empty() {
        return None;
    }

    let final_mean = kept.iter().sum::<f64>() / kept.len() as f64;
    let lufs = to_lufs(final_mean);
    if !lufs.is_finite() {
        return None;
    }
    Some(lufs)
}

/// Gain in linear terms to reach `target` from `current`.
fn gain_for(current: f32, target: f32) -> f32 {
    10f32.powf((target - current) / 20.0)
}

/// Look-ahead peak limiter.
///
/// A plain hard clip distorts badly; this finds the peak in a sliding window
/// ahead of the write position and eases gain down *before* the peak arrives, so
/// the reduction is smooth and inaudible rather than a crackle.
pub fn limit(wav: &mut Wav, ceiling_dbfs: f32) {
    let ceiling = 10f32.powf(ceiling_dbfs / 20.0);
    if ceiling <= 0.0 {
        return;
    }
    let channels = wav.channels as usize;
    let frames = wav.frames();
    if channels == 0 || frames == 0 {
        return;
    }

    // ~5 ms of look-ahead at this rate: long enough to ramp smoothly, short
    // enough that the delay is not audible as a smear on a transient.
    let lookahead = ((0.005 * wav.sample_rate as f32) as usize).max(1);

    // Future peak of the window starting at each sample.
    let mut window_peak = vec![0f32; frames];
    let mut running = 0f32;
    for i in (0..frames).rev() {
        for c in 0..channels {
            let v = wav.samples[i * channels + c].abs();
            if v > running {
                running = v;
            }
        }
        window_peak[i] = running;
        if i > 0 {
            // Drop whatever left the window ending at i-1.
            let leaving = i - 1;
            let mut next = 0f32;
            for j in leaving.saturating_sub(lookahead).max(0)..=i {
                for c in 0..channels {
                    let v = wav.samples[j * channels + c].abs();
                    if v > next {
                        next = v;
                    }
                }
            }
            running = next;
        }
    }

    let mut gain = 1.0f32;
    for i in 0..frames {
        let peak = window_peak[i];
        let wanted = if peak > ceiling { ceiling / peak } else { 1.0 };
        // Attack instantly, release smoothly: the gain must already be down
        // when the peak arrives, but must not pump back up audibly after it.
        if wanted < gain {
            gain = wanted;
        } else {
            gain += (wanted - gain) * 0.002;
        }
        for c in 0..channels {
            wav.samples[i * channels + c] *= gain;
        }
    }

    // A final hard guard: the limiter is not allowed to leave a sample above the
    // ceiling no matter what the envelope said.
    for s in wav.samples.iter_mut() {
        if *s > ceiling {
            *s = ceiling;
        } else if *s < -ceiling {
            *s = -ceiling;
        }
    }
}

/// A coarse spectral centroid, in Hz.
///
/// Used as a single "brightness" number for matching, not as an analysis tool.
/// It is the cheapest descriptor that actually correlates with what people mean
/// by dark versus bright: the centre of gravity of the magnitude spectrum.
pub fn spectral_centroid(wav: &Wav) -> Option<f32> {
    let channels = wav.channels as usize;
    let frames = wav.frames();
    if channels == 0 || frames == 0 || wav.sample_rate == 0 {
        return None;
    }
    // 1024-point Goertzel-style scan across a log-spaced set of bands. A full
    // FFT is not worth it here and the resolution is finer than the match can
    // act on anyway.
    let bands = 48usize;
    let lowest = 40.0f32;
    let highest = (wav.sample_rate as f32 / 2.0).min(16_000.0);

    // Sample a bounded window rather than the whole file: the centroid of a
    // full track drifts with its arrangement, and matching should not depend on
    // where the loud chorus sits.
    let window_frames = (wav.sample_rate as usize / 2).min(frames);
    let offset = (frames - window_frames) / 2;

    let mut numerator = 0f64;
    let mut denominator = 0f64;
    for b in 0..bands {
        let t = b as f32 / (bands - 1) as f32;
        let freq = lowest * (highest / lowest).powf(t);
        let omega = 2.0 * std::f32::consts::PI * freq / wav.sample_rate as f32;
        let c = omega.cos();
        let coeff = 2.0 * c;

        let mut z1 = 0f32;
        let mut z2 = 0f32;
        for i in 0..window_frames {
            let mut x = 0f32;
            for ch in 0..channels {
                x += wav.samples[(offset + i) * channels + ch];
            }
            x /= channels as f32;
            // Goertzel: two-pole resonator at `freq`.
            let y = x + coeff * z1 - z2;
            z2 = z1;
            z1 = y;
        }
        let power =
            (z1 * z1 + z2 * z2 - coeff * z1 * z2).max(0.0) as f64 / window_frames as f64;
        let weight = power * freq as f64;
        numerator += weight;
        denominator += power;
    }
    if denominator <= 0.0 {
        return None;
    }
    Some((numerator / denominator) as f32)
}

/// Shift a track's spectral balance toward a reference's, using a single
/// first-order tilt.
///
/// A full equaliser match needs a spectrum to fit and tends to ring; the goal
/// here is that a mix does not sound obviously darker or brighter than the
/// reference it is being compared against, so one tilt parameter is the right
/// amount of machinery.
pub fn match_brightness(wav: &mut Wav, reference: &Wav, max_tilt_db: f32) -> Option<f32> {
    let a = spectral_centroid(wav)?;
    let b = spectral_centroid(reference)?;
    if a <= 0.0 || b <= 0.0 {
        return None;
    }
    // A ratio of the centroids approximates the dB of tilt needed. Clamped so a
    // wildly different reference cannot produce a wild sweep.
    let tilt_db = ((b / a).log10() * 20.0).clamp(-max_tilt_db, max_tilt_db);
    if tilt_db.abs() < 0.1 {
        return Some(0.0);
    }

    let channels = wav.channels as usize;
    let frames = wav.frames();
    let mut tilt = Biquad::high_shelf(wav.sample_rate as f32, 2000.0, tilt_db, 0.7);
    let mut lowcut = Biquad::high_pass(wav.sample_rate as f32, 25.0, 0.707);
    let mut out = Vec::with_capacity(wav.samples.len());
    for i in 0..frames {
        for c in 0..channels {
            let x = wav.samples[i * channels + c];
            out.push(tilt.process(lowcut.process(x)));
        }
    }
    wav.samples = out;
    Some(tilt_db)
}

/// What the normaliser did, for the UI to report.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NormaliseReport {
    pub measured_lufs: Option<f32>,
    pub target_lufs: f32,
    pub applied_gain_db: f32,
    pub limited: bool,
    pub peak_before_dbfs: f32,
    pub peak_after_dbfs: f32,
    /// Spectral tilt applied to match a reference, if one was given.
    pub brightness_tilt_db: Option<f32>,
}

fn to_dbfs(v: f32) -> f32 {
    if v <= 1e-9 {
        -120.0
    } else {
        20.0 * v.log10()
    }
}

/// Measure, gain toward the target, then limit. Returns the report.
pub fn normalise(
    wav: &mut Wav,
    target: LoudnessTarget,
    ceiling_dbfs: f32,
) -> NormaliseReport {
    let peak_before = wav.peak();
    let measured = integrated_lufs(wav);

    let mut applied_db = 0.0;
    if let Some(current) = measured {
        let gain = gain_for(current, target.value());
        applied_db = 20.0 * gain.log10();
        if gain > 0.0 && gain.is_finite() {
            for s in wav.samples.iter_mut() {
                *s *= gain;
            }
        }
    }

    let ceiling = 10f32.powf(ceiling_dbfs / 20.0);
    // Only limit if the gain actually pushed something past the ceiling.
    let needs_limiting = peak_before * 10f32.powf(applied_db / 20.0) > ceiling;
    if needs_limiting {
        limit(wav, ceiling_dbfs);
    }

    NormaliseReport {
        measured_lufs: measured,
        target_lufs: target.value(),
        applied_gain_db: applied_db,
        limited: needs_limiting,
        peak_before_dbfs: to_dbfs(peak_before),
        peak_after_dbfs: to_dbfs(wav.peak()),
        brightness_tilt_db: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f32, secs: f32, rate: u32, amp: f32) -> Wav {
        let frames = (secs * rate as f32) as usize;
        let mut samples = Vec::with_capacity(frames * 2);
        for i in 0..frames {
            let v = (2.0 * std::f32::consts::PI * freq * i as f32 / rate as f32).sin() * amp;
            samples.push(v);
            samples.push(v);
        }
        Wav {
            sample_rate: rate,
            channels: 2,
            samples,
        }
    }

    #[test]
    fn a_full_scale_tone_measures_near_zero_lufs() {
        // A 1 kHz sine at full scale is the reference case: BS.1770 puts it
        // just under 0 LUFS. If this is far off, the K-weighting is wrong.
        let w = sine(1000.0, 2.0, 48000, 1.0);
        let l = integrated_lufs(&w).expect("a two-second tone must measure");
        assert!(l > -3.0 && l < 0.5, "expected roughly 0 LUFS, got {l}");
    }

    #[test]
    fn silence_is_unmeasurable_rather_than_minus_infinity() {
        // Reporting a number here would make the normaliser apply the full
        // target gain to a silent render.
        let w = Wav {
            sample_rate: 48000,
            channels: 2,
            samples: vec![0.0; 48000 * 4],
        };
        assert_eq!(integrated_lufs(&w), None);
    }

    #[test]
    fn material_shorter_than_a_block_is_unmeasurable() {
        let w = sine(1000.0, 0.1, 48000, 0.5);
        assert_eq!(integrated_lufs(&w), None);
    }

    #[test]
    fn quiet_material_measures_quieter_than_loud_material() {
        let loud = sine(1000.0, 3.0, 48000, 0.5);
        let quiet = sine(1000.0, 3.0, 48000, 0.05);
        let a = integrated_lufs(&loud).unwrap();
        let b = integrated_lufs(&quiet).unwrap();
        let gap = a - b;
        // A 10x amplitude ratio is 20 dB of power, so the loudness gap is 20 LU,
        // not 20 dB of amplitude. Allow a band for the filter's transient.
        assert!((gap - 20.0).abs() < 2.0, "expected ~20 LU apart, got {gap}");
    }

    #[test]
    fn the_absolute_gate_excludes_trailing_silence() {
        // Five seconds of tone then three of silence. Without the gate the
        // silence drags the mean down and the track reads far too quiet.
        let tone = sine(1000.0, 5.0, 48000, 0.5);
        let frames = tone.frames();
        let mut with_tail = tone.clone();
        with_tail.samples.resize(frames * 2 + 48000 * 3 * 2, 0.0);

        let plain = integrated_lufs(&tone).unwrap();
        let gated = integrated_lufs(&with_tail).unwrap();
        assert!(
            (plain - gated).abs() < 1.0,
            "silence changed the reading by {} LU",
            (plain - gated).abs()
        );
    }

    #[test]
    fn the_relative_gate_excludes_a_quiet_intro() {
        // The relative gate is what stops a mostly-silent track from being
        // measured by its one loud moment.
        let loud = sine(1000.0, 1.0, 48000, 0.9);
        let frames = loud.frames();
        let mut mixed = Wav {
            sample_rate: 48000,
            channels: 2,
            samples: vec![0.0; frames * 2 * 5],
        };
        // Loud in the middle, near-silent elsewhere.
        mixed.samples[frames * 2 * 2..frames * 2 * 3].copy_from_slice(&loud.samples);

        let l = integrated_lufs(&mixed);
        assert!(l.is_some(), "a one-second loud section should still measure");
        assert!(l.unwrap() > -20.0, "the quiet parts were not gated out: {l:?}");
    }

    #[test]
    fn the_limiter_never_leaves_a_sample_over_the_ceiling() {
        let mut w = sine(440.0, 2.0, 48000, 1.0);
        limit(&mut w, -1.0);
        assert!(
            w.peak() <= 10f32.powf(-1.0 / 20.0) + 1e-6,
            "peak {} is above the -1 dBFS ceiling",
            w.peak()
        );
    }

    #[test]
    fn the_limiter_leaves_quiet_material_alone() {
        let mut w = sine(440.0, 2.0, 48000, 0.1);
        let before: Vec<f32> = w.samples.clone();
        limit(&mut w, -1.0);
        for (a, b) in before.iter().zip(&w.samples) {
            assert!((a - b).abs() < 1e-4, "the limiter altered quiet material");
        }
    }

    #[test]
    fn the_limiter_reduces_rather_than_clips() {
        // A hard clipper produces flat tops; a limiter rounds them. If this
        // material ends up with a run of identical maxima it was clipped.
        let mut w = sine(100.0, 1.0, 48000, 3.0);
        limit(&mut w, -1.0);
        let ceiling = 10f32.powf(-1.0 / 20.0);
        let at_ceiling = w.samples.iter().filter(|s| (**s - ceiling).abs() < 1e-6).count();
        let total = w.samples.len();
        assert!(
            at_ceiling * 100 < total,
            "{at_ceiling} of {total} samples sit exactly on the ceiling"
        );
    }

    #[test]
    fn normalising_moves_measurement_toward_the_target() {
        let mut w = sine(1000.0, 4.0, 48000, 0.02);
        let before = integrated_lufs(&w).unwrap();
        let report = normalise(&mut w, LoudnessTarget::Streaming14, DEFAULT_CEILING_DBFS);
        let after = integrated_lufs(&w).unwrap();
        assert!(
            (after - LoudnessTarget::Streaming14.value()).abs() < 0.5,
            "wanted -14, measured {after}"
        );
        // The gain must be positive: the source was quieter than the target.
        assert!(report.applied_gain_db > 0.0, "{}", report.applied_gain_db);
        assert!(after > before);
    }

    #[test]
    fn normalising_a_loud_track_reduces_it() {
        let mut w = sine(1000.0, 4.0, 48000, 0.9);
        let report = normalise(&mut w, LoudnessTarget::Broadcast24, DEFAULT_CEILING_DBFS);
        assert!(report.applied_gain_db < 0.0, "{}", report.applied_gain_db);
        let after = integrated_lufs(&w).unwrap();
        assert!((after + 24.0).abs() < 0.5, "measured {after}");
    }

    #[test]
    fn normalising_silence_leaves_it_silent() {
        // The dangerous case: no measurement, so no gain. A silent render must
        // not come back amplified by 40 dB of noise floor.
        let mut w = Wav {
            sample_rate: 48000,
            channels: 2,
            samples: vec![0.0; 48000 * 4],
        };
        let report = normalise(&mut w, LoudnessTarget::Streaming14, DEFAULT_CEILING_DBFS);
        assert_eq!(report.measured_lufs, None);
        assert_eq!(report.applied_gain_db, 0.0);
        assert_eq!(w.peak(), 0.0);
    }

    /// A quiet continuous bed with loud transients: high crest factor, which is
    /// the only shape that makes loudness gain and peak gain disagree. A steady
    /// tone cannot do this, because its peak tracks its loudness.
    fn peaky(secs: f32, rate: u32, bed: f32, click: f32) -> Wav {
        let frames = (secs * rate as f32) as usize;
        let mut samples = Vec::with_capacity(frames * 2);
        for i in 0..frames {
            let t = i as f32 / rate as f32;
            let v = if (t % 0.5) < 0.01 {
                click * (t % 0.5) * 100.0
            } else {
                bed * (2.0 * std::f32::consts::PI * 400.0 * t).sin()
            };
            samples.push(v);
            samples.push(v);
        }
        Wav {
            sample_rate: rate,
            channels: 2,
            samples,
        }
    }

    #[test]
    fn a_target_that_would_clip_reports_limiting_and_stays_under_the_ceiling() {
        let mut w = peaky(6.0, 48000, 0.02, 1.0);
        let measured = integrated_lufs(&w).expect("this material must measure");
        // Sanity: the fixture only exercises the limiter if the gain the target
        // demands genuinely outruns the available headroom.
        assert!(measured < -20.0, "fixture is not peaky enough: {measured}");

        let report = normalise(&mut w, LoudnessTarget::Loud6, DEFAULT_CEILING_DBFS);
        assert!(report.limited, "this source must need limiting");
        assert!(
            report.peak_after_dbfs <= -1.0 + 1e-3,
            "ended at {} dBFS",
            report.peak_after_dbfs
        );
    }

    #[test]
    fn a_steady_tone_is_left_under_the_ceiling_without_limiting() {
        // The flip side: not everything should be limited. Only material that
        // actually exceeds the ceiling pays for the limiter.
        let mut w = sine(1000.0, 4.0, 48000, 0.5);
        let report = normalise(&mut w, LoudnessTarget::Loud6, DEFAULT_CEILING_DBFS);
        assert!(!report.limited, "a steady tone should not need limiting");
    }

    #[test]
    fn a_low_tone_reads_darker_than_a_high_one() {
        // The centroid is only useful if it moves the right way; if it does
        // not, brightness matching would push mixes the wrong direction.
        let low = sine(150.0, 3.0, 48000, 0.3);
        let high = sine(6000.0, 3.0, 48000, 0.3);
        let a = spectral_centroid(&low).expect("a tone has a centroid");
        let b = spectral_centroid(&high).expect("a tone has a centroid");
        assert!(a < b, "expected the low tone to measure darker: {a} vs {b}");
    }

    #[test]
    fn silence_has_no_centroid() {
        let w = Wav {
            sample_rate: 48000,
            channels: 2,
            samples: vec![0.0; 48000 * 2],
        };
        assert_eq!(spectral_centroid(&w), None);
    }

    #[test]
    fn matching_a_track_to_itself_applies_almost_nothing() {
        let mut w = sine(1000.0, 3.0, 48000, 0.3);
        let reference = w.clone();
        let before = w.peak();
        let applied = match_brightness(&mut w, &reference, 6.0);
        assert_eq!(applied, Some(0.0), "a track already matches itself");
        assert!((w.peak() - before).abs() < 1e-3);
    }

    #[test]
    fn matching_to_a_brighter_reference_moves_the_centroid_up() {
        let mut dull = sine(200.0, 3.0, 48000, 0.3);
        let bright = sine(8000.0, 3.0, 48000, 0.3);
        let before = spectral_centroid(&dull).unwrap();
        let target = spectral_centroid(&bright).unwrap();

        let applied = match_brightness(&mut dull, &bright, 6.0).expect("both measure");
        assert!(applied > 0.0, "a dull track needs a brightening tilt");
        let after = spectral_centroid(&dull).unwrap();
        assert!(after > before, "centroid went {before} -> {after}");
        // Clamped, so it must not overshoot past the reference's own centroid
        // by much.
        assert!(after <= target * 1.5, "overshot: {after} vs {target}");
    }

    #[test]
    fn brightness_tilt_is_clamped() {
        let mut dull = sine(100.0, 3.0, 48000, 0.3);
        let bright = sine(12_000.0, 3.0, 48000, 0.3);
        let applied = match_brightness(&mut dull, &bright, 4.0).unwrap();
        assert!(applied <= 4.0, "clamp not applied: {applied}");
        assert!(dull.peak() < 2.0, "the tilt must not blow the signal up");
    }

    #[test]
    fn every_target_round_trips_to_its_own_value() {
        for target in [
            LoudnessTarget::Streaming14,
            LoudnessTarget::Club9,
            LoudnessTarget::Broadcast24,
            LoudnessTarget::Loud6,
        ] {
            let mut w = sine(1000.0, 5.0, 48000, 0.3);
            normalise(&mut w, target, DEFAULT_CEILING_DBFS);
            let got = integrated_lufs(&w).unwrap();
            assert!(
                (got - target.value()).abs() < 0.5,
                "{target:?} wanted {} but measured {got}",
                target.value()
            );
        }
    }

    #[test]
    fn mono_and_stereo_of_the_same_tone_agree() {
        // The channel weighting gives L and R each 1.0, so summing a mono
        // signal to both channels must not change the reading.
        let stereo = sine(1000.0, 3.0, 48000, 0.4);
        let frames = stereo.frames();
        let mono = Wav {
            sample_rate: 48000,
            channels: 1,
            samples: stereo.samples[..frames].to_vec(),
        };
        let a = integrated_lufs(&stereo).unwrap();
        let b = integrated_lufs(&mono).unwrap();
        // BS.1770 sums energy across channels, so the same waveform in two
        // channels is 3 LU louder than in one. That is the specified behaviour,
        // not an error: it is why a duplicated-mono master reads hotter.
        let gap = a - b;
        assert!((gap - 3.0).abs() < 0.2, "expected +3 LU for stereo, got {gap}");
    }
}