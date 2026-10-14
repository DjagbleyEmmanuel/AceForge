import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { ServerStatus, StoreSnapshot, WeightFile } from "./types";

export type { WeightFile };

export const store = {
  snapshot: () => invoke<StoreSnapshot>("store_snapshot"),
  refresh: () => invoke<WeightFile[]>("refresh_weights"),
  catalogueIsStale: () => invoke<boolean>("catalogue_is_stale"),
  downloadScript: (presetId: string) =>
    invoke<string>("preset_download_script", { presetId }),
  freeSpace: () => invoke<number>("models_free_space"),
};

export type DownloadState =
  | "running"
  | "paused"
  | "done"
  | "failed"
  | "cancelled";

export interface DownloadStatus {
  id: string;
  filename: string;
  received: number;
  total: number;
  state: DownloadState;
  error: string | null;
  rate_bps: number;
  eta_secs: number | null;
}

export const downloads = {
  start: (file: WeightFile) =>
    invoke<string>("start_download", {
      filename: file.filename,
      url: file.url,
      total: file.size_bytes,
    }),
  cancel: (id: string) => invoke<null>("cancel_download", { id }),
  statuses: () => invoke<DownloadStatus[]>("download_statuses"),
  clearFinished: () => invoke<null>("clear_finished_downloads"),
  onProgress: (handler: (s: DownloadStatus) => void) =>
    listen<DownloadStatus>("dl://progress", (e) => handler(e.payload)),
};

export const settings = {
  get: () => invoke<SettingsView>("get_settings"),
  setModelsDir: (path: string) =>
    invoke<SettingsView>("set_models_dir", { path }),
  setVaeChunk: (chunk: number) =>
    invoke<SettingsView>("set_vae_chunk", { chunk }),
  setMaxSeq: (value: number) =>
    invoke<SettingsView>("set_max_seq", { value }),
  setAudioCppModelsDir: (path: string) =>
    invoke<SettingsView>("set_audiocpp_models_dir", { path }),
  setLibraryDir: (path: string) =>
    invoke<SettingsView>("set_library_dir", { path }),
};

export const server = {
  start: () => invoke<ServerStatus>("start_server"),
  stop: () => invoke<number>("stop_server"),
  port: () => invoke<number | null>("server_port"),
  /** Whole log, oldest first. */
  logs: (limit?: number) => invoke<string[]>("server_logs", { limit }),
  /** Byte offset of the log end, for resuming. */
  cursor: () => invoke<number>("server_log_cursor"),
  /** Only what was written since `cursor`. Empty when idle. */
  logsSince: (cursor: number, limit?: number) =>
    invoke<string[]>("server_logs_since", { cursor, limit }),
};

/** camelCase: Rust's SettingsView uses rename_all = "camelCase". A mismatched
 *  key deserialises as undefined rather than erroring. */
export interface SettingsView {
  audiocpp_models_dir: string;
  vae_chunk: number;
  max_seq: number;
  models_dir: string;
  resolved_models_dir: string;
  default_models_dir: string;
  autostart_server: boolean;
  env_override: string | null;
  library_dir: string;
  resolved_library_dir: string;
}

/** Field names are camelCase because Rust serialises `GenerateParams` with
 *  `rename_all = "camelCase"`. They must match exactly: a mismatched key
 *  deserialises as empty rather than erroring, which silently cleared the
 *  model pickers. */
export interface GenerateParams {
  caption: string;
  lyrics: string;
  duration: number;
  bpm: number;
  keyscale: string;
  timesignature: string;
  vocalLanguage: string;
  inferenceSteps: number;
  guidanceScale: number;
  shift: number;
  seed: number;
  outputFormat: string;
  synthBatchSize: number;
  lmModel: string;
  synthModel: string;
  solver: string;
  taskType: string;
  /** Seconds into the source to repaint. */
  repaintingStart: number;
  /** Seconds into the source to stop. -1 means "to the end". */
  repaintingEnd: number;
  /** 0-1: fraction of diffusion steps that hear the source. */
  audioCoverStrength: number;
  /** Library path of the audio to repaint or cover. Empty = text-to-music. */
  sourceAudio: string;
  /** Library track whose timbre to copy. ACE-Step has no voice or gender
   *  field, so this is the only real control over vocal character. */
  refAudio: string;
  /** User-supplied name; empty falls back to the caption. */
  title: string;
  /** "male" | "female" | "". Appended to the caption as words — ACE-Step has
   *  no gender parameter, so this is a hint, not a switch. */
  vocalGender: string;
  /** Planner sampling. These used to be hardcoded server-side. */
  lmTemperature: number;
  lmCfgScale: number;
  lmTopP: number;
  /** 0 disables hard top-k; top_p still applies. */
  lmTopK: number;
  lmNegativePrompt: string;
  /** Let the planner rewrite the caption as chain-of-thought first. */
  useCotCaption: boolean;
  /** Comma-separated audio codes that bypass the planner's arrangement. */
  audioCodes: string;
  /** LoRA adapter name from the server's adapter registry. */
  adapter: string;
  adapterScale: number;
  /** Which stem to generate, for lego / extract / complete. Becomes the DiT
   *  instruction: "Generate the {track} track based on the audio context:". */
  track: string;
}

export interface Track {
  id: string;
  path: string;
  format: string;
  bytes: number;
  caption: string;
  lyrics: string;
  bpm: number;
  keyscale: string;
  timesignature: string;
  vocalLanguage: string;
  durationSecs: number;
  createdAt: number;
}

export interface GenerateOutcome {
  tracks: Track[];
  cancelled: boolean;
}

export interface GenerationPreset {
  id: string;
  label: string;
  description: string;
  intent: string;
  approx_seconds: string;
  params: GenerateParams;
}

export const presets = {
  list: () => invoke<GenerationPreset[]>("generation_presets"),
};

/** A parameter bundle the user saved. Distinct from `GenerationPreset`, which
 *  is the curated set shipped with the app. */
export interface UserPreset {
  id: string;
  label: string;
  params: GenerateParams;
  createdAt: number;
}

/** A file the user imported for use as a source or voice reference. */
export interface ImportedFile {
  name: string;
  path: string;
  bytes: number;
}

export const imports = {
  list: () => invoke<ImportedFile[]>("list_imports"),
  add: (sources: string[]) =>
    invoke<ImportedFile[]>("import_audio", { sources }),
  remove: (path: string) => invoke<null>("remove_import", { path }),
};

export const adapters = {
  list: (port?: number) => invoke<string[]>("list_adapters", { port }),
};

export const userPresets = {
  list: () => invoke<UserPreset[]>("user_presets_list"),
  save: (label: string, current: GenerateParams) =>
    invoke<UserPreset>("user_preset_save", { label, current }),
  remove: (id: string) => invoke<null>("user_preset_delete", { id }),
};

export interface MusicalKey {
  value: string;
  tonic: string;
  mode: string;
}
export interface BpmPreset {
  value: number;
  label: string;
}
export interface Language {
  code: string;
  label: string;
}
export interface SamplingValues {
  inferenceSteps: number;
  guidanceScale: number;
  shift: number;
  solver: string;
  synthBatchSize: number;
}
export interface SamplingPreset {
  id: string;
  label: string;
  description: string;
  intendedFor: string;
  approxSeconds: string;
  params: SamplingValues;
}
export interface MusicReference {
  keys: MusicalKey[];
  bpmPresets: BpmPreset[];
  languages: Language[];
  samplingPresets: SamplingPreset[];
  solvers: string[];
}

export const music = {
  reference: () => invoke<MusicReference>("music_reference"),
  applySampling: (presetId: string, current: GenerateParams) =>
    invoke<GenerateParams>("apply_sampling_preset", { presetId, current }),
};

export const generate = {
  run: (port: number, params: GenerateParams) =>
    invoke<GenerateOutcome>("generate_track", { port, params }),
  cancel: (port: number, jobId: string) =>
    invoke<null>("cancel_track", { port, jobId }),
};

export const library = {
  list: () => invoke<Track[]>("library"),
  remove: (path: string) => invoke<null>("delete_track", { path }),
  reveal: (path: string) => invoke<null>("reveal_track", { path }),
};

/** Tauri serves local files over the asset protocol; the webview needs a URL
 *  rather than a filesystem path. */
export function audioUrl(track: Pick<Track, "path">): string {
  return `asset://localhost${encodeURI(track.path)}`;
}

/** Live job id for the run in flight, so Cancel has something to cancel. */
export function onJob(handler: (jobId: string) => void): Promise<() => void> {
  return listen<string>("gen://job", (event) => handler(event.payload));
}

export function onStage(
  handler: (stage: string, detail: string) => void,
): Promise<() => void> {
  return listen<[string, string]>("gen://stage", (event) => {
    handler(event.payload[0], event.payload[1]);
  });
}