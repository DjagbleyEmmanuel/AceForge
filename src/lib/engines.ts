import { invoke } from "@tauri-apps/api/core";

export type EngineId = "acestep" | "hotstep";
export type EngineKind = "asyncJobs" | "blockingRequest";

export interface EngineFile {
  name: string;
  size_bytes: number;
  present: boolean;
}

export interface Engine {
  id: EngineId;
  label: string;
  kind: EngineKind;
  families: string[];
  binary: string;
  installed: boolean;
  files: EngineFile[];
  total_bytes: number;
  notes: string;
}

export interface EngineInspection {
  ok: boolean;
  id: string | null;
  label: string | null;
  problems: string[];
  files: EngineFile[];
}

export const engines = {
  list: () => invoke<Engine[]> ("list_engines"),
  adoptBundled: () => invoke<boolean>("adopt_bundled_engine"),
  adoptBundledHotStep: () => invoke<boolean>("adopt_bundled_hotstep"),
  inspect: (path: string) => invoke<EngineInspection>("inspect_engine", { path }),
  install: (path: string) => invoke<string>("install_engine", { path }),
  remove: (id: string) => invoke<null>("remove_engine", { id }),
  dir: () => invoke<string>("engines_dir"),
};

/** Never throws. A renamed or missing field arrives as `undefined`, and this
 *  is called from render: `undefined.toFixed()` would throw and React would
 *  unmount the whole tree, turning one bad value into a blank window. */
export function formatBytes(bytes: number | undefined | null): string {
  if (typeof bytes !== "number" || !Number.isFinite(bytes) || bytes <= 0) {
    return "—";
  }
  const units = ["B", "KiB", "MiB", "GiB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  const d = unit === 0 ? 0 : value >= 100 ? 0 : value >= 10 ? 1 : 2;
  return `${value.toFixed(d)} ${units[unit]}`;
}

/** How an engine reports a run. Both bundled engines poll and can be
 *  cancelled; a hand-installed third-party engine may not. */
export function engineCapability(kind: EngineKind): string {
  return kind === "asyncJobs"
    ? "polls progress and can be cancelled"
    : "one blocking request, no progress, no cancel";
}