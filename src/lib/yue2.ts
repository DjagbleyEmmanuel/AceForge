import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { Track } from "./api";

/** How much lead sheet the composer writes before the audio. The engine's own
 *  enum names: `full`, `melody`, `off`. */
export type CotMode = "full" | "melody" | "off";

/** One stage's sampler settings. -1 on every field means "leave the
 *  checkpoint's own value", which is why the defaults are all -1 rather than
 *  the numbers `/yue2/props` reports: those are what it picked, not what a
 *  request should impose. */
export interface StageSampling {
  temperature: number;
  topP: number;
  topK: number;
  repetitionPenalty: number;
  penaltyWindow: number;
  minTokens: number;
  maxTokens: number;
}

export interface Yue2Params {
  /** A plain style sentence. Tempo and key go in this text: there are no
   *  separate fields for them on this pipeline. */
  style: string;
  lyrics: string;
  cot: CotMode;
  /** An ABC lead sheet. Setting it skips the plan stage entirely. */
  abc: string;
  planOnly: boolean;
  semanticOnly: boolean;
  plan: StageSampling;
  semantic: StageSampling;
  cfgScale: number;
  odeSteps: number;
  narSolver: string;
  narScheduler: string;
  lmBatchSize: number;
  synthBatchSize: number;
  seed: number;
  noiseSeed: number;
  semanticRetries: number;
  planCutoff: number;
  endThreshold: number;
  narCacheRatio: number;
  narChunkFrames: number;
  lmAdapter: string;
  narAdapter: string;
}

const unsetStage = (): StageSampling => ({
  temperature: -1,
  topP: -1,
  topK: -1,
  repetitionPenalty: -1,
  penaltyWindow: -1,
  minTokens: -1,
  maxTokens: -1,
});

export const YUE2_DEFAULTS: Yue2Params = {
  style: "",
  lyrics: "",
  cot: "full",
  abc: "",
  planOnly: false,
  semanticOnly: false,
  plan: unsetStage(),
  semantic: unsetStage(),
  cfgScale: 0,
  odeSteps: 0,
  narSolver: "",
  narScheduler: "",
  lmBatchSize: 1,
  synthBatchSize: 1,
  seed: -1,
  noiseSeed: -1,
  semanticRetries: 0,
  planCutoff: 1.5,
  endThreshold: 0,
  narCacheRatio: 0,
  narChunkFrames: 0,
  lmAdapter: "",
  narAdapter: "",
};

/** The engine's own report, forwarded largely as received. `raw` carries the
 *  file list and the checkpoint's sampling defaults, which the form uses to show
 *  what will actually happen rather than only what can be typed. */
export interface Yue2Props {
  available: boolean;
  synthReady: boolean;
  modelsDir: string;
  maxLmBatch: number;
  maxSynthBatch: number;
  vaeVariantLoaded: string;
  errors: string[];
  raw: unknown;
}

export interface Yue2Outcome {
  tracks: Track[];
  score: string | null;
  semanticIds: number[] | null;
  cancelled: boolean;
}

export const yue2Api = {
  props: (port?: number) =>
    invoke<Yue2Props>("yue2_props", { port: port ?? null }),
  render: (port: number | null, params: Yue2Params) =>
    invoke<Yue2Outcome>("yue2_render", { port, params }),
};

/** Read a value out of the forwarded document without asserting a shape.
 *
 *  `/yue2/props` grows new keys as the engine does, and a wrong guess here
 *  would throw during render and blank the window. */
function pick(raw: unknown, ...path: (string | number)[]): unknown {
  let node: unknown = raw;
  for (const key of path) {
    if (node === null || typeof node !== "object") return undefined;
    node = (node as Record<string | number, unknown>)[key];
  }
  return node;
}

export interface Yue2Weights {
  lm: { name: string; bytes: number } | null;
  vae: { name: string; bytes: number } | null;
  /** Sampling defaults the checkpoint asks for, per stage. */
  planDefaults: StageSampling | null;
  semanticDefaults: StageSampling | null;
}

export function readWeights(props: Yue2Props): Yue2Weights {
  const file = (key: string) => {
    const found = pick(props.raw, "files", key, "found") === true;
    const name = pick(props.raw, "files", key, "name");
    const bytes = pick(props.raw, "files", key, "bytes");
    if (!found || typeof name !== "string" || name === "") return null;
    return { name, bytes: typeof bytes === "number" ? bytes : 0 };
  };
  const stage = (key: string): StageSampling | null => {
    const s = pick(props.raw, "sampling", key);
    if (s === null || typeof s !== "object") return null;
    const src = s as Record<string, unknown>;
    const num = (k: string) => (typeof src[k] === "number" ? (src[k] as number) : -1);
    return {
      temperature: num("temperature"),
      topP: num("top_p"),
      topK: num("top_k"),
      repetitionPenalty: num("repetition_penalty"),
      penaltyWindow: num("penalty_window"),
      minTokens: num("min_tokens"),
      maxTokens: num("max_tokens"),
    };
  };
  return {
    lm: file("lm"),
    vae: file("vae_standard") ?? file("vae_legacy"),
    planDefaults: stage("plan"),
    semanticDefaults: stage("semantic"),
  };
}

/** Poll props while the panel is open.
 *
 *  The engine only knows what it has once it has started, and the weights can
 *  appear while the window is already up, so this is a poll rather than a
 *  one-shot read. Slow, because nothing changes often.
 */
export function useYue2Props(
  port: number | null,
  engineIsHotStep: boolean,
  /** While a render is running the engine's worker is saturated, so this poll
   *  queues behind it and answers only when the render is over -- several
   *  times over, one every eight seconds, none of it useful. Props do not
   *  change during a render, so the poll simply pauses and resumes after. */
  paused = false,
) {
  const [props, setProps] = useState<Yue2Props | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    if (port === null || !engineIsHotStep) {
      setProps(null);
      return;
    }
    try {
      setProps(await yue2Api.props(port));
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }, [port, engineIsHotStep]);

  useEffect(() => {
    void load();
    if (port === null || !engineIsHotStep || paused) return;
    const id = window.setInterval(() => void load(), 8000);
    return () => window.clearInterval(id);
  }, [load, port, engineIsHotStep, paused]);

  return { props, error, reload: load };
}