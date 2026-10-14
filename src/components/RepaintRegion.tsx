import { useEffect, useRef, useState } from "react";
import { computePeaks, drawWaveform, type Peaks } from "../lib/waveform";
import { loadTrackSource } from "../lib/audio";
import type { Track } from "../lib/api";

interface Props {
  track: Track;
  /** Region in seconds. `end` null means "to the end of the track". */
  start: number;
  end: number | null;
  onChange: (start: number, end: number | null) => void;
  disabled?: boolean;
}

type Drag = { mode: "start" | "end"; anchor: number } | null;

/**
 * Pick the stretch of a finished song to regenerate.
 *
 * The region is chosen here rather than with two numeric boxes because the
 * only useful way to pick it is against the audio. Backend coordinates are
 * plain seconds, the same units `repainting_start`/`repainting_end` take, and
 * `-1` is the sentinel meaning "to the end" -- which is why `end` is nullable
 * here rather than just a number.
 */
export function RepaintRegion({
  track,
  start,
  end,
  onChange,
  disabled,
}: Props) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [peaks, setPeaks] = useState<Peaks | null>(null);
  const [srcUrl, setSrcUrl] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [drag, setDrag] = useState<Drag>(null);
  const audioRef = useRef<HTMLAudioElement>(null);

  const duration = peaks?.duration ?? 0;

  useEffect(() => {
    let alive = true;
    setPeaks(null);
    setError(null);
    // Guarded: a throw in an effect unmounts the whole subtree and the panel
    // goes blank, which is indistinguishable from a layout bug.
    try {
      void loadTrackSource(track)
        .then(async ({ url }) => {
          if (!alive) return;
          setSrcUrl(url);
          const res = await fetch(url);
          const bytes = await res.arrayBuffer();
          const p = await computePeaks(bytes);
          if (alive) setPeaks(p);
        })
        .catch((e) => {
          if (alive) setError(String(e));
        });
    } catch (e) {
      if (alive) setError(String(e));
    }
    return () => {
      alive = false;
    };
  }, [track]);

  // Stop playback when the track changes, or the old clip keeps playing under
  // the new waveform.
  useEffect(() => {
    const a = audioRef.current;
    if (a) {
      a.pause();
      a.currentTime = 0;
    }
  }, [srcUrl]);

  // Keep an audition inside the region. Playing the whole track to judge ten
  // seconds of it is the reason the region-scoped button exists.
  useEffect(() => {
    const a = audioRef.current;
    if (!a || !srcUrl) return;
    const onTime = () => {
      const limit = end ?? duration;
      if (limit > 0 && a.currentTime >= limit) a.pause();
    };
    a.addEventListener("timeupdate", onTime);
    return () => a.removeEventListener("timeupdate", onTime);
  }, [srcUrl, end, duration]);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas || !peaks) return;
    const render = () => {
      drawWaveform(canvas, {
        peaks,
        progress: 0,
        width: canvas.clientWidth,
        height: 80,
        played: "var(--color-mint-400)",
        unplayed: "var(--color-ink-700)",
        centre: "var(--color-ink-800)",
        // Flat, not the rainbow gradient: this is a selection surface, and a
        // moving gradient behind the region overlay makes the edge hard to see.
        rainbow: false,
      });
    };
    render();
    const observer = new ResizeObserver(render);
    observer.observe(canvas);
    window.addEventListener("resize", render);
    return () => {
      observer.disconnect();
      window.removeEventListener("resize", render);
    };
  }, [peaks]);

  function secondsAt(clientX: number): number {
    const rect = canvasRef.current?.getBoundingClientRect();
    if (!rect || rect.width === 0 || duration === 0) return 0;
    const ratio = Math.max(0, Math.min(1, (clientX - rect.left) / rect.width));
    return ratio * duration;
  }

  // Dragging snaps to 0.1s. Anything finer is invisible on the canvas and the
  // backend only needs a sane region.
  const snap = (s: number) => Math.round(s * 10) / 10;

  function onPointerDown(e: React.PointerEvent<HTMLCanvasElement>) {
    if (disabled || !peaks) return;
    const at = secondsAt(e.clientX);
    // Grab whichever handle is nearer; otherwise start a fresh region.
    const endAt = end ?? duration;
    const dStart = Math.abs(at - start);
    const dEnd = Math.abs(at - endAt);
    const mode = Math.min(dStart, dEnd) < Math.max(1, duration * 0.04)
      ? dStart <= dEnd
        ? "start"
        : "end"
      : "start";
    setDrag({ mode, anchor: mode === "start" ? endAt : start });
    (e.target as HTMLElement).setPointerCapture(e.pointerId);
  }

  function onPointerMove(e: React.PointerEvent<HTMLCanvasElement>) {
    if (!drag || !peaks) return;
    const at = Math.max(0, Math.min(duration, snap(secondsAt(e.clientX))));
    if (drag.mode === "start") {
      // Keep at least 0.2s of region; the backend rejects end <= start.
      const lo = end === null ? 0 : Math.min(start, Math.max(0, (end ?? duration) - 0.2));
      onChange(Math.min(at, lo), end);
    } else {
      onChange(start, Math.max(at, start + 0.2) >= duration - 0.05 ? null : Math.max(at, start + 0.2));
    }
  }

  function onPointerUp(e: React.PointerEvent<HTMLCanvasElement>) {
    if (drag) {
      setDrag(null);
      (e.target as HTMLElement).releasePointerCapture?.(e.pointerId);
    }
  }

  function preview() {
    const a = audioRef.current;
    if (!a || !srcUrl) return;
    if (a.paused) {
      // Always audition from the start of the region, not wherever the last
      // preview left off.
      if (a.currentTime < start || a.currentTime >= (end ?? duration)) {
        a.currentTime = start;
      }
      void a.play().catch(() => undefined);
    } else {
      a.pause();
    }
  }

  const endAt = end ?? duration;
  const leftPct = duration > 0 ? (start / duration) * 100 : 0;
  const widthPct = duration > 0 ? (Math.max(0, endAt - start) / duration) * 100 : 0;

  return (
    <div className="space-y-2">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <p className="text-[11px] text-ink-300">
          Repaint{" "}
          <span className="font-mono text-ink-100">
            {start.toFixed(1)}s → {end === null ? "end" : `${end.toFixed(1)}s`}
          </span>
        </p>
        <div className="flex gap-1.5">
          <button
            onClick={preview}
            disabled={!srcUrl}
            className="btn btn-ghost px-2 py-1 text-[10px] disabled:opacity-40"
            title="Audition just the selected region"
          >
            preview region
          </button>
          <button
            onClick={() => onChange(0, null)}
            disabled={disabled || (start === 0 && end === null)}
            className="btn btn-ghost px-2 py-1 text-[10px] disabled:opacity-40"
          >
            whole track
          </button>
        </div>
      </div>

      {/* Exact times. Dragging is imprecise by nature, and a region that is a
          tenth of a second out wastes a render that takes minutes. */}
      <div className="flex items-center gap-2 text-[11px]">
        <label className="flex items-center gap-1 text-ink-500">
          from
          <input
            type="number"
            min={0}
            step={0.1}
            value={Number(start.toFixed(1))}
            disabled={disabled}
            onChange={(e) => {
              const v = Number(e.target.value);
              if (!Number.isFinite(v)) return;
              onChange(Math.max(0, Math.min(v, endAt - 0.2)), end);
            }}
            className="field w-20 px-1.5 py-0.5 font-mono text-[11px]"
          />
        </label>
        <label className="flex items-center gap-1 text-ink-500">
          to
          <input
            type="number"
            min={0}
            step={0.1}
            value={end === null ? "" : Number(end.toFixed(1))}
            placeholder={duration > 0 ? duration.toFixed(1) : "end"}
            disabled={disabled}
            onChange={(e) => {
              const raw = e.target.value;
              if (raw.trim() === "") {
                onChange(start, null);
                return;
              }
              const v = Number(raw);
              if (!Number.isFinite(v)) return;
              onChange(start, v <= start + 0.2 ? start + 0.2 : v);
            }}
            className="field w-20 px-1.5 py-0.5 font-mono text-[11px]"
          />
        </label>
        <span className="text-ink-600">seconds; empty "to" means the end</span>
      </div>

      <div className="relative">
        <canvas
          ref={canvasRef}
          onPointerDown={onPointerDown}
          onPointerMove={onPointerMove}
          onPointerUp={onPointerUp}
          className={`h-20 w-full rounded-lg ${disabled ? "opacity-40" : "cursor-crosshair"}`}
        />
        {peaks && (
          <>
            <div
              className="pointer-events-none absolute inset-y-0 rounded-full bg-emerald-400/25 ring-1 ring-emerald-400/60"
              style={{ left: `${leftPct}%`, width: `${widthPct}%` }}
            />
            {/* Visible grab handles. The region overlay alone does not say
                where its edges are, and the pointer already snaps to the
                nearer one -- this makes that affordance visible. */}
            {([
              [leftPct, "start"],
              [leftPct + widthPct, "end"],
            ] as const).map(([pct, which]) => (
              <span
                key={which}
                className="pointer-events-none absolute inset-y-0 w-1 -translate-x-1/2 rounded-full bg-emerald-300/80"
                style={{ left: `${pct}%` }}
              />
            ))}
          </>
        )}
      </div>

      <audio ref={audioRef} src={srcUrl ?? undefined} preload="none" className="hidden" />

      <p className="text-[10px] leading-relaxed text-ink-500">
        Drag on the waveform to choose the stretch to regenerate. Everything
        outside it is preserved.
        {error && <span className="block text-rose-300">{error}</span>}
      </p>
    </div>
  );
}
