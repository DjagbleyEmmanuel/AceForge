import type { GenerateParams } from "./api";

export type Family = "acestep" | "yue2" | "minimax";

export type Role =
  | "Planner"
  | "TextEncoder"
  | "Dit"
  | "Vae"
  /** YuE2's autoregressive composer: a different pipeline from `Planner`, not
   *  a variant of it, and loaded by its own backend. */
  | "Composer"
  | "DecoderAdapter"
  | "DepthDecoder"
  | "ConditionEncoder";

export interface WeightFile {
  filename: string;
  url: string;
  /** Which pipeline this weight belongs to. */
  family: Family;
  /** Subdirectory under the models folder it has to land in, "" for the top
   *  level. Each backend looks in its own directory. */
  subdir: string;
  size_bytes: number;
  role: Role;
  present: boolean;
  quant: string;
  variant: string;
  backend_supported: boolean;
  partial_bytes: number;
  size_mismatch: boolean;
}

export interface Preset {
  id: string;
  label: string;
  description: string;
  total_bytes: number;
  fits_4gb: boolean;
  files: string[];
}

export interface Validation {
  ok: boolean;
  models_dir: string;
  dir_exists: boolean;
  free_bytes: number;
  present: string[];
  missing: string[];
  reason: string | null;
}

export interface StoreSnapshot {
  repo: string;
  models_dir: string;
  files: WeightFile[];
  presets: Preset[];
  validation: Validation;
}

export interface ServerStatus {
  running: boolean;
  pid: number | null;
  port: number | null;
  ready: boolean;
  log_tail: string[];
  /** Weight files present when the engine started. The backend scans once and
   *  never rescans, so anything newer is on disk but not yet usable. */
  loadedWeights: string[];
}

const ROLE_LABEL: Record<Role, string> = {
  Planner: "Planner LM",
  TextEncoder: "Text encoder",
  Dit: "DiT",
  Vae: "VAE",
  Composer: "Composer (AR)",
  DecoderAdapter: "Decoder adapter",
  DepthDecoder: "Depth decoder",
  ConditionEncoder: "Condition encoder",
};

export const FAMILY_LABEL: Record<Family, string> = {
  acestep: "ACE-Step 1.5",
  yue2: "YuE2",
  minimax: "MiniMax-Music3",
};

export function familyLabel(family: Family): string {
  return FAMILY_LABEL[family] ?? family;
}

export function roleLabel(role: Role): string {
  return ROLE_LABEL[role];
}

/** Never throws: called from render, so a missing field must render as a dash
 *  rather than take the tree down with it. */
export function formatBytes(bytes: number | undefined | null): string {
  if (typeof bytes !== "number" || !Number.isFinite(bytes) || bytes <= 0) {
    return "—";
  }
  const units = ["B", "KiB", "MiB", "GiB", "TiB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  // A 300 KB mp3 must not read as "0 MiB".
  const decimals = unit === 0 ? 0 : value >= 100 ? 0 : value >= 10 ? 1 : 2;
  return `${value.toFixed(decimals)} ${units[unit]}`;
}

export function formatDuration(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds <= 0) return "—";
  if (seconds < 60) return `${seconds.toFixed(1)}s`;
  const m = Math.floor(seconds / 60);
  return `${m}m ${Math.round(seconds % 60)}s`;
}
/** Queue lifecycle. `interrupted` means the app exited mid-render: the job
 *  never resumes on its own, so a restart can never run the same work twice. */
export type JobState =
  | "queued"
  | "running"
  | "done"
  | "failed"
  | "cancelled"
  | "interrupted";

export interface QueuedJob {
  id: string;
  params: GenerateParams;
  state: JobState;
  error: string | null;
  track_ids: string[];
  created_at: number;
  started_at: number | null;
  finished_at: number | null;
  audio_secs: number;
  elapsed_secs: number;
}

export interface Marker {
  atSecs: number;
  label: string;
}

export interface TrackNotes {
  markers: Marker[];
}

export type LoudnessTarget = "streaming14" | "club9" | "broadcast24" | "loud6";

export interface NormaliseReport {
  measuredLufs: number | null;
  targetLufs: number;
  appliedGainDb: number;
  limited: boolean;
  peakBeforeDbfs: number;
  peakAfterDbfs: number;
  /** Spectral tilt applied to match a reference, if one was given. */
  brightnessTiltDb: number | null;
}

export interface SavedSeed {
  seed: number;
  caption: string;
  starred: boolean;
  createdAt: number;
}

export interface Layer {
  id: string;
  label: string;
  path: string;
}

export interface LayerStack {
  id: string;
  name: string;
  layers: Layer[];
  updatedAt: number;
}

export interface Section {
  id: string;
  name: string;
  lyrics: string;
  lengthSecs: number;
  variants: string[];
  chosen: number | null;
  basePath: string | null;
}

export interface SongProject {
  id: string;
  name: string;
  caption: string;
  bpm: number;
  keyscale: string;
  timesignature: string;
  sections: Section[];
  updatedAt: number;
}
