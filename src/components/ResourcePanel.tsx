import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

/** camelCase throughout, because that is what the backend serialises. Written
 *  in snake_case it type-checks perfectly and renders `undefined` at runtime,
 *  which is exactly what happened. `sysinfo` asserts the payload keys. */
export interface GpuProcess {
  pid: number;
  usedMib: number;
  name: string | null;
}

export interface SystemStats {
  gpu: {
    name: string;
    /** Share of the card's memory in use. */
    usedMib: number;
    totalMib: number;
    percent: number;
    /** How busy the card is right now. High memory with low utilisation means
     *  something other than the GPU is the bottleneck. */
    utilization: number;
    processes: GpuProcess[];
  } | null;
  ramUsedMib: number;
  ramTotalMib: number;
  ramPercent: number;
}

/** How often to ask. Two seconds: slow enough that nvidia-smi is not a
 *  constant cost, fast enough that a model landing on the card is visible
 *  while you are watching for it. */
const POLL_MS = 2000;

/**
 * Live VRAM and RAM.
 *
 * VRAM is the figure worth putting in front of someone. On a card this size a
 * render is decided by whether the next allocation fits, and a number that
 * updates during the render says whether it will -- rather than leaving it to
 * be discovered from a failed job.
 */
export function useSystemStats(): SystemStats | null {
  const [stats, setStats] = useState<SystemStats | null>(null);

  useEffect(() => {
    let alive = true;
    const read = async () => {
      try {
        const next = await invoke<SystemStats>("system_stats");
        if (alive) setStats(next);
      } catch {
        // Leave the last reading up. A failed sample is not a reason to blank
        // the meter, which would read as "idle" when it means "unknown".
      }
    };
    void read();
    const id = window.setInterval(() => void read(), POLL_MS);
    return () => {
      alive = false;
      window.clearInterval(id);
    };
  }, []);

  return stats;
}

/** One bar. The percentage is the number; the bar is for seeing it move. */
function Meter({
  label,
  detail,
  percent,
  tone,
  warnAt,
}: {
  label: string;
  detail: string;
  percent: number;
  tone: string;
  /** Where the bar changes colour. On the GPU this is the point at which the
   *  next model load is in doubt, so it is not an arbitrary threshold. */
  warnAt: number;
}) {
  const hot = percent >= warnAt;
  return (
    <div className="mt-2">
      <div className="flex items-baseline justify-between gap-2">
        <span className="text-[11px] text-ink-400">{label}</span>
        <span className={`font-mono text-[11px] tabular-nums ${hot ? "text-amber-400" : "text-ink-300"}`}>
          {percent}%
        </span>
      </div>
      <div
        className="mt-1 h-1.5 overflow-hidden rounded-full bg-ink-800"
        role="meter"
        aria-valuenow={percent}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-label={label}
      >
        <div
          className={`h-full rounded-full transition-[width] duration-500 ${tone}`}
          style={{ width: `${Math.min(100, Math.max(0, percent))}%` }}
        />
      </div>
      <p className="mt-1 truncate font-mono text-[10px] text-ink-500" title={detail}>
        {detail}
      </p>
    </div>
  );
}

/**
 * The sidebar resource panel.
 *
 * Per-process VRAM is shown when something holds more than a rounding error,
 * because "the engine is holding 2.4 GB" and "something else is holding 2.4 GB"
 * are very different things to be told while a render is queued behind it.
 */
export function ResourcePanel() {
  const stats = useSystemStats();

  return (
    <div className="panel p-3">
      <div className="flex items-center justify-between">
        <span className="text-[11px] text-ink-400">Resources</span>
      </div>

      {stats?.gpu ? (
        <>
          <Meter
            label="GPU"
            detail={
              stats.gpu.utilization > 0
                ? "card busy"
                : "card idle"
            }
            percent={stats.gpu.utilization}
            tone="bg-sky-400"
            warnAt={99}
          />
          <Meter
            label="VRAM"
            detail={`${mib(stats.gpu.usedMib)} / ${mib(stats.gpu.totalMib)} · ${stats.gpu.name}`}
            percent={stats.gpu.percent}
            tone="bg-glow-500"
            warnAt={80}
          />
          {stats.gpu.processes
            .filter((p) => p.usedMib >= 16)
            .slice(0, 3)
            .map((p) => (
              <p
                key={p.pid}
                className="mt-1 truncate font-mono text-[10px] text-ink-500"
                title={p.name ? `${p.name} (pid ${p.pid})` : `pid ${p.pid}`}
              >
                <span className="text-ink-400">{p.name ?? `pid ${p.pid}`}</span>{" "}
                {mib(p.usedMib)}
              </p>
            ))}
        </>
      ) : (
        <p className="mt-2 text-[10px] leading-relaxed text-ink-500">
          No NVIDIA card reported. VRAM is not shown on this machine.
        </p>
      )}

      {stats && stats.ramTotalMib > 0 && (
        <Meter
          label="RAM"
          detail={`${mib(stats.ramUsedMib)} / ${mib(stats.ramTotalMib)}`}
          percent={stats.ramPercent}
          tone="bg-mint-400"
          warnAt={90}
        />
      )}
    </div>
  );
}

function mib(v: number): string {
  if (v >= 1024) return `${(v / 1024).toFixed(1)} GB`;
  return `${v} MiB`;
}