import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export interface AudioCppModel {
  id: string;
  family: string;
  task: string;
  mode: string;
  path: string;
  sessionOptions?: Record<string, string> | null;
}

export interface AudioCppStatus {
  running: boolean;
  port: number | null;
  models: AudioCppModel[];
  error: string | null;
}

/** YuE2 request options. Names come from audio.cpp's `yue2_cli_interface()`. */
export interface YuE2Params {
  style: string;
  lyrics: string;
  seed: number | null;
  guidanceScale: number | null;
  numInferenceSteps: number | null;
  cot: string | null;
  stopAfter: string | null;
  /** YuE2's length control. Semantic codec tokens, not seconds -- audio.cpp
   *  documents no conversion rate. Default 9000. */
  semanticMaxTokens: number | null;
  semanticMinTokens: number | null;
}

/** One selectable model plus everything the picker needs to describe it. */
export interface ModelOption {
  entry: AudioCppModel;
  label: string;
  quant: string;
  taskLabel: string;
  sizeBytes: number;
  hasVae: boolean;
  hasSidecars: boolean;
}

export interface ScanResult {
  directory: string;
  options: ModelOption[];
  warnings: string[];
}

export interface Rendered {
  path: string;
  bytes: number;
  seconds: number;
  elapsedSecs: number;
  sampleRate: number;
}

export const audiocpp = {
  start: (models: AudioCppModel[]) =>
    invoke<AudioCppStatus>("audiocpp_start", { models }),
  models: () => invoke<AudioCppModel[]>("audiocpp_models"),
  stop: () => invoke<null>("audiocpp_stop"),
  render: (model: string, params: YuE2Params) =>
    invoke<Rendered>("audiocpp_render", { model, params }),
  cancel: () => invoke<null>("audiocpp_cancel"),
  probe: (path: string) => invoke<AudioCppModel[]>("audiocpp_probe", { path }),
  scan: (path: string) => invoke<ScanResult>("audiocpp_scan", { path }),
  setRuntime: (backend: string, device: number, threads: number) =>
    invoke<import("./api").SettingsView>("set_audiocpp_runtime", { backend, device, threads }),
  onStage: (handler: (stage: string, detail: string) => void) =>
    listen<[string, string]>("ac://stage", (e) => handler(e.payload[0], e.payload[1])),
};