/**
 * Output sample-rate switching for auditioning.
 *
 * A render clocked at 44.1 kHz played through a 48 kHz device is resampled on
 * the way out, which is audibly different from the same file through a 44.1 kHz
 * graph — that is the whole point of the toggle.
 *
 * The awkward part: an `HTMLMediaElement` can only ever be bound to one
 * `AudioContext`, and a `MediaElementAudioSourceNode` belongs to the context
 * that created it. So changing the rate means closing the old context, creating
 * a new one at the requested rate, and re-binding the element. Re-binding means
 * re-assigning `src`, which resets playback position — unavoidable with this
 * API, so the caller is told when it happened and can restore the position.
 */

/** Rates worth switching between. 48k is the ACE-Step output rate; 44.1k is the
 *  other common one and what most streaming services normalise to. */
export const OUTPUT_RATES = [48000, 44100] as const;
export type OutputRate = (typeof OUTPUT_RATES)[number];

type Ctor = new (options?: AudioContextOptions) => AudioContext;

/** WebKitGTK ships prefixed constructors in some builds. */
function audioContextCtor(): Ctor | null {
  const w = window as unknown as {
    AudioContext?: Ctor;
    webkitAudioContext?: Ctor;
  };
  return w.AudioContext ?? w.webkitAudioContext ?? null;
}

/** The device's own rate, so the default toggle position reflects reality
 *  rather than assuming 48 kHz. */
export function nativeOutputRate(): OutputRate {
  if (typeof window === "undefined") return 48000;
  try {
    const Ctor = audioContextCtor();
    if (!Ctor) return 48000;
    const probe = new Ctor();
    const rate = probe.sampleRate;
    void probe.close();
    return rate >= 46000 ? 48000 : 44100;
  } catch {
    return 48000;
  }
}

export interface RateBinding {
  context: AudioContext;
  source: MediaElementAudioSourceNode;
}

/**
 * Route `element` through an `AudioContext` running at `rate`.
 *
 * Returns the binding so the caller can tear it down. Throws if the browser
 * refuses; callers should treat that as "the toggle is unavailable" rather than
 * surfacing an error, because audio still plays fine without it.
 */
export function bindOutputRate(
  element: HTMLMediaElement,
  rate: OutputRate,
): RateBinding {
  const Ctor = audioContextCtor();
  if (!Ctor) {
    throw new Error("this webview has no AudioContext");
  }
  const context = new Ctor({ sampleRate: rate });
  const source = context.createMediaElementSource(element);
  source.connect(context.destination);
  return { context, source };
}

/** Tear a binding down. Disconnect before close, or the close races the node's
 *  own teardown and can throw in some WebKitGTK builds. */
export function releaseOutputRate(binding: RateBinding | null): void {
  if (!binding) return;
  try {
    binding.source.disconnect();
  } catch {
    // Already torn down with its context.
  }
  void binding.context.close().catch(() => {});
}

/**
 * Re-bind `element` at `rate`, preserving where it was playing.
 *
 * Re-assigning `src` resets the element, so the previous `currentTime` is
 * restored once metadata is back and playback is resumed only if it was already
 * running. Returns the new binding.
 */
export function rebindOutputRate(
  element: HTMLMediaElement,
  previous: RateBinding | null,
  rate: OutputRate,
): RateBinding {
  const wasPlaying = !element.paused && !element.ended;
  const at = element.currentTime;
  const src = element.src;

  releaseOutputRate(previous);

  if (src) {
    // Re-assigning the same src is what forces the element to re-bind.
    element.src = "";
    element.src = src;
    element.load();
    const restore = () => {
      element.currentTime = at;
      if (wasPlaying) void element.play().catch(() => {});
    };
    element.addEventListener("loadedmetadata", restore, { once: true });
  }

  return bindOutputRate(element, rate);
}