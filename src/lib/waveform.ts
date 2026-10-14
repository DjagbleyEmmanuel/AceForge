/**
 * Waveform peak extraction.
 *
 * Decodes with the Web Audio API from bytes already in memory, so it needs no
 * backend round trip and works offline. Peaks are reduced to fixed-width
 * buckets: a 3 minute track has far more samples than a 900px canvas has
 * pixels, and drawing every sample is both slow and visually identical.
 */

export interface Peaks {
  /** Normalised 0..1 per bucket, always an even number so pairs share a column. */
  data: Float32Array;
  duration: number;
}

/** Bucket count. Pairs share a pixel column: one mirrored above the centre
 *  line, one below, which reads as a waveform rather than a filled block. */
const BUCKETS = 1200;

let context: AudioContext | null = null;

function audioContext(): AudioContext {
  if (!context) {
    const Ctor =
      window.AudioContext ??
      (window as unknown as { webkitAudioContext: typeof AudioContext })
        .webkitAudioContext;
    context = new Ctor();
  }
  return context;
}

/** Reduce the decoded buffer to normalised peaks. */
export async function computePeaks(bytes: ArrayBuffer): Promise<Peaks> {
  const ctx = audioContext();
  // decodeAudioData detaches the buffer, so hand it a copy: the caller may
  // still want the original bytes for playback.
  const decoded = await ctx.decodeAudioData(bytes.slice(0));
  const channels: Float32Array[] = [];
  for (let c = 0; c < decoded.numberOfChannels; c++) {
    channels.push(decoded.getChannelData(c));
  }
  if (channels.length === 0 || decoded.length === 0) {
    return { data: new Float32Array(0), duration: 0 };
  }

  const samplesPerBucket = Math.max(1, Math.floor(decoded.length / BUCKETS));
  const buckets = Math.max(1, Math.floor(decoded.length / samplesPerBucket));
  const data = new Float32Array(buckets);

  for (let b = 0; b < buckets; b++) {
    const start = b * samplesPerBucket;
    const end = Math.min(decoded.length, start + samplesPerBucket);
    let peak = 0;
    // Mixing to mono keeps the envelope stable when channels drift apart.
    for (let i = start; i < end; i++) {
      let sum = 0;
      for (let c = 0; c < channels.length; c++) sum += channels[c][i];
      const v = Math.abs(sum / channels.length);
      if (v > peak) peak = v;
    }
    data[b] = peak;
  }

  // Normalise against the loudest bucket so quiet renders are still legible,
  // but never amplify pure silence into a fake waveform.
  let max = 0;
  for (const v of data) if (v > max) max = v;
  if (max > 0.001) {
    for (let i = 0; i < data.length; i++) data[i] = Math.min(1, data[i] / max);
  }

  return { data, duration: decoded.duration };
}

export interface DrawOptions {
  peaks: Peaks;
  /** 0..1 playback position. Drives the playhead. */
  progress: number;
  width: number;
  height: number;
  /** Played region colour. */
  played: string;
  unplayed: string;
  centre: string;
  /** Rainbow mode: tint each bar by its position across the track. */
  rainbow: boolean;
}

/** Hue ramp for the rainbow waveform. Saturated but not neon — the panel is
 *  dark, so pure #f0f lands far too bright against it. */
function spectrum(t: number, lightness: number, alpha: number): string {
  const hue = 280 - t * 300; // violet through to teal
  return `hsl(${hue.toFixed(0)} 85% ${lightness}% / ${alpha})`;
}

export function drawWaveform(
  canvas: HTMLCanvasElement,
  { peaks, progress, width, height, played, unplayed, centre, rainbow }: DrawOptions,
): void {
  const dpr = window.devicePixelRatio || 1;
  // Resize the backing store to the element's real pixel size; without this
  // the canvas is blurry on any HiDPI display.
  const cssWidth = width || canvas.clientWidth || 300;
  const cssHeight = height || canvas.clientHeight || 48;

  if (canvas.width !== Math.round(cssWidth * dpr)) {
    canvas.width = Math.round(cssWidth * dpr);
    canvas.height = Math.round(cssHeight * dpr);
  }

  const ctx = canvas.getContext("2d");
  if (!ctx) return;

  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.clearRect(0, 0, cssWidth, cssHeight);

  const mid = cssHeight / 2;

  if (peaks.data.length === 0) {
    // No decode yet: a flat centre line reads as "idle", not "broken".
    ctx.strokeStyle = centre;
    ctx.lineWidth = 1;
    ctx.beginPath();
    ctx.moveTo(0, mid);
    ctx.lineTo(cssWidth, mid);
    ctx.stroke();
    return;
  }

  const playedUntil = progress * cssWidth;
  const barWidth = Math.max(1, cssWidth / peaks.data.length);
  const half = mid - 2;

  for (let i = 0; i < peaks.data.length; i++) {
    const ratio = i / peaks.data.length;
    const x = ratio * cssWidth;
    const amplitude = Math.max(0.5, peaks.data[i] * half);
    const isPlayed = x <= playedUntil;

    if (rainbow) {
      // Brighter where played, muted ahead of the playhead: position still
      // reads as "done" and "to go" without a hard colour break.
      ctx.fillStyle = isPlayed
        ? spectrum(ratio, 68, 1)
        : spectrum(ratio, 34, 0.85);
    } else {
      ctx.fillStyle = isPlayed ? played : unplayed;
    }

    // Mirror the pair above and below the centre line.
    ctx.fillRect(x, mid - amplitude, Math.max(1, barWidth - 0.5), amplitude * 2);
  }

  // Playhead: a bright rule with a soft glow, so the exact position stays
  // readable against a busy spectrum.
  if (progress > 0) {
    const x = Math.min(cssWidth - 1, Math.max(0, playedUntil));
    ctx.save();
    ctx.shadowColor = rainbow ? "hsl(0 90% 70% / 0.9)" : played;
    ctx.shadowBlur = 8;
    ctx.fillStyle = rainbow ? "#ffffff" : played;
    ctx.fillRect(x - 0.75, 0, 1.5, cssHeight);
    ctx.restore();

    // Small cap so the head is visible even where the waveform is silent.
    ctx.fillStyle = rainbow ? "hsl(0 90% 72%)" : played;
    ctx.beginPath();
    ctx.arc(x, 3, 2.5, 0, Math.PI * 2);
    ctx.fill();
  }
}

/** Map a click anywhere on the waveform back to a playback position. */
export function seekFromEvent(
  event: React.MouseEvent<HTMLCanvasElement>,
  duration: number,
): number {
  const rect = event.currentTarget.getBoundingClientRect();
  if (rect.width === 0) return 0;
  const ratio = (event.clientX - rect.left) / rect.width;
  return Math.max(0, Math.min(1, ratio)) * duration;
}