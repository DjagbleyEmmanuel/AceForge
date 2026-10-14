import { useCallback, useEffect, useRef, useState } from "react";
import { loadTrackSource } from "../lib/audio";
import { trimming, type Track } from "../lib/api";
import { computePeaks, drawWaveform, type Peaks } from "../lib/waveform";

interface Props {
  track: Track;
  onClose: () => void;
  onSaved: () => void;
}

type Mode = "keep" | "delete";

function clock(secs: number): string {
  const s = Math.max(0, secs);
  const m = Math.floor(s / 60);
  const r = Math.floor(s % 60);
  return `${m}:${r.toString().padStart(2, "0")}`;
}

/**
 * Select a stretch of a finished song and keep or remove it.
 *
 * The selection is made by dragging across the waveform, which is the only
 * accurate way to pick a region -- typing times is for adjusting a drag that
 * was nearly right, not for finding one in the first place. With a range
 * highlighted there are exactly two things you can mean by it, so both are one
 * click rather than a mode to switch into first.
 */
export function CropDialog({ track, onClose, onSaved }: Props) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const audioRef = useRef<HTMLAudioElement>(null);
  const [peaks, setPeaks] = useState<Peaks | null>(null);
  const [srcUrl, setSrcUrl] = useState<string | null>(null);
  const [duration, setDuration] = useState(track.durationSecs || 0);
  /** The highlighted range. Null until the first drag. */
  const [sel, setSel] = useState<{ from: number; to: number } | null>(null);
  const [dragFrom, setDragFrom] = useState<number | null>(null);
  const [phase, setPhase] = useState<"select" | "confirm">("select");
  const [mode, setMode] = useState<Mode>("keep");
  const [replace, setReplace] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    let url: string | null = null;
    void (async () => {
      try {
        const loaded = await loadTrackSource(track);
        if (!alive) {
          URL.revokeObjectURL(loaded.url);
          return;
        }
        url = loaded.url;
        setSrcUrl(loaded.url);
        const bytes = await (await fetch(loaded.url)).arrayBuffer();
        const p = await computePeaks(bytes);
        if (!alive) return;
        setPeaks(p);
        if (p.duration > 0) setDuration(p.duration);
      } catch (e) {
        if (alive) setError(String(e));
      }
    })();
    return () => {
      alive = false;
      if (url) URL.revokeObjectURL(url);
    };
  }, [track]);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas || !peaks) return;
    const render = () => {
      drawWaveform(canvas, {
        peaks,
        progress: 0,
        width: canvas.clientWidth,
        height: 96,
        played: "var(--color-ink-700)",
        unplayed: "var(--color-ink-750)",
        centre: "var(--color-ink-800)",
        rainbow: false,
      });
    };
    render();
    const observer = new ResizeObserver(render);
    observer.observe(canvas);
    return () => observer.disconnect();
  }, [peaks]);

  const secondsAt = useCallback(
    (clientX: number) => {
      const rect = canvasRef.current?.getBoundingClientRect();
      if (!rect || rect.width === 0 || duration === 0) return 0;
      return Math.max(0, Math.min(1, (clientX - rect.left) / rect.width)) * duration;
    },
    [duration],
  );

  const snap = (s: number) => Math.round(s * 10) / 10;
  /** One keyboard step. A tenth of a second is too coarse for a clean cut. */
  const NUDGE = 0.05;

  function onPointerDown(e: React.PointerEvent<HTMLCanvasElement>) {
    if (!peaks) return;
    setError(null);
    setPhase("select");
    const at = snap(secondsAt(e.clientX));
    setDragFrom(at);
    setSel({ from: at, to: at });
    (e.target as HTMLElement).setPointerCapture(e.pointerId);
  }

  function onPointerMove(e: React.PointerEvent<HTMLCanvasElement>) {
    if (dragFrom === null || !peaks) return;
    const at = snap(secondsAt(e.clientX));
    setSel({
      from: Math.max(0, Math.min(dragFrom, at)),
      to: Math.min(duration, Math.max(dragFrom, at)),
    });
  }

  function onPointerUp(e: React.PointerEvent<HTMLCanvasElement>) {
    if (dragFrom === null) return;
    setDragFrom(null);
    (e.target as HTMLElement).releasePointerCapture?.(e.pointerId);
  }

  /**
   * Keyboard selection.
   *
   * Arrow keys alone are not enough: dragging snaps to 0.1s, so a cut can never
   * land exactly where a transient starts. Shift gives frame-level control --
   * arrows move the whole selection, up and down resize its end.
   */
  function onKeyDown(e: React.KeyboardEvent) {
    if (!e.shiftKey) return;
    const k = e.key;
    if (k !== "ArrowLeft" && k !== "ArrowRight" && k !== "ArrowUp" && k !== "ArrowDown") {
      return;
    }
    e.preventDefault();
    const dir = k === "ArrowRight" || k === "ArrowUp" ? 1 : -1;
    const step = NUDGE * (e.altKey ? 5 : 1);

    setSel((prev) => {
      const base = prev ?? { from: 0, to: Math.min(duration, 1) };
      if (k === "ArrowLeft" || k === "ArrowRight") {
        const width = base.to - base.from;
        let from = base.from + dir * step;
        if (from < 0) from = 0;
        if (from + width > duration) from = duration - width;
        if (width > duration) from = 0;
        return { from: Math.max(0, from), to: Math.max(0, from) + width };
      }
      // Up grows the selection, down shrinks it, never past zero width.
      const to = Math.max(base.from + step, Math.min(duration, base.to + dir * step));
      return { from: base.from, to };
    });
  }

  /** Audition the highlighted range, looping inside it. */
  function preview() {
    const a = audioRef.current;
    if (!a || !srcUrl || !sel) return;
    if (!a.paused) {
      a.pause();
      return;
    }
    if (a.currentTime < sel.from || a.currentTime >= sel.to) a.currentTime = sel.from;
    void a.play().catch(() => undefined);
  }

  useEffect(() => {
    const a = audioRef.current;
    if (!a || !srcUrl || !sel) return;
    const stop = () => {
      if (sel.to > sel.from && a.currentTime >= sel.to) a.pause();
    };
    a.addEventListener("timeupdate", stop);
    return () => a.removeEventListener("timeupdate", stop);
  }, [srcUrl, sel]);

  const span = sel ? sel.to - sel.from : 0;
  const usable = !!sel && span >= 0.2;
  // Removing a range that is the whole track would leave nothing; keeping one
  // that is the whole track would change nothing.
  const removesEverything =
    !!sel && sel.from <= 0.05 && sel.to >= duration - 0.05;

  const keptLength = mode === "keep" ? span : Math.max(0, duration - span);

  async function commit() {
    if (!sel) return;
    setBusy(true);
    setError(null);
    try {
      await trimming.crop(
        track.path,
        sel.from,
        sel.to,
        replace,
        mode === "keep" ? "only" : "except",
      );
      onSaved();
      setResult(
        mode === "keep"
          ? replace
            ? `Replaced the file — now ${clock(span)}.`
            : `Saved a clip — ${clock(span)}. The original is untouched.`
          : replace
            ? `Replaced the file — ${clock(span)} removed.`
            : `Saved a cut version — ${clock(span)} removed. The original is untouched.`,
      );
    } catch (e) {
      setError(String(e));
      setPhase("select");
    } finally {
      setBusy(false);
    }
  }

  const leftPct = sel && duration > 0 ? (sel.from / duration) * 100 : 0;
  const widthPct = sel && duration > 0 ? (span / duration) * 100 : 0;

  return (
    <div
      className="fixed inset-0 z-[60] flex items-center justify-center bg-black/70 p-4 backdrop-blur-sm"
      onPointerDown={(e) => {
        if (e.target === e.currentTarget && !busy) onClose();
      }}
    >
      <div className="panel w-full max-w-2xl space-y-4 p-5">
        <div className="flex items-start justify-between gap-3">
          <div className="min-w-0">
            <h3 className="text-sm font-semibold">Crop song</h3>
            <p className="mt-0.5 truncate text-xs text-ink-400">
              {track.caption || track.id}
            </p>
          </div>
          <button
            onClick={onClose}
            disabled={busy}
            aria-label="Close"
            className="shrink-0 text-ink-500 hover:text-ink-200 disabled:opacity-40"
          >
            ×
          </button>
        </div>

        {result ? (
          <div className="space-y-4">
            <div className="rounded-lg bg-mint-400/10 px-3 py-2.5 text-xs text-mint-300 ring-1 ring-mint-400/25">
              {result}
            </div>
            <button onClick={onClose} className="btn btn-primary w-full">
              Done
            </button>
          </div>
        ) : (
          <>
            <div className="relative">
              <canvas
                ref={canvasRef}
                onPointerDown={onPointerDown}
                onPointerMove={onPointerMove}
                onPointerUp={onPointerUp}
                className="h-24 w-full cursor-ew-resize touch-none rounded-lg"
                aria-label="Drag across the waveform to select a section"
                tabIndex={0}
                onKeyDown={onKeyDown}
              />
              {peaks && sel && (
                <div
                  className={`pointer-events-none absolute inset-y-0 rounded-md border ${
                    mode === "keep"
                      ? "border-glow-400 bg-glow-500/25"
                      : "border-rose-400 bg-rose-500/25"
                  }`}
                  style={{ left: `${leftPct}%`, width: `${widthPct}%` }}
                />
              )}
            </div>

            <div className="flex flex-wrap items-center gap-2 text-xs">
              {sel && usable ? (
                <>
                  <span className="font-mono text-ink-200">
                    {clock(sel.from)} → {clock(sel.to)}
                  </span>
                  <span className="text-ink-500">({clock(span)})</span>
                  <button onClick={preview} className="chip px-2 py-1 text-[11px]">
                    play selection
                  </button>
                  <button
                    onClick={() => setSel(null)}
                    className="chip px-2 py-1 text-[11px]"
                  >
                    clear
                  </button>
                  <span className="text-[10px] text-ink-500">
                    <kbd className="rounded bg-ink-800 px-1">shift</kbd> +{" "}
                    <kbd className="rounded bg-ink-800 px-1">←</kbd>
                    <kbd className="rounded bg-ink-800 px-1">→</kbd> moves,
                    <kbd className="rounded bg-ink-800 px-1">↑</kbd>
                    <kbd className="rounded bg-ink-800 px-1">↓</kbd> resizes
                  </span>
                </>
              ) : (
                <span className="text-ink-500">
                  {sel
                    ? "Drag a longer stretch on the waveform."
                    : "Drag across the waveform to select a section."}
                </span>
              )}
            </div>

            {error && <p className="text-xs text-rose-300">{error}</p>}

            {usable && (
              <div className="space-y-3 border-t border-ink-800 pt-3">
                <div className="flex flex-wrap gap-2">
                  <button
                    onClick={() => {
                      setMode("keep");
                      setPhase("confirm");
                      setReplace(false);
                    }}
                    className="btn btn-primary px-3 py-1.5 text-xs"
                  >
                    keep only this ({clock(span)})
                  </button>
                  <button
                    onClick={() => {
                      setMode("delete");
                      setPhase("confirm");
                      setReplace(false);
                    }}
                    disabled={removesEverything}
                    title={
                      removesEverything
                        ? "That is the whole track"
                        : "Remove this section and keep the rest"
                    }
                    className="btn btn-ghost px-3 py-1.5 text-xs disabled:opacity-40"
                  >
                    delete this section
                  </button>
                </div>

                {phase === "confirm" && (
                  <div className="space-y-2 rounded-lg bg-ink-900/60 p-3">
                    <p className="text-[11px] leading-relaxed text-ink-300">
                      {mode === "keep" ? (
                        <>
                          Keep only{" "}
                          <span className="font-mono text-ink-100">
                            {clock(sel!.from)}–{clock(sel!.to)}
                          </span>{" "}
                          ({clock(keptLength)}) — everything outside that range is
                          left out.
                        </>
                      ) : (
                        <>
                          Remove{" "}
                          <span className="font-mono text-ink-100">
                            {clock(sel!.from)}–{clock(sel!.to)}
                          </span>{" "}
                          and join the rest into one{" "}
                          <span className="font-mono text-ink-100">
                            {clock(keptLength)}
                          </span>{" "}
                          file.
                        </>
                      )}
                    </p>

                    {!replace ? (
                      <div className="flex flex-wrap justify-end gap-2">
                        <button
                          onClick={() => setPhase("select")}
                          className="btn btn-ghost px-3 py-1 text-xs"
                        >
                          back
                        </button>
                        <button
                          onClick={() => void commit()}
                          disabled={busy}
                          className="btn btn-primary px-3 py-1 text-xs disabled:opacity-40"
                        >
                          {busy ? "working…" : "save as a new file"}
                        </button>
                        <button
                          onClick={() => setReplace(true)}
                          className="btn btn-ghost px-3 py-1 text-xs"
                        >
                          replace the original…
                        </button>
                      </div>
                    ) : (
                      <div className="space-y-2">
                        <p className="rounded bg-rose-500/10 px-2.5 py-2 text-[11px] leading-relaxed text-rose-200 ring-1 ring-rose-500/25">
                          <strong>This cannot be undone.</strong> The change
                          overwrites{" "}
                          <span className="font-mono">
                            {track.caption || track.id}
                          </span>{" "}
                          in place. What is removed is discarded immediately and
                          cannot be recovered — not by closing the app, and not by
                          reopening this dialog.
                        </p>
                        <div className="flex flex-wrap justify-end gap-2">
                          <button
                            onClick={() => setReplace(false)}
                            className="btn btn-ghost px-3 py-1 text-xs"
                          >
                            keep the original
                          </button>
                          <button
                            onClick={() => void commit()}
                            disabled={busy}
                            className="btn bg-rose-500 px-3 py-1 text-xs text-white hover:brightness-110 disabled:opacity-40"
                          >
                            {busy ? "working…" : "replace the original"}
                          </button>
                        </div>
                      </div>
                    )}
                  </div>
                )}
              </div>
            )}
          </>
        )}
      </div>

      <audio ref={audioRef} src={srcUrl ?? undefined} preload="none" className="hidden" />
    </div>
  );
}