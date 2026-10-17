import { invoke } from "@tauri-apps/api/core";
import type { EngineId } from "./engines";
import { listen } from "@tauri-apps/api/event";
import type {
  Layer,
  LayerStack,
  LoudnessTarget,
  NormaliseReport,
  QueuedJob,
  SavedSeed,
  Section,
  SongProject,
  ServerStatus,
  StoreSnapshot,
  TrackNotes,
  WeightFile,
} from "./types";

export type { WeightFile, SavedSeed, Layer, LayerStack, Section, SongProject };

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
  setHotStepModelsDir: (path: string) =>
    invoke<SettingsView>("set_hotstep_models_dir", { path }),
  setLibraryDir: (path: string) =>
    invoke<SettingsView>("set_library_dir", { path }),
};

export const server = {
  start: (engine?: EngineId) =>
    invoke<ServerStatus>("start_server", { engine: engine ?? null }),
  stop: () => invoke<number>("stop_server"),
  port: () => invoke<number | null>("server_port"),
  /** Whole log, oldest first. */
  logs: (limit?: number) => invoke<string[]>("server_logs", { limit }),
  /** Byte offset of the log end, for resuming. */
  cursor: () => invoke<number>("server_log_cursor"),
  /** Only what was written since `cursor`. Empty when idle. */
  logsSince: (cursor: number, limit?: number) =>
    invoke<string[]>("server_logs_since", { cursor, limit }),
  /** Empty the log. Answers with the new cursor, which is always 0. */
  clearLogs: () => invoke<number>("server_clear_logs"),
  /** Solver names the engine will accept. Empty unless it serves `/plugins`,
   *  which is the bundled engine's way of saying it only wants `euler`. */
  solvers: (port: number | null) => invoke<string[]>("engine_solvers", { port }),
  /** The port, but only while an engine is still listening on it. Null once it
   *  has died, which is how the UI stops claiming a dead runtime is live. */
  liveness: () => invoke<number | null>("engine_liveness"),
};

/** camelCase: Rust's SettingsView uses rename_all = "camelCase". A mismatched
 *  key deserialises as undefined rather than erroring. */
export interface SettingsView {
  hotstep_models_dir: string;
  hotstep_adapters_dir: string;
  hotstep_backend: string;
  vae_chunk: number;
  max_seq: number;
  models_dir: string;
  resolved_models_dir: string;
  default_models_dir: string;
  autostart_server: boolean;
  env_override: string | null;
  library_dir: string;
  resolved_library_dir: string;
  covers_dir: string;
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
  // ---- HOT-Step CPP only -------------------------------------------------
  // Ignored by the bundled engine, which reads request fields one at a time.
  /** Encoded as the unconditional branch: how to steer away from a sound. */
  negativePrompt: string;
  /** Which VAE decodes the latents. Empty keeps the loaded one. HOT-Step also
   *  ships a fine-tuned ScragVAE and an optional PP-VAE cleanup pass. */
  vae: string;
  /** Second autoencoder pass over the finished audio, to take fizz off. */
  ppVaeReencode: boolean;
  /** The planner LM's own adapter slot, separate from the DiT one. */
  lmAdapter: string;
  lmAdapterScale: number;
  /** One of HOT-Step's shipped Lua postprocess plugins, by name. */
  postprocessPlugin: string;
  /** Fraction of the sequence CFG applies to. 1 = all of it. */
  lmCfgCutoffRatio: number;
  /** Comma separated timestep schedule, replacing the solver's own. */
  customTimesteps: string;
  mp3Bitrate: number;
  /** -1 keeps the model's own window length. */
  ditSlidingWindow: number;
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
  /** Absolute path to the assigned cover image, when one exists. */
  cover: string | null;
}

export interface GenerateOutcome {
  tracks: Track[];
  cancelled: boolean;
}

/** Wire shape of `Preset`. The struct is `rename_all = "camelCase"` in Rust,
 *  so these are camelCase too -- `params` is a `GenerateParams` and camelCase
 *  for the same reason. */
export interface GenerationPreset {
  id: string;
  label: string;
  description: string;
  intent: string;
  approxSeconds: string;
  /** Whether this preset is allowed to replace the lyrics field. */
  setsLyrics: boolean;
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

/** Durable generation queue. `add` returns as soon as the job is on disk; the
 *  backend runner decides when it starts. */
export const queue = {
  list: () => invoke<QueuedJob[]>("queue_list"),
  add: (params: GenerateParams) => invoke<QueuedJob>("queue_add", { params }),
  cancel: (jobId: string) => invoke<null>("queue_cancel", { jobId }),
  retry: (jobId: string) => invoke<QueuedJob>("queue_retry", { jobId }),
  discard: (jobId: string) => invoke<null>("queue_discard", { jobId }),
  clearDone: () => invoke<null>("queue_clear_done"),
  clearFailed: () => invoke<null>("queue_clear_failed"),
  reset: () => invoke<null>("queue_reset"),
  onChanged: (handler: () => void) =>
    listen("queue://changed", () => handler()),
  onStage: (
    handler: (jobId: string, stage: string, detail: string) => void,
  ) =>
    listen<[string, string, string]>("queue://stage", (event) =>
      handler(event.payload[0], event.payload[1], event.payload[2]),
    ),
};

/** Audio seconds per second of generation. 3.0 means a minute of music took
 *  twenty seconds. Only meaningful once a job finished with both clocks set. */
export function renderRatio(job: QueuedJob): number | null {
  if (job.state !== "done" || job.audio_secs <= 0.1 || job.elapsed_secs <= 0.1) {
    return null;
  }
  return job.audio_secs / job.elapsed_secs;
}

/** Wildcard preview. Runs the same Rust expander the engine-facing path uses. */
export const wildcards = {
  preview: (text: string, seed: number) =>
    invoke<string>("expand_wildcards", { text, seed }),
  /** Cheap check for the hint: does this text contain a `{a|b}` group at all? */
  hasWildcards: (text: string) => /\{[^{}]*\|/.test(text),
};

/** Per-track annotations: section markers and the pinned A/B pair. */
export const annotations = {
  notes: (trackId: string) => invoke<TrackNotes>("track_notes", { trackId }),
  addMarker: (trackId: string, atSecs: number, label: string) =>
    invoke<TrackNotes>("add_marker", { trackId, atSecs, label }),
  removeMarker: (trackId: string, index: number) =>
    invoke<TrackNotes>("remove_marker", { trackId, index }),
  songs: () => invoke<SongProject[]>("songs"),
  saveSong: (song: SongProject) => invoke<SongProject[]>("save_song", { song }),
  deleteSong: (id: string) => invoke<SongProject[]>("delete_song", { id }),
  layerStacks: () => invoke<LayerStack[]>("layer_stacks"),
  saveLayerStack: (stack: LayerStack) =>
    invoke<LayerStack[]>("save_layer_stack", { stack }),
  deleteLayerStack: (id: string) =>
    invoke<LayerStack[]>("delete_layer_stack", { id }),
  seeds: () => invoke<SavedSeed[]>("saved_seeds"),
  saveSeed: (seed: number, caption: string) =>
    invoke<SavedSeed[]>("save_seed", { seed, caption }),
  toggleStar: (seed: number) => invoke<SavedSeed[]>("toggle_star_seed", { seed }),
  removeSeed: (seed: number) => invoke<SavedSeed[]>("remove_seed", { seed }),
  abSlots: () => invoke<[string, string] | null>("ab_slots"),
  setAbSlot: (index: number, trackId: string) =>
    invoke<[string, string]>("set_ab_slot", { index, trackId }),
};

export interface CoverEntry {
  path: string;
  name: string;
  fromUserDir: boolean;
}

/** Cover art: the user's directory first, then the bundled set. */
export const covers = {
  list: () => invoke<CoverEntry[]>("list_covers"),
  setDir: (path: string) => invoke<SettingsView>("set_covers_dir", { path }),
};

/** Crop a track to a time range. Writes a new file; the original is kept. */
export type TimingSource = "manual" | "sidecar";

export interface LyricLine {
  atSecs: number;
  text: string;
}

export interface LyricTimings {
  lines: LyricLine[];
  source: TimingSource;
}

/** Lyric timings: a sidecar `.lrc` if present, else what the user tapped in.
 *
 *  The engine produces none of its own, so without one of these there is
 *  nothing to follow and the UI shows plain lyrics instead of pretending.
 */
export const lyricsApi = {
  timings: (trackPath: string) =>
    invoke<LyricTimings | null>("lyric_timings", { trackPath }),
  save: (trackPath: string, lines: LyricLine[]) =>
    invoke<null>("save_lyric_timings", { trackPath, lines }),
};

export interface SyncReport {
  path: string;
  lines: number;
  tookSecs: number;
  error: string | null;
}

export interface SyncProgress {
  running: boolean;
  done: number;
  total: number;
  current: string;
  error: string | null;
}

/** Backfill lyric timings for songs rendered before the engine could emit them.
 *  Runs one at a time in the background; the audio is discarded each pass. */
export const lyricSync = {
  pending: () => invoke<Track[]>("lyric_sync_pending"),
  start: (paths: string[]) => invoke<SyncProgress>("lyric_sync_start", { paths }),
  progress: () => invoke<SyncProgress>("lyric_sync_progress"),
  cancel: () => invoke<null>("lyric_sync_cancel"),
  onProgress: (handler: (p: SyncProgress) => void) =>
    listen<SyncProgress>("lyricsync://progress", (e) => handler(e.payload)),
};

export interface CropResult {
  seconds: number;
  replaced: boolean;
  /** The caller asked to overwrite but the source is compressed, so a sibling
   *  was written instead and the original survives. */
  keptOriginal: boolean;
}

export const trimming = {
  trim: (path: string, fromSecs: number, toSecs: number) =>
    invoke<number>("trim_track", { path, fromSecs, toSecs }),
  /** `keep: "only"` keeps the highlighted range, `"except"` cuts it out.
   *  `replace` overwrites the file; the UI confirms that separately. */
  crop: (
    path: string,
    fromSecs: number,
    toSecs: number,
    replace: boolean,
    keep: "only" | "except",
  ) => invoke<CropResult>("crop_track", { path, fromSecs, toSecs, replace, keep }),
};

/** Loudness normalisation. Writes a new file; the original is left alone. */
export const mastering = {
  normalise: (path: string, target: LoudnessTarget, referencePath?: string) =>
    invoke<NormaliseReport>("normalise_track", {
      path,
      target,
      referencePath: referencePath ?? null,
    }),
};

/** Display names and descriptions for the targets, in the order shown. */
export const LOUDNESS_TARGETS: {
  id: LoudnessTarget;
  label: string;
  note: string;
}[] = [
  { id: "streaming14", label: "-14", note: "streaming default" },
  { id: "club9", label: "-9", note: "dense club master" },
  { id: "broadcast24", label: "-24", note: "broadcast / R128" },
  { id: "loud6", label: "-6", note: "loud, limited hard" },
];

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