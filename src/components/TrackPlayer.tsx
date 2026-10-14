import { useEffect, useRef, useState } from "react";
import { annotations, library, trimming, type Track } from "../lib/api";
import { loadTrackSource } from "../lib/audio";
import { claimPlayback, releasePlayback } from "../lib/playerRegistry";
import { isLooping, toggleLooping } from "../lib/loop";
import {
  computePeaks,
  drawWaveform,
  seekFromEvent,
  type Peaks,
} from "../lib/waveform";
import { formatBytes, type Marker } from "../lib/types";
import { SyncedLyrics } from "./SyncedLyrics";
import {
  nativeOutputRate,
  rebindOutputRate,
  releaseOutputRate,
  type OutputRate,
  type RateBinding,
} from "../lib/outputRate";

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
  // Preview controls. Rate and pitch lock are per-player on purpose: auditioning
  // a take at half speed while the others stay normal is the point.
  const [rate, setRate] = useState(1);
  const [pitchLock, setPitchLock] = useState(true);
  const [outRate, setOutRate] = useState<OutputRate>(() => nativeOutputRate());
  const [looping, setLoopingHere] = useState(() => isLooping(track.path));
  const [rateUnavailable, setRateUnavailable] = useState(false);
  const bindingRef = useRef<RateBinding | null>(null);
  const [markers, setMarkers] = useState<Marker[]>([]);
  const [trimFrom, setTrimFrom] = useState("");
  const [trimTo, setTrimTo] = useState("");
  const [trimMsg, setTrimMsg] = useState<string | null>(null);
  const [marking, setMarking] = useState(false);
  const [markerLabel, setMarkerLabel] = useState("");

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

  // Markers are keyed by track id, so they reload on their own and not as part
  // of the audio load: a failed decode must not hide them.
  useEffect(() => {
    let alive = true;
    void annotations
      .notes(track.id)
      .then((n) => {
        if (alive) setMarkers(n.markers);
      })
      .catch(() => {
        if (alive) setMarkers([]);
      });
    return () => {
      alive = false;
    };
  }, [track.id]);

  // An AudioContext outlives the component unless it is closed, and a closed
  // context leaves the element silent. Every card that ever bound one must
  // release it on unmount.
  useEffect(
    () => () => {
      releaseOutputRate(bindingRef.current);
      bindingRef.current = null;
    },
    [],
  );

  // Rate and pitch lock are DOM properties rather than attributes, so they are
  // applied imperatively. `preservesPitch` is vendor-prefixed in WebKitGTK,
  // which is the webview this app ships in.
  useEffect(() => {
    const el = audioRef.current;
    if (!el) return;
    el.playbackRate = rate;
    const withPitch = el as HTMLAudioElement & {
      preservesPitch?: boolean;
      mozPreservesPitch?: boolean;
      webkitPreservesPitch?: boolean;
    };
    withPitch.preservesPitch = pitchLock;
    withPitch.mozPreservesPitch = pitchLock;
    withPitch.webkitPreservesPitch = pitchLock;
  }, [rate, pitchLock, visible]);

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
          releasePlayback(track.path);
          // Restarting in place keeps the playback claim, so the card does not
          // flicker back to paused between repeats.
          if (looping) {
            const a = audioRef.current;
            if (a) {
              a.currentTime = 0;
              void a.play().catch(() => undefined);
            }
            return;
          }
          setPlaying(false);
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

        <div className="flex shrink-0 items-center gap-1">
          <button
            onClick={() => setRate((r) => Math.max(0.5, Math.round((r - 0.05) * 100) / 100))}
            disabled={rate <= 0.5}
            aria-label="Slower"
            title="Slow playback by 0.05x"
            className="rounded px-1.5 py-0.5 font-mono text-xs text-ink-400 transition hover:bg-ink-750 hover:text-ink-200 disabled:opacity-30"
          >
            −
          </button>
          <button
            onClick={() => setRate(1)}
            title="Back to 1x"
            className={`min-w-[2.75rem] rounded px-1 py-0.5 font-mono text-xs transition ${
              rate === 1
                ? "text-ink-500"
                : "bg-glow-500/20 text-glow-300 ring-1 ring-glow-500/40"
            }`}
          >
            {rate.toFixed(2)}x
          </button>
          <button
            onClick={() => setRate((r) => Math.min(2, Math.round((r + 0.05) * 100) / 100))}
            disabled={rate >= 2}
            aria-label="Faster"
            title="Speed up by 0.05x"
            className="rounded px-1.5 py-0.5 font-mono text-xs text-ink-400 transition hover:bg-ink-750 hover:text-ink-200 disabled:opacity-30"
          >
            +
          </button>
          <button
            onClick={() => setPitchLock((v) => !v)}
            aria-pressed={!pitchLock}
            title={
              pitchLock
                ? "Pitch is locked: speed changes tempo only. Click to let pitch shift too."
                : "Pitch shifts with speed, like turning a jog wheel. Click to lock pitch."
            }
            className={`ml-1 rounded px-1.5 py-0.5 font-mono text-[10px] uppercase tracking-wide transition ${
              pitchLock
                ? "bg-ink-800 text-ink-500"
                : "bg-amber-500/20 text-amber-300 ring-1 ring-amber-500/40"
            }`}
          >
            {pitchLock ? "lock" : "dj"}
          </button>
          <button
            onClick={() => setLoopingHere(toggleLooping(track.path))}
            aria-pressed={looping}
            aria-label="Loop this track"
            title={
              looping
                ? "Looping this track — click to stop repeating"
                : "Loop this track"
            }
            className={`ml-1 flex h-[18px] items-center gap-0.5 rounded px-1 transition ${
              looping
                ? "bg-glow-500/20 text-glow-300 ring-1 ring-glow-500/40"
                : "text-ink-600 hover:text-ink-400"
            }`}
          >
            <svg
              width="11"
              height="11"
              viewBox="0 0 16 16"
              fill="none"
              stroke="currentColor"
              strokeWidth="1.7"
              strokeLinecap="round"
              strokeLinejoin="round"
            >
              <path d="M2.5 6.5A3.5 3.5 0 0 1 6 3h6" />
              <path d="M10 1l2 2-2 2" />
              <path d="M13.5 9.5A3.5 3.5 0 0 1 10 13H4" />
              <path d="M6 15l-2-2 2-2" />
            </svg>
            <span className="text-[9px] font-bold leading-none">1</span>
          </button>
          <button
            onClick={() => {
              const next = outRate === 48000 ? 44100 : 48000;
              const el = audioRef.current;
              if (!el || !source) return;
              try {
                bindingRef.current = rebindOutputRate(
                  el,
                  bindingRef.current,
                  next,
                );
                setOutRate(next);
                setRateUnavailable(false);
              } catch {
                // A refused AudioContext must not break playback. The toggle
                // stays visible but inert so it is clear why nothing happened.
                setRateUnavailable(true);
                bindingRef.current = null;
              }
            }}
            disabled={rateUnavailable || !source}
            title={
              rateUnavailable
                ? "this webview would not create an AudioContext, so the output rate cannot be changed"
                : outRate === 48000
                  ? "Play through a 44.1 kHz graph, as streaming services deliver"
                  : "Play through a 48 kHz graph, matching the render rate"
            }
            className={`ml-1 rounded px-1.5 py-0.5 font-mono text-[10px] transition disabled:opacity-40 ${
              outRate === 44100
                ? "bg-glow-500/20 text-glow-300 ring-1 ring-glow-500/40"
                : "bg-ink-800 text-ink-500"
            }`}
          >
            {outRate === 44100 ? "44.1k" : "48k"}
          </button>
        </div>

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
        {markers.map((m, i) => {
          const dur = total || peaks?.duration || 0;
          if (dur <= 0) return null;
          return (
            <button
              key={`${m.atSecs}-${i}`}
              onClick={() => onScrub(m.atSecs)}
              title={`${m.label} — ${formatTime(m.atSecs)}`}
              className="absolute bottom-0 top-0 w-px -translate-x-1/2"
              style={{ left: `${(m.atSecs / dur) * 100}%` }}
            >
              <span className="absolute -top-0.5 left-0 block size-1.5 -translate-x-1/2 rounded-full bg-amber-400" />
            </button>
          );
        })}
        {!peaks && (
          <span className="pointer-events-none absolute inset-x-0 bottom-1 text-center text-[10px] text-ink-600">
            decoding waveform…
          </span>
        )}
      </div>

      {/* Crop. Writes a new file rather than editing in place: trimming is a
          judgement call, and the untrimmed render has to survive a bad one. */}
      <div className="flex flex-wrap items-center gap-2">
        <span className="text-[10px] text-ink-600">crop</span>
        <input
          type="number"
          min={0}
          step={0.1}
          value={trimFrom}
          onChange={(e) => setTrimFrom(e.target.value)}
          placeholder="0.0"
          className="field w-20 px-1.5 py-0.5 font-mono text-[11px]"
        />
        <span className="text-ink-600">→</span>
        <input
          type="number"
          min={0}
          step={0.1}
          value={trimTo}
          onChange={(e) => setTrimTo(e.target.value)}
          placeholder={total > 0 ? total.toFixed(1) : "end"}
          className="field w-20 px-1.5 py-0.5 font-mono text-[11px]"
        />
        <button
          onClick={() => setTrimFrom(current.toFixed(1))}
          title="Use the playhead as the start"
          className="chip px-1.5 py-0.5 text-[10px]"
        >
          in
        </button>
        <button
          onClick={() => setTrimTo(current.toFixed(1))}
          title="Use the playhead as the end"
          className="chip px-1.5 py-0.5 text-[10px]"
        >
          out
        </button>
        <button
          onClick={async () => {
            const from = Number(trimFrom);
            const to = trimTo.trim() === "" ? total : Number(trimTo);
            if (!Number.isFinite(from) || !Number.isFinite(to)) return;
            setTrimMsg(null);
            try {
              const secs = await trimming.trim(track.path, from, to);
              setTrimMsg(`cropped copy: ${secs.toFixed(1)}s`);
              setTrimFrom("");
              setTrimTo("");
            } catch (e) {
              setTrimMsg(String(e));
            }
          }}
          disabled={!source || total <= 0}
          className="chip px-2 py-1 text-[11px] disabled:opacity-40"
          title="Save a cropped copy to the library"
        >
          save crop
        </button>
        {trimMsg && <span className="text-[10px] text-ink-400">{trimMsg}</span>}
      </div>

      <div className="flex flex-wrap items-center gap-2">
        <button
          onClick={() => setMarking((v) => !v)}
          aria-pressed={marking}
          title="Drop a named section marker at the playhead"
          className={`chip px-2 py-1 text-[11px] ${
            marking ? "bg-amber-500/20 text-amber-300 ring-1 ring-amber-500/40" : ""
          }`}
        >
          + section
        </button>
        {marking && (
          <>
            <input
              value={markerLabel}
              onChange={(e) => setMarkerLabel(e.target.value)}
              onKeyDown={(e) => {
                if (e.key !== "Enter" || !markerLabel.trim()) return;
                void annotations
                  .addMarker(track.id, current, markerLabel.trim())
                  .then((n) => {
                    setMarkers(n.markers);
                    setMarkerLabel("");
                    setMarking(false);
                  });
              }}
              placeholder="chorus"
              className="field min-w-0 flex-1 py-1 text-xs"
              autoFocus
            />
            <button
              onClick={() => {
                if (!markerLabel.trim()) return;
                void annotations
                  .addMarker(track.id, current, markerLabel.trim())
                  .then((n) => {
                    setMarkers(n.markers);
                    setMarkerLabel("");
                    setMarking(false);
                  });
              }}
              className="chip px-2 py-1 text-[11px]"
            >
              save
            </button>
          </>
        )}
        {markers.map((m, i) => (
          <span
            key={`${m.atSecs}-tag-${i}`}
            className="chip gap-1 px-1.5 py-1 text-[11px] text-amber-300"
          >
            <button onClick={() => onScrub(m.atSecs)} title={`jump to ${formatTime(m.atSecs)}`}>
              {m.label}
            </button>
            <button
              onClick={() =>
                void annotations.removeMarker(track.id, i).then((n) => setMarkers(n.markers))
              }
              title="remove"
              className="text-ink-500 hover:text-rose-300"
            >
              ×
            </button>
          </span>
        ))}
      </div>

      <SyncedLyrics
        trackPath={track.path}
        lyrics={track.lyrics}
        currentTime={current}
      />

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