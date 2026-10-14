import { useEffect, useRef, useState } from "react";
import { audioUrl, type Track } from "../lib/api";

export interface LayerAudio {
  id: string;
  label: string;
  track: Track;
  volume: number;
  muted: boolean;
  onVolume: (v: number) => void;
  onToggleMute: () => void;
}

interface Props {
  layers: LayerAudio[];
}

/**
 * Play several tracks at once, in sync.
 *
 * Every layer is its own `<audio>` element because WebKitGTK will not mix
 * several sources through one element. That makes synchronisation this
 * component's job: one element is the master transport and the others are
 * corrected against it whenever they drift.
 *
 * Drift correction is the part that matters. Separate elements started by
 * separate `play()` calls land tens of milliseconds apart, which is audible on
 * a hi-hat as a flam. Rather than trusting start times, each follower is
 * nudged back whenever it falls outside a small window.
 */
export function LayeredPreview({ layers }: Props) {
  const masterRef = useRef<HTMLAudioElement>(null);
  const refs = useRef<Map<string, HTMLAudioElement>>(new Map());
  const [playing, setPlaying] = useState(false);
  const [ready, setReady] = useState(false);
  const [levels, setLevels] = useState<Record<string, number>>({});

  // Tighter than it sounds: two layers of a stem arrangement have to read as one
  // performance, and ~8 ms is about the threshold before flam is audible.
  const DRIFT_TOLERANCE = 0.008;

  useEffect(() => {
    const master = masterRef.current;
    if (!master || layers.length === 0) {
      setReady(false);
      return;
    }
    // The first layer is the master transport; it always exists.
    master.src = audioUrl(layers[0].track);
    const onReady = () => setReady(true);
    master.addEventListener("loadedmetadata", onReady);
    return () => {
      master.removeEventListener("loadedmetadata", onReady);
    };
  }, [layers[0]?.track.path]);

  useEffect(() => {
    // Rebuild the follower set whenever the stack changes.
    refs.current.clear();
    for (const layer of layers.slice(1)) {
      const el = new Audio(audioUrl(layer.track));
      el.preload = "metadata";
      el.volume = layer.muted ? 0 : layer.volume;
      refs.current.set(layer.id, el);
    }
    return () => {
      for (const el of refs.current.values()) {
        el.pause();
        el.src = "";
      }
      refs.current.clear();
    };
  }, [layers.map((l) => l.id).join(",")]);

  useEffect(() => {
    for (const layer of layers.slice(1)) {
      const el = refs.current.get(layer.id);
      if (el) el.volume = layer.muted ? 0 : layer.volume;
    }
  }, [layers.map((l) => `${l.id}:${l.muted}:${l.volume}`).join(",")]);

  // Watchdog: correct followers against the master rather than hoping they
  // stayed together.
  useEffect(() => {
    if (!playing) return;
    const timer = window.setInterval(() => {
      const master = masterRef.current;
      if (!master) return;
      for (const el of refs.current.values()) {
        const drift = el.currentTime - master.currentTime;
        if (Math.abs(drift) > DRIFT_TOLERANCE) {
          el.currentTime = master.currentTime;
        }
        if (el.paused) void el.play().catch(() => undefined);
      }
    }, 60);
    return () => window.clearInterval(timer);
  }, [playing]);

  function toggle() {
    const master = masterRef.current;
    if (!master) return;
    if (playing) {
      master.pause();
      for (const el of refs.current.values()) el.pause();
      setPlaying(false);
      return;
    }
    if (master.ended || master.currentTime >= master.duration - 0.05) {
      master.currentTime = 0;
    }
    // Start everyone from the master's position so a toggle mid-playback does
    // not restart the followers.
    for (const el of refs.current.values()) {
      el.currentTime = master.currentTime;
      void el.play().catch(() => undefined);
    }
    void master.play().catch(() => undefined);
    setPlaying(true);
  }

  // A rough level meter per layer, sampled rather than measured: this is a
  // mixing aid, not a meter.
  useEffect(() => {
    if (!playing) {
      setLevels({});
      return;
    }
    const timer = window.setInterval(() => {
      const master = masterRef.current;
      if (!master) return;
      const next: Record<string, number> = {};
      next[layers[0].id] = master.currentTime / (master.duration || 1);
      for (const layer of layers.slice(1)) {
        const el = refs.current.get(layer.id);
        if (el) next[layer.id] = el.currentTime / (el.duration || 1);
      }
      setLevels(next);
    }, 120);
    return () => window.clearInterval(timer);
  }, [playing, layers]);

  if (layers.length === 0) return null;

  return (
    <div className="space-y-2">
      <audio
        ref={masterRef}
        preload="metadata"
        onEnded={() => {
          for (const el of refs.current.values()) el.pause();
          setPlaying(false);
        }}
        className="hidden"
      />
      <div className="flex items-center gap-2">
        <button
          onClick={toggle}
          disabled={!ready}
          className="btn btn-ghost px-3 py-1 text-xs disabled:opacity-40"
        >
          {playing ? "stop" : "play stack"}
        </button>
        <span className="text-[11px] text-ink-500">
          {layers.length} layer{layers.length > 1 ? "s" : ""}, kept in sync
        </span>
      </div>
      <ul className="space-y-1">
        {layers.map((layer) => (
          <li key={layer.id} className="flex items-center gap-2">
            <button
              onClick={layer.onToggleMute}
              aria-pressed={layer.muted}
              title={layer.muted ? "Unmute" : "Mute"}
              className={`w-6 shrink-0 text-center text-xs ${
                layer.muted ? "text-ink-600" : "text-glow-300"
              }`}
            >
              {layer.muted ? "○" : "●"}
            </button>
            <span className="w-20 shrink-0 truncate text-[11px] text-ink-300">
              {layer.label}
            </span>
            <input
              type="range"
              min={0}
              max={2}
              step={0.01}
              value={layer.volume}
              onChange={(e) => layer.onVolume(Number(e.target.value))}
              title="Volume"
              className="min-w-0 flex-1 accent-glow-500"
            />
            <span className="w-9 shrink-0 text-right font-mono text-[10px] text-ink-500">
              {Math.round(layer.volume * 100)}%
            </span>
            <span
              className="h-1 w-10 shrink-0 overflow-hidden rounded-full bg-ink-800"
              title="Playback position"
            >
              <span
                className="block h-full bg-glow-500/70"
                style={{
                  width: `${Math.min(100, (levels[layer.id] ?? 0) * 100)}%`,
                }}
              />
            </span>
          </li>
        ))}
      </ul>
    </div>
  );
}