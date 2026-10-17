/** Sorting the backend log into kinds the user can pick between.
 *
 *  The backend tags nearly everything `[Like-This]`, so the tag is the cheap
 *  signal and the message text is the fallback. Buckets are ordered by how much
 *  a user would want them: a failure outranks everything, because it is the one
 *  line in a thousand that explains why a render died.
 *
 *  Every tag here was read off the real `ace-server.stderr.log` rather than
 *  guessed, so an unfamiliar line falls through to "other" instead of being
 *  filed under a bucket it does not belong to.
 */

export type LogCategory =
  | "error"
  | "warn"
  | "memory"
  | "backend"
  | "audio"
  | "models"
  | "generate"
  | "other";

/** Shown in the picker, in this order. `other` is deliberately absent: it is the
 *  bucket you get when nothing matched, not something to select. */
export const CATEGORIES: { id: LogCategory; label: string }[] = [
  { id: "error", label: "errors" },
  { id: "warn", label: "warnings" },
  { id: "generate", label: "generation" },
  { id: "models", label: "models" },
  { id: "memory", label: "memory" },
  { id: "backend", label: "backend" },
  { id: "audio", label: "audio" },
];

const ERROR = /\b(FATAL|ERROR)\b|segmentation fault|unknown solver|cannot allocate|out of memory/i;
const WARN = /\bWARN(ING)?\b/i;

const AUDIO = /^\[(MP3|WAV|Audio|Decode|Encode|Crop|Normalis|Normaliz|Loud)/i;
const MEMORY = /^\[(VRAM|DIAG)\b|Unload|out of memory|cudaMalloc|\bfree\s+\d+\s*MB/i;
/** The GPU/runtime chatter, which is mostly untagged: ggml and CUDA announce
 *  themselves in prose. Worth its own bucket on a card this small, because
 *  "disabling CUDA graphs due to GPU architecture" is a diagnosis and not noise. */
const BACKEND =
  /^(load_backend:|ggml_|ggml_cuda|Device \d+:)|CUDA|compute capability|\bsm_\d+|VMM:/i;
const MODELS = /^\[(Registry|GGUF|Load|Store|WeightCtx|Qwen|BPE|LM-Config|Synth-Load|Token)/i;
const GENERATE =
  /^\[(DiT|LM|Ace-LM|CondEnc|Context|Init-Noise|FSM|Resolve-|Align-|LRC-|VAE|Server|Prop|Prompt)/i;

/** The `[Tag]` a line opens with, or an empty string when it has none. */
export function tagOf(line: string): string {
  const m = /^\[([^\]]{1,24})\]/.exec(line);
  return m ? m[1] : "";
}

/**
 * Which bucket a line belongs to. First match wins, so the order below is the
 * precedence: a `[DiT] ERROR:` line is a failure, not a generation line.
 */
export function categorise(line: string): LogCategory {
  if (ERROR.test(line)) return "error";
  if (WARN.test(line)) return "warn";
  if (MEMORY.test(line)) return "memory";
  if (BACKEND.test(line)) return "backend";
  if (AUDIO.test(line)) return "audio";
  if (MODELS.test(line)) return "models";
  if (GENERATE.test(line)) return "generate";
  return "other";
}