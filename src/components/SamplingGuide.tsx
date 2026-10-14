import type { SamplingPreset } from "../lib/api";

/**
 * Plain-language explanation of every sampling control.
 *
 * Kept as data rather than prose in the JSX so the guide and the form stay in
 * one place, and so a control added to the form has to be described here too.
 */
export interface SamplingDoc {
  id: string;
  name: string;
  blurb: string;
  detail: string;
}

export const SAMPLING_DOCS: SamplingDoc[] = [
  {
    id: "inference_steps",
    name: "Steps",
    blurb: "How many denoising passes the DiT takes.",
    detail:
      "Each step is one more refinement of the waveform, and cost scales " +
      "linearly. Turbo is guidance-distilled for 8 and gets no better with " +
      "more. Base and SFT want roughly 50. Between 8 and 50 you are trading " +
      "minutes for a result that is not proportionally better.",
  },
  {
    id: "guidance_scale",
    name: "Guidance",
    blurb: "How literally the caption is followed. 1.0 is off.",
    detail:
      "Guidance runs the model twice per step, once with and once without the " +
      "text, and pushes the difference. Higher values track the prompt more " +
      "literally and sound more rigid; the extract route uses 7.0. Turbo " +
      "ignores anything above 1.0 outright, because it was distilled with " +
      "guidance off.",
  },
  {
    id: "shift",
    name: "Shift",
    blurb: "How the timesteps are spread across the denoising run.",
    detail:
      "Flow matching samples from a continuous noise scale, and shift decides " +
      "where the schedule spends its steps. Low shift front-loads detail, high " +
      "shift favours large-scale structure. Turbo is trained at 3.0 and base " +
      "and SFT at 1.0; these are not interchangeable defaults.",
  },
  {
    id: "solver",
    name: "Solver",
    blurb: "The numerical method used to walk the schedule.",
    detail:
      "Euler is first order: one evaluation per step, predictable, and what " +
      "everything else is measured against. DPM++ 3M is third order and " +
      "reaches the same endpoint in fewer evaluations. SDE Ancestral re-noises " +
      "at each step for extra variety at the cost of reproducibility.",
  },
  {
    id: "synth_batch_size",
    name: "Variations",
    blurb: "How many DiT passes to render from one lyric.",
    detail:
      "Each variation gets a consecutive seed, so they diverge from each " +
      "other rather than resampling the same noise. Cost multiplies directly: " +
      "four variations is four times the render.",
  },
  {
    id: "seed",
    name: "Seed",
    blurb: "Fixes the noise. -1 picks a fresh one every run.",
    detail:
      "The same seed, prompt and settings reproduce a track byte for byte. " +
      "Note that the planner is always sampled independently, so locking the " +
      "seed does not freeze the lyrics — only the audio.",
  },
  {
    id: "duration",
    name: "Duration",
    blurb: "Target length in seconds. 0 lets the planner choose.",
    detail:
      "This is a target, not a guarantee. The planner is constrained to " +
      "10–600 seconds and can pick its own within that, and the DiT will " +
      "sometimes overshoot what was asked for.",
  },
  {
    id: "vocal_language",
    name: "Vocal language",
    blurb: "Which language the sung lyrics should be in.",
    detail:
      "Leaving this on auto makes the planner guess from the caption, and it " +
      "guesses badly — English prompts have come back tagged Haitian Creole " +
      "and Japanese. It is pinned to English by default for that reason. " +
      "\"No specific language\" is a distinct signal to the model, not the " +
      "same as auto.",
  },
];

export function SamplingGuide({
  onClose,
}: {
  onClose: () => void;
}) {
  return (
    <div className="panel bg-ink-900/95 p-5">
      <div className="flex items-start justify-between gap-4">
        <div>
          <h3 className="text-sm font-semibold text-ink-100">
            What the sampling controls actually do
          </h3>
          <p className="mt-1 text-xs text-ink-500">
            Defaults are tuned for a 4 GB card. Numbers that the model rejects
            are ignored rather than clamped, so check the preset you picked
            matches your DiT.
          </p>
        </div>
        <button
          onClick={onClose}
          className="chip bg-ink-800 text-ink-300 ring-1 ring-ink-700 hover:bg-ink-750"
        >
          close
        </button>
      </div>

      <dl className="mt-4 space-y-3">
        {SAMPLING_DOCS.map((doc) => (
          <div key={doc.id} className="border-l-2 border-ink-700 pl-3">
            <dt className="text-xs font-medium text-ink-200">{doc.name}</dt>
            <dd className="mt-0.5 text-xs text-ink-400">{doc.blurb}</dd>
            <dd className="mt-1 text-[11px] leading-relaxed text-ink-500">
              {doc.detail}
            </dd>
          </div>
        ))}
      </dl>
    </div>
  );
}

export function samplingSummary(
  values: {
    inferenceSteps: number;
    guidanceScale: number;
    shift: number;
    solver: string;
    synthBatchSize: number;
  },
  presetLabel?: string,
): string {
  const steps = values.inferenceSteps || "auto";
  const guide = values.guidanceScale === 0 ? "auto" : values.guidanceScale;
  const shift = values.shift === 0 ? "auto" : values.shift;
  const parts = [
    `${steps} steps`,
    `cfg ${guide}`,
    `shift ${shift}`,
    values.solver,
  ];
  if (values.synthBatchSize > 1) {
    parts.push(`x${values.synthBatchSize}`);
  }
  return (presetLabel ? `${presetLabel} · ` : "") + parts.join(" · ");
}

export function presetTitle(preset: SamplingPreset): string {
  return `${preset.description}\n\nFor: ${preset.intendedFor} · ${preset.approxSeconds}`;
}