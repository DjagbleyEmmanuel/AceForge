import { useEffect, useRef, useState } from "react";
import { library, type Track } from "../lib/api";
import { loadTrackSource } from "../lib/audio";
import { claimPlayback, releasePlayback } from "../lib/playerRegistry";
import {
  computePeaks,
  drawWaveform,
  seekFromEvent,
  type Peaks,
} from "../lib/waveform";
import { formatBytes } from "../lib/types";

interface Props {
  track: Track;
  compact?: boolean;
}

/** Custom transport over a hidden native <audio>.
 *
 *  The native control cannot be themed, so it is kept out of sight and the
 *  button, scrubber and clock are drawn here instead. The element itself is
 *  the source of truth for time, so scrubbing stays in sync by construction.
 */
export function TrackPlayer({ track, compact = false }: Props) {
  const audioRef = useRef<HTMLAudioElement>(null);
  const cardRef = useRef<HTMLDivElement>(null);
  // Panels stay mounted now, so every card would eagerly pull its whole file
  // into a Blob. Gate on visibility: a large WAV is tens of MB.
  const [visible, setVisible] = useState(false);
  const [playing, setPlaying] = useState(false);
  const [current, setCurrent] = useState(0);
  const [total, setTotal] = useState(track.durationSecs || 0);
  const [failed, setFailed] = useState<string | null>(null);
  const [source, setSource] = useState<string | null>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const wrapRef = useRef<HTMLDivElement>(null);
  const [peaks, setPeaks] = useState<Peaks | null>(null);
  const [rainbow, setRainbow] = useState(true);

  // Blob URLs are replaced per track and must be revoked, or a long session
  // leaks every track it has ever played.
  // Start loading a little before the card scrolls into view so the scrubber
  // is populated by the time it is on screen.
  useEffect(() => {
    const node = cardRef.current;
    if (!node || typeof IntersectionObserver === "undefined") {
      setVisible(true);
      return;
    }
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries.some((e) => e.isIntersecting)) {
          setVisible(true);
          observer.disconnect();
        }
      },
      { rootMargin: "200px" },
    );
    observer.observe(node);
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    if (!visible) return;
    let cancelled = false;
    let objectUrl: string | null = null;

    setPlaying(false);
    setCurrent(0);
    setFailed(null);
    setSource(null);
    setPeaks(null);
    // Seed from the stored duration so the clock is populated immediately,
    // before the element reports its own metadata.
    setTotal(track.durationSecs || 0);
    releasePlayback(track.path);

    void (async () => {
      try {
        const loaded = await loadTrackSource(track);
        if (cancelled) {
          URL.revokeObjectURL(loaded.url);
          return;
        }
        objectUrl = loaded.url;
        setSource(loaded.url);
        const el = audioRef.current;
        if (el) {
          el.src = loaded.url;
          el.load();
        }

        // Waveform comes from the same bytes. Failures are non-fatal: the
        // scrubber falls back to a plain bar.
        void (async () => {
          try {
            const response = await fetch(loaded.url);
            if (cancelled) return;
            const computed = await computePeaks(await response.arrayBuffer());
            if (!cancelled) setPeaks(computed);
          } catch {
            if (!cancelled) setPeaks(null);
          }
        })();
      } catch (e) {
        if (!cancelled) setFailed(String(e));
      }
    })();

    return () => {
      cancelled = true;
      releasePlayback(track.path);
      if (objectUrl) URL.revokeObjectURL(objectUrl);
    };
  }, [visible, track.path, track.format]);

  const ready = source !== null;

  function toggle() {
    const el = audioRef.current;
    if (!el || !el.src) return;
    if (el.paused) {
      void el.play().catch((e) => setFailed(String(e)));
    } else {
      el.pause();
    }
  }

  // A seek issued before the element has metadata is silently dropped by the
  // browser, which is why clicking mid-waveform did nothing: the waveform is
  // drawn from a separate fetch, so it can be on screen well before the audio
  // element is seekable. Park the request and apply it once ready.
  const pendingSeek = useRef<number | null>(null);

  function onScrub(value: number) {
    const el = audioRef.current;
    if (!el) return;
    if (el.readyState < 1 /* HAVE_METADATA */) {
      pendingSeek.current = value;
      return;
    }
    el.currentTime = value;
    setCurrent(value);
  }

  useEffect(() => {
    const canvas = canvasRef.current;
    const wrap = wrapRef.current;
    if (!canvas) return;
    const render = () => {
      drawWaveform(canvas, {
        peaks: peaks ?? { data: new Float32Array(0), duration: 0 },
        progress: total > 0 ? current / total : 0,
        width: wrap?.clientWidth ?? canvas.clientWidth,
        height: 56,
        played: "var(--color-glow-400)",
        unplayed: "var(--color-ink-600)",
        centre: "var(--color-ink-700)",
        rainbow,
      });
    };
    render();
    if (playing) {
      const raf = requestAnimationFrame(render);
      return () => cancelAnimationFrame(raf);
    }
    // devicePixelRatio changes when a window moves between displays.
    const observer = new ResizeObserver(render);
    if (wrap) observer.observe(wrap);
    window.addEventListener("resize", render);
    return () => {
      observer.disconnect();
      window.removeEventListener("resize", render);
    };
  }, [peaks, current, total, playing, rainbow]);

  const meta = [track.keyscale, track.bpm ? `${track.bpm} BPM` : null, track.vocalLanguage]
    .filter(Boolean)
    .join(" · ");

  return (
    <div ref={cardRef} className={compact ? "space-y-2" : "space-y-3"}>
      <audio
        ref={audioRef}
        className="player-native"
        preload="metadata"
        onPlay={() => {
          setPlaying(true);
          // Starting this track silences whichever one was playing.
          claimPlayback(track.path, () => {
            const other = audioRef.current;
            if (other) other.pause();
            setPlaying(false);
          });
        }}
        onPause={() => {
          setPlaying(false);
          releasePlayback(track.path);
        }}
        onEnded={() => {
          setPlaying(false);
          releasePlayback(track.path);
        }}
        onError={() =>
          setFailed(
            source
              ? "the browser could not decode this file"
              : "could not read audio file",
          )
        }
        onLoadedMetadata={(e) => {
          const el = e.currentTarget;
          const d = el.duration;
          if (Number.isFinite(d) && d > 0) setTotal(d);
          // Apply any seek that arrived before the audio was seekable.
          if (pendingSeek.current !== null) {
            const target = Math.min(pendingSeek.current, Number.isFinite(d) ? d : pendingSeek.current);
            el.currentTime = target;
            setCurrent(target);
            pendingSeek.current = null;
          }
        }}
        onTimeUpdate={(e) => setCurrent(e.currentTarget.currentTime)}
      />

      <div className="flex items-center gap-3">
        <button
          onClick={toggle}
          disabled={!!failed || !ready}
          aria-label={playing ? "Pause" : "Play"}
          className="flex size-10 shrink-0 items-center justify-center rounded-full bg-glow-500 text-white transition hover:brightness-110 disabled:opacity-40"
        >
          {playing ? (
            <svg width="14" height="14" viewBox="0 0 14 14" fill="currentColor">
              <rect x="2" y="1" width="3.5" height="12" rx="1" />
              <rect x="8.5" y="1" width="3.5" height="12" rx="1" />
            </svg>
          ) : (
            <svg width="14" height="14" viewBox="0 0 14 14" fill="currentColor">
              <path d="M3 1.5v11l9-5.5z" />
            </svg>
          )}
        </button>

        <div className="min-w-0 flex-1">
          <p className="truncate text-sm text-ink-100">{track.caption || track.id}</p>
          <p className="mt-0.5 truncate text-xs text-ink-400">
            {meta || "no metadata"} · {formatBytes(track.bytes)}
          </p>
        </div>

        <button
          onClick={() => setRainbow((v) => !v)}
          aria-pressed={rainbow}
          title={rainbow ? "Switch to flat waveform" : "Switch to rainbow waveform"}
          className="shrink-0 rounded-full p-1 transition hover:bg-ink-750"
        >
          <span
            className={`block size-3.5 rounded-full ${
              rainbow ? "" : "bg-ink-500"
            }`}
            style={
              rainbow
                ? { background: "linear-gradient(135deg,#c084fc,#22d3ee)" }
                : undefined
            }
          />
        </button>

        <span className="shrink-0 font-mono text-xs text-ink-400">
          {formatTime(current)} / {total > 0 ? formatTime(total) : "--:--"}
        </span>
      </div>

      <div ref={wrapRef} className="relative">
        <canvas
          ref={canvasRef}
          onClick={(e) => {
            const duration = total || peaks?.duration || 0;
            if (!duration) return;
            onScrub(seekFromEvent(e, duration));
          }}
          className="h-14 w-full cursor-pointer"
          role="slider"
          aria-label="Seek"
          aria-valuemin={0}
          aria-valuemax={Math.round(total)}
          aria-valuenow={Math.round(current)}
          tabIndex={0}
          onKeyDown={(e) => {
            if (e.key === "ArrowRight") onScrub(Math.min(total, current + 5));
            if (e.key === "ArrowLeft") onScrub(Math.max(0, current - 5));
          }}
        />
        {!peaks && (
          <span className="pointer-events-none absolute inset-x-0 bottom-1 text-center text-[10px] text-ink-600">
            decoding waveform…
          </span>
        )}
      </div>

      {failed && (
        <div className="rounded-lg bg-rose-500/10 px-3 py-2 ring-1 ring-rose-500/25">
          <p className="text-xs text-rose-300">{failed}</p>
          {!source && (
            <button
              onClick={() => void library.reveal(track.path)}
              className="mt-1 text-[11px] text-rose-400/80 underline underline-offset-2"
            >
              open the file directly
            </button>
          )}
        </div>
      )}
    </div>
  );
}

function formatTime(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds <= 0) return "0:00";
  const m = Math.floor(seconds / 60);
  const s = Math.floor(seconds % 60);
  return `${m}:${s.toString().padStart(2, "0")}`;
}