import { memo, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

/** Where a render has got to. Null until the first progress line appears,
 *  which is a real state rather than a failure: the planner writes nothing
 *  until it hands off to the sampler. */
interface Progress {
  fraction: number | null;
  stage: string;
  step: [number, number] | null;
  tiles_done: [number, number] | null;
}

interface Props {
  /** Byte offset of the log when the current job was submitted. Progress is
   *  parsed from what the engine logged since then, so an earlier render's
   *  completed steps cannot be mistaken for this one's. */
  mark: number | null;
  /** Unix seconds the render started, from the queue row. The clock ticks
   *  locally rather than through the store, so a second passing re-renders
   *  this card and nothing else. */
  startedAt: number | null;
  /** True while a job is actually running. */
  active: boolean;
}

const POLL_MS = 1000;

function clock(secs: number): string {
  const s = Math.max(0, Math.floor(secs));
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}:${String(s % 60).padStart(2, "0")}`;
  return `${Math.floor(m / 60)}:${String(m % 60).padStart(2, "0")}`;
}

/**
 * Live render progress, for the side panel.
 *
 * The engine reports no percentage of its own -- `/job` answers a bare
 * `{"status":"running"}` -- so the fraction is parsed from the step lines the
 * sampler writes to stderr. That is real data rather than a timer that guesses.
 *
 * Three rules keep this from costing anything, because a render is exactly when
 * the app must not stutter:
 *
 *  - One timer, one second. Not a `requestAnimationFrame` loop. A permanently
 *    running rAF is the usual way a progress bar costs more than the work it is
 *    reporting on.
 *  - The bar moves by CSS. The fill is a `transform: scaleX()` on a composited
 *    layer with a transition, so the animation is handled off the main thread.
 *    Animating `width` would relayout the panel every frame.
 *  - State changes at 1 Hz, and only the numbers in here are state. The bar's
 *    position is a style, so changing it does not re-render anything.
 */
export const ProgressPanel = memo(function ProgressPanel({
  mark,
  startedAt,
  active,
}: Props) {
  const [progress, setProgress] = useState<Progress | null>(null);
  const [now, setNow] = useState(() => Date.now() / 1000);
  const barRef = useRef<HTMLDivElement>(null);
  const labelRef = useRef<HTMLSpanElement>(null);

  // Poll only while there is something to poll for, and clear on the way out so
  // a finished render costs no IPC at all.
  useEffect(() => {
    if (!active || mark === null) {
      setProgress(null);
      return;
    }
    let alive = true;
    const read = async () => {
      try {
        const p = await invoke<Progress>("job_progress", { mark });
        if (alive) setProgress(p);
      } catch {
        // The engine may be mid-restart. Leave the last value rather than
        // blanking a bar that was showing something true a moment ago.
      }
    };
    void read();
    const id = window.setInterval(() => void read(), POLL_MS);
    return () => {
      alive = false;
      window.clearInterval(id);
    };
  }, [active, mark]);

  // One local tick, one second. Deliberately not in the store: the store's
  // value feeds the whole app, so ticking it here would re-render the nav, the
  // panels and the queue once a second for the sake of one clock.
  useEffect(() => {
    if (!active) return;
    const id = window.setInterval(
      () => setNow(Date.now() / 1000),
      POLL_MS,
    );
    return () => window.clearInterval(id);
  }, [active]);

  const fraction = progress?.fraction ?? null;

  // Write the bar position and its readout straight to the DOM. Keeping these
  // out of React means the bar animating does not re-render the panel, let
  // alone the app.
  useEffect(() => {
    const bar = barRef.current;
    const label = labelRef.current;
    if (!bar || !label) return;
    if (fraction === null) {
      bar.style.transform = "scaleX(0)";
      label.textContent = "";
      return;
    }
    const pct = Math.round(fraction * 100);
    bar.style.transform = `scaleX(${fraction})`;
    label.textContent = `${pct}%`;
  }, [fraction]);

  if (!active) return null;

  const elapsedSecs = startedAt ? Math.max(0, now - startedAt) : 0;
  // Only an ETA once there is enough signal to mean anything: extrapolating from
  // the first second of a render produces a confident, wrong number.
  const eta =
    fraction !== null && fraction > 0.08
      ? Math.max(0, elapsedSecs * (1 / fraction - 1))
      : null;

  const step = progress?.step;
  const detail =
    step && step[1] > 0
      ? `step ${step[0]}/${step[1]}`
      : progress?.stage === "decoding"
        ? "decoding audio"
        : "planning";

  return (
    <div className="panel overflow-hidden p-3">
      <div className="flex items-center justify-between">
        <span className="text-[11px] text-ink-400">Rendering</span>
        <span
          ref={labelRef}
          className="font-mono text-[11px] tabular-nums text-glow-300"
        />
      </div>

      {/* Track and fill are separate layers so the fill can be transformed
          without the track repainting under it. */}
      <div className="relative mt-2 h-1.5 overflow-hidden rounded-full bg-ink-850">
        <div
          ref={barRef}
          className="absolute inset-0 origin-left rounded-full bg-gradient-to-r from-mint-400 via-glow-400 to-glow-500 will-change-transform"
          style={{ transform: "scaleX(0)", transition: "transform 900ms cubic-bezier(0.22,1,0.36,1)" }}
        />
        {/* While the fraction is unknown the bar still needs to say something is
            happening. A CSS keyframe, not a JS loop. */}
        {fraction === null && (
          <div
            className="absolute inset-y-0 w-1/3 rounded-full bg-gradient-to-r from-transparent via-glow-400/80 to-transparent"
            style={{ animation: "progress-sweep 1.4s ease-in-out infinite" }}
          />
        )}
      </div>

      <div className="mt-2 flex items-baseline justify-between font-mono text-[10px] tabular-nums">
        <span className="truncate text-ink-500">{detail}</span>
        <span className="text-ink-400">{clock(elapsedSecs)}</span>
      </div>
      <div className="mt-0.5 flex items-baseline justify-between font-mono text-[10px] tabular-nums">
        <span className="text-ink-600">started {clock(elapsedSecs)} ago</span>
        {eta !== null && <span className="text-mint-400">~{clock(eta)} left</span>}
      </div>
    </div>
  );
});