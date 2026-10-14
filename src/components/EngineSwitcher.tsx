import { useState } from "react";
import {
  engineCapability,
  formatBytes,
  type Engine,
  type EngineId,
} from "../lib/engines";

interface Props {
  selected: EngineId;
  onSelect: (id: EngineId) => void;
  /** Owned by the parent. Reading it here raced adoption and showed both
   *  engines as missing. */
  engines: Engine[];
}

/** Which runtime the Studio panel drives. */
export function EngineSwitcher({ selected, onSelect, engines: list }: Props) {
  const [expanded, setExpanded] = useState(false);

  const active = list.find((e) => e.id === selected);

  // The registry read is a round trip, and adoption can follow it with a long
  // one. Reserving the space means the Studio does not jump when they land.
  if (list.length === 0) {
    return (
      <div className="flex gap-2" aria-busy="true">
        {["ACE-Step (acestep.cpp)", "audio.cpp"].map((name) => (
          <div
            key={name}
            className="flex-1 animate-pulse rounded-xl border border-ink-800 bg-ink-900/40 px-3 py-2.5"
          >
            <div className="h-3 w-28 rounded bg-ink-800" />
            <div className="mt-2 h-2.5 w-40 rounded bg-ink-850" />
          </div>
        ))}
      </div>
    );
  }

  return (
    <div className="space-y-3">
      <div className="flex gap-2">
        {list.map((engine) => {
          const isActive = engine.id === selected;
          return (
            <button
              key={engine.id}
              onClick={() => onSelect(engine.id)}
              disabled={!engine.installed}
              title={
                engine.installed
                  ? `${engine.notes}\n\n${engine.families.join("\n")}`
                  : "not installed"
              }
              className={`flex-1 rounded-xl px-3 py-2.5 text-left transition ${
                isActive
                  ? "bg-glow-500/15 ring-1 ring-glow-500/45"
                  : engine.installed
                    ? "bg-ink-800/70 ring-1 ring-ink-700 hover:bg-ink-800"
                    : "bg-ink-900/50 ring-1 ring-ink-800 opacity-50"
              }`}
            >
              <div className="flex items-center justify-between gap-2">
                <span className="truncate text-xs font-medium text-ink-100">
                  {engine.label}
                </span>
                <span
                  className={`size-1.5 shrink-0 rounded-full ${
                    engine.installed ? "bg-mint-400" : "bg-ink-600"
                  }`}
                />
              </div>
              <p className="mt-0.5 truncate text-[10px] text-ink-500">
                {engine.installed
                  ? engineCapability(engine.kind)
                  : "not installed"}
              </p>
            </button>
          );
        })}
      </div>

      {active && (
        <div className="rounded-xl bg-ink-900/60 p-3 ring-1 ring-ink-800">
          <div className="flex items-start justify-between gap-3">
            <p className="text-[11px] leading-relaxed text-ink-400">
              {active.notes}
            </p>
            <button
              onClick={() => setExpanded((v) => !v)}
              className="shrink-0 text-[11px] text-glow-300 underline-offset-2 hover:underline"
            >
              {expanded ? "less" : "details"}
            </button>
          </div>

          <div className="mt-2 flex flex-wrap gap-1.5">
            {active.families.map((family) => (
              <span
                key={family}
                className="chip bg-ink-800 text-ink-300 ring-1 ring-ink-700"
              >
                {family}
              </span>
            ))}
          </div>

          {expanded && (
            <dl className="mt-3 space-y-1 border-t border-ink-800 pt-3 text-[11px]">
              <div className="flex justify-between gap-3">
                <dt className="text-ink-500">binary</dt>
                <dd className="font-mono text-ink-300">{active.binary}</dd>
              </div>
              <div className="flex justify-between gap-3">
                <dt className="text-ink-500">size</dt>
                <dd className="font-mono text-ink-300">
                  {formatBytes(active.total_bytes)}
                </dd>
              </div>
              <div className="flex justify-between gap-3">
                <dt className="text-ink-500">progress</dt>
                <dd className="text-ink-300">{engineCapability(active.kind)}</dd>
              </div>
              <ul className="mt-2 space-y-0.5">
                {active.files.map((f) => (
                  <li key={f.name} className="flex justify-between gap-3">
                    <span className="truncate font-mono text-ink-400">
                      {f.name}
                    </span>
                    <span className="shrink-0 text-ink-500">
                      {formatBytes(f.size_bytes)}
                    </span>
                  </li>
                ))}
              </ul>
            </dl>
          )}
        </div>
      )}
    </div>
  );
}