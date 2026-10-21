import { useCallback, useEffect, useMemo, useState } from "react";
import {
  readWeights,
  yue2Api,
  useYue2Props,
  YUE2_DEFAULTS,
  type CotMode,
  type StageSampling,
  type Yue2Lm,
  type Yue2Params,
} from "../lib/yue2";
import { formatBytes } from "../lib/engines";
import { queue, server } from "../lib/api";
import { useQueueBusy } from "../lib/queueBusy";
import { Select } from "./Select";

interface Props {
  port: number | null;
  params: Yue2Params;
  setParams: (fn: (p: Yue2Params) => Yue2Params) => void;
  onGenerated: () => void;
}

/** Sampling knobs, with a per-stage "use the checkpoint's value" affordance.
 *
 *  Each field is -1 by default, which the backend omits from the request so the
 *  checkpoint's own setting applies. Showing the number it picked, next to the
 *  box that would override it, is the only way to tell those two apart.
 */
function Stage({
  title,
  defaults,
  value,
  onChange,
}: {
  title: string;
  defaults: StageSampling | null;
  value: StageSampling;
  onChange: (next: StageSampling) => void;
}) {
  const set = (k: keyof StageSampling, v: number) => onChange({ ...value, [k]: v });
  const fields: { key: keyof StageSampling; label: string; step: string; hint?: string }[] = [
    { key: "temperature", label: "temperature", step: "0.05" },
    { key: "topP", label: "top p", step: "0.01" },
    { key: "topK", label: "top k", step: "1" },
    {
      key: "repetitionPenalty",
      label: "repetition penalty",
      step: "0.005",
      hint: "above 1 discourages repeating the plan",
    },
    { key: "penaltyWindow", label: "penalty window", step: "1" },
    { key: "minTokens", label: "min tokens", step: "1" },
    { key: "maxTokens", label: "max tokens", step: "1" },
  ];
  const inherited = (k: keyof StageSampling) =>
    value[k] < 0 && defaults ? defaults[k] : null;

  return (
    <div className="space-y-2">
      <div className="flex items-baseline justify-between">
        <span className="label">{title}</span>
        {defaults && (
          <span className="text-[10px] text-ink-600">showing the checkpoint's values</span>
        )}
      </div>
      <div className="grid gap-2 sm:grid-cols-2 lg:grid-cols-3">
        {fields.map((f) => {
          const own = inherited(f.key);
          return (
            <label key={f.key} className="block">
              <span className="mb-1 block text-[10px] text-ink-500">
                {f.label}
                {own !== null && (
                  <span className="text-ink-600"> · uses {own}</span>
                )}
              </span>
              <input
                type="number"
                step={f.step}
                value={value[f.key]}
                onChange={(e) => set(f.key, Number(e.target.value))}
                title={f.hint}
                className="field"
              />
            </label>
          );
        })}
      </div>
      <p className="text-[10px] text-ink-600">
        -1 leaves the field out of the request entirely.
      </p>
    </div>
  );
}

/** YuE2 generation.
 *
 *  Deliberately not the ACE-Step form. There is no duration here because the
 *  composer decides how long the song is, no BPM or key field because those
 *  belong in the style sentence, and no stems, repaint or lego: this pipeline
 *  has none of those tasks. Reusing the ordinary panel would show controls the
 *  request cannot carry, and every one of them would silently do nothing.
 */
export function Yue2Panel({ port, params, setParams, onGenerated }: Props) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [score, setScore] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  // The installed models, read from disk rather than from the engine: the pick
  // has to be made before a job is submitted, so it cannot wait on a sidecar.
  // Polled slowly for the same reason props are -- a download can finish while
  // the window is open, and the list should catch up without a reopen.
  // Resolved at startup, so it is asked once rather than per render.
  const [mp3Ok, setMp3Ok] = useState(true);
  useEffect(() => {
    void server
      .mp3Available()
      .then(setMp3Ok)
      .catch(() => setMp3Ok(false));
  }, []);

  const [lms, setLms] = useState<Yue2Lm[]>([]);
  const loadLms = useCallback(async () => {
    try {
      setLms(await yue2Api.lms());
    } catch {
      // An unreadable models directory leaves the picker on "engine picks",
      // which is the behaviour before any of this existed.
    }
  }, []);
  useEffect(() => {
    void loadLms();
    const t = setInterval(() => void loadLms(), 15000);
    return () => clearInterval(t);
  }, [loadLms]);

  // A render owned by the runner, not by this panel. Switching tabs unmounts
  // the panel, and a render awaited from here is orphaned with it: nothing left
  // to show progress, and nothing to stop a second render being submitted over
  // the top of it.
  const queueBusy = useQueueBusy();
  const { props, error: propsError } = useYue2Props(port, true, busy || queueBusy);
  const weights = useMemo(() => (props ? readWeights(props) : null), [props]);

  const ready = !!props?.available && !!weights?.lm && !!weights?.vae;

  const canSubmit =
    ready &&
    !busy &&
    !queueBusy &&
    params.style.trim().length > 0 &&
    params.style.trim().length < 2000;

  async function run(planOnly: boolean, semanticOnly = false) {
    if (port === null) return;
    setError(null);
    setNotice(null);

    // A full render goes to the runner, like every other engine. Plan-only and
    // semantic-only stay here: their results are a score and a codec stream,
    // which the queue row has nowhere to carry, and a plan-only job produces no
    // audio at all -- queued, the row would report it as a failure.
    if (!planOnly && !semanticOnly) {
      try {
        // No ACE-Step parameters: a YuE2 job runs off its own. The row still
        // carries an empty placeholder, because the queue struct has held those
        // fields since before YuE2 was queued and reshaping it now would mean
        // migrating every saved queue.
        await queue.add(null, { ...params, planOnly: false, semanticOnly: false });
        setNotice("Added to the queue — progress is in the queue below.");
        setTimeout(() => setNotice(null), 2600);
      } catch (e) {
        setError(String(e));
      }
      return;
    }

    setBusy(true);
    try {
      const outcome = await yue2Api.render(port, {
        ...params,
        planOnly,
        semanticOnly,
      });
      if (outcome.cancelled) {
        setNotice("cancelled");
      } else if (planOnly && outcome.score !== null) {
        setScore(outcome.score);
        setNotice("score ready — edit it below, or paste your own");
      } else if (semanticOnly && outcome.semanticIds !== null) {
        setNotice(`composer produced ${outcome.semanticIds.length} codec ids`);
      } else if (outcome.tracks.length > 0) {
        onGenerated();
        setNotice(
          `Rendered ${outcome.tracks.length} song${outcome.tracks.length === 1 ? "" : "s"}`,
        );
      } else {
        setNotice("nothing came back");
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="space-y-3">
      <section className="panel space-y-3 p-4">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <h3 className="text-sm font-medium text-ink-200">YuE2</h3>
          <span className="text-[10px] text-ink-500">
            AR composer, NAR renderer, own VAE. The composer sets the length.
          </span>
        </div>

        {/* Weights first: everything below is pointless without them, and the
            engine answers a missing file with a generic 400. */}
        {!ready && (
          <div className="rounded-lg bg-amber-500/10 p-3 text-[11px] leading-relaxed text-amber-200 ring-1 ring-amber-500/30">
            {propsError ? (
              <>Could not ask the engine about YuE2: {propsError}</>
            ) : props === null ? (
              <>Waiting for the engine to report what it has…</>
            ) : !props.available ? (
              <>
                This engine build reports no YuE2 weights. They are looked for in
                {" "}
                <code className="font-mono">
                  {props.modelsDir || "the weights folder"}/yue2
                </code>{" "}
                as <code className="font-mono">yue2-lm-*.gguf</code> and{" "}
                <code className="font-mono">yue2-vae-*.gguf</code>.
              </>
            ) : (
              <>
                Found the LM but not the VAE, or the reverse. YuE2 needs both.
                {" "}
                {weights?.lm ? `LM: ${weights.lm.name}. ` : ""}
                {weights?.vae ? `VAE: ${weights.vae.name}.` : ""}
              </>
            )}
            {props?.errors?.length ? (
              <ul className="mt-1 list-disc pl-4 text-amber-300/80">
                {props.errors.slice(0, 4).map((e) => (
                  <li key={e}>{e}</li>
                ))}
              </ul>
            ) : null}
          </div>
        )}

        {ready && weights && (
          <div className="flex flex-wrap gap-x-4 gap-y-1 text-[10px] text-ink-500">
            <span>LM {weights.lm?.name} · {formatBytes(weights.lm?.bytes)}</span>
            <span>VAE {weights.vae?.name} · {formatBytes(weights.vae?.bytes)}</span>
            {props && props.vaeVariantLoaded && (
              <span>decoder {props.vaeVariantLoaded}</span>
            )}
          </div>
        )}

        <div>
          <label className="label" htmlFor="yue-title">
            Title
          </label>
          <input
            id="yue-title"
            type="text"
            value={params.title}
            onChange={(e) => setParams((p) => ({ ...p, title: e.target.value }))}
            placeholder="Neon Rain"
            className="field"
          />
          <p className="mt-1 text-[10px] leading-relaxed text-ink-500">
            Names the file. Leave it empty and the style text is used instead.
          </p>
        </div>

        <div className="grid grid-cols-2 gap-2">
          <Select
            label="Format"
            value={params.outputFormat}
            onChange={(v) => setParams((p) => ({ ...p, outputFormat: v }))}
            options={[
              {
                value: "mp3",
                label: "mp3",
                note: mp3Ok
                  ? "About an eighth of the size. The encoder ships with the app."
                  : "No encoder found on this machine, so renders will stay WAV.",
              },
              { value: "wav", label: "wav", note: "Uncompressed, as the engine returns it." },
            ]}
          />
          <Select
            label="Bitrate"
            value={String(params.mp3Bitrate)}
            onChange={(v) =>
              setParams((p) => ({ ...p, mp3Bitrate: Number(v) || 128 }))
            }
            options={[
              { value: "128", label: "128 kbps", note: "Same as ACE-Step's default." },
              { value: "192", label: "192 kbps", note: "Noticeably better on cymbals and reverb tails." },
              { value: "320", label: "320 kbps", note: "Larger file, diminishing return." },
            ]}
            note={params.outputFormat === "mp3" ? undefined : "Only used for mp3."}
          />
        </div>

        <div>
          <Select
            label="Model"
            value={params.lmType}
            onChange={(v) => setParams((p) => ({ ...p, lmType: v }))}
            options={[
              { value: "", label: "engine picks", note: "Largest model the engine finds first. This is what a render used before this picker existed." },
              ...lms.map((m) => ({
                value: m.lmType,
                label: m.lmType,
                meta: formatBytes(m.sizeBytes),
              })),
            ]}
            note={
              lms.length === 0
                ? "No yue2-lm-*.gguf found in the models directory, so there is nothing to choose between."
                : "Sized smallest first. On a 4 GB card the smallest is the one most likely to finish; the split render already avoids the two-caches-at-once collision."
            }
          />
        </div>

        <div>
          <label className="label" htmlFor="yue-style">
            Style
          </label>
          <textarea
            id="yue-style"
            rows={3}
            value={params.style}
            onChange={(e) => setParams((p) => ({ ...p, style: e.target.value }))}
            placeholder="One warm reggae song in D minor, 158 bpm, laid-back one drop, deep patois vocals"
            className="field resize-y"
          />
          <p className="mt-1 text-[10px] leading-relaxed text-ink-500">
            A plain sentence. Tempo and key go here — this pipeline has no
            separate fields for them, and no duration control: the composer
            decides how long the song is.
          </p>
        </div>

        <div>
          <label className="label" htmlFor="yue-lyrics">
            Lyrics
          </label>
          <textarea
            id="yue-lyrics"
            rows={5}
            value={params.lyrics}
            onChange={(e) => setParams((p) => ({ ...p, lyrics: e.target.value }))}
            placeholder="[Verse]&#10;I walked the long road home tonight"
            className="field resize-y font-mono text-[11px]"
          />
          <p className="mt-1 text-[10px] leading-relaxed text-ink-500">
            Lyric timings come from the score the composer wrote, so each
            line is placed at the bar it is sung over. Put a section marker
            above each block --{" "}
            <code className="mx-1 font-mono">[Verse]</code>{" "}
            <code className="mx-1 font-mono">[Chorus]</code>{" "}
            -- so the score can be matched to it.
          </p>
        </div>

        <div className="grid gap-3 sm:grid-cols-3">
          <div>
            {/* The app's own Select, not a native <select>: WebKitGTK draws
                those with the platform widget style, and `appearance: none`
                only removes the arrow -- the control and its popup stay in the
                desktop's colours however the page is styled. That is why every
                other picker in this app is built from a button and a listbox. */}
            <Select<CotMode>
              label="Chain of thought"
              value={params.cot}
              onChange={(v) => setParams((p) => ({ ...p, cot: v }))}
              options={[
                { value: "full", label: "Full lead sheet" },
                { value: "melody", label: "Melody only" },
                { value: "off", label: "Off (slowest)" },
              ]}
            />
          </div>
          <div>
            <label className="label" htmlFor="yue-steps">
              Renderer steps
            </label>
            <input
              id="yue-steps"
              type="number"
              min="0"
              value={params.odeSteps}
              onChange={(e) => setParams((p) => ({ ...p, odeSteps: Number(e.target.value) }))}
              className="field"
            />
            <p className="mt-1 text-[10px] text-ink-500">0 = checkpoint default</p>
          </div>
          <div>
            <label className="label" htmlFor="yue-cfg">
              Guidance
            </label>
            <input
              id="yue-cfg"
              type="number"
              step="0.1"
              min="0"
              value={params.cfgScale}
              onChange={(e) => setParams((p) => ({ ...p, cfgScale: Number(e.target.value) }))}
              className="field"
            />
            <p className="mt-1 text-[10px] text-ink-500">0 = checkpoint default</p>
          </div>
        </div>

        <div className="grid gap-3 sm:grid-cols-4">
          <div>
            <label className="label" htmlFor="yue-songs">
              Songs
            </label>
            <input
              id="yue-songs"
              type="number"
              min="1"
              max={props?.maxLmBatch ?? 4}
              value={params.lmBatchSize}
              onChange={(e) =>
                setParams((p) => ({ ...p, lmBatchSize: Number(e.target.value) }))
              }
              className="field"
            />
          </div>
          <div>
            <label className="label" htmlFor="yue-variations">
              Variations
            </label>
            <input
              id="yue-variations"
              type="number"
              min="1"
              max={props?.maxSynthBatch ?? 9}
              value={params.synthBatchSize}
              onChange={(e) =>
                setParams((p) => ({ ...p, synthBatchSize: Number(e.target.value) }))
              }
              className="field"
            />
          </div>
          <div>
            <label className="label" htmlFor="yue-seed">
              Seed
            </label>
            <input
              id="yue-seed"
              type="number"
              value={params.seed}
              onChange={(e) => setParams((p) => ({ ...p, seed: Number(e.target.value) }))}
              className="field"
            />
          </div>
          <div>
            <label className="label" htmlFor="yue-retries">
              Runaway retries
            </label>
            <input
              id="yue-retries"
              type="number"
              min="0"
              max="5"
              value={params.semanticRetries}
              onChange={(e) =>
                setParams((p) => ({ ...p, semanticRetries: Number(e.target.value) }))
              }
              className="field"
            />
          </div>
        </div>

        <details className="rounded-lg bg-ink-900/50 p-3">
          <summary className="cursor-pointer text-[11px] text-ink-400">
            Sampler, renderer and adapters
          </summary>
          <div className="mt-3 space-y-4">
            <Stage
              title="Plan stage (lead sheet)"
              defaults={weights?.planDefaults ?? null}
              value={params.plan}
              onChange={(plan) => setParams((p) => ({ ...p, plan }))}
            />
            <Stage
              title="Semantic stage (codec stream)"
              defaults={weights?.semanticDefaults ?? null}
              value={params.semantic}
              onChange={(semantic) => setParams((p) => ({ ...p, semantic }))}
            />

            <div className="grid gap-3 sm:grid-cols-2">
              <div>
                <label className="label" htmlFor="yue-nar-solver">
                  NAR solver
                </label>
                <input
                  id="yue-nar-solver"
                  value={params.narSolver}
                  onChange={(e) =>
                    setParams((p) => ({ ...p, narSolver: e.target.value }))
                  }
                  placeholder="midpoint (built in)"
                  className="field"
                />
              </div>
              <div>
                <label className="label" htmlFor="yue-nar-sched">
                  NAR scheduler
                </label>
                <input
                  id="yue-nar-sched"
                  value={params.narScheduler}
                  onChange={(e) =>
                    setParams((p) => ({ ...p, narScheduler: e.target.value }))
                  }
                  placeholder="uniform (built in)"
                  className="field"
                />
              </div>
            </div>

            <div className="grid gap-3 sm:grid-cols-2">
              <div>
                <label className="label" htmlFor="yue-lm-adapter">
                  Composer adapter
                </label>
                <input
                  id="yue-lm-adapter"
                  value={params.lmAdapter}
                  onChange={(e) =>
                    setParams((p) => ({ ...p, lmAdapter: e.target.value }))
                  }
                  placeholder="none"
                  className="field"
                />
              </div>
              <div>
                <label className="label" htmlFor="yue-nar-adapter">
                  Renderer adapter
                </label>
                <input
                  id="yue-nar-adapter"
                  value={params.narAdapter}
                  onChange={(e) =>
                    setParams((p) => ({ ...p, narAdapter: e.target.value }))
                  }
                  placeholder="none"
                  className="field"
                />
              </div>
            </div>

            <div className="grid gap-3 sm:grid-cols-3">
              <div>
                <label className="label" htmlFor="yue-end">
                  End threshold
                </label>
                <input
                  id="yue-end"
                  type="number"
                  step="0.05"
                  min="0"
                  max="1"
                  value={params.endThreshold}
                  onChange={(e) =>
                    setParams((p) => ({ ...p, endThreshold: Number(e.target.value) }))
                  }
                  className="field"
                />
                <p className="mt-1 text-[10px] text-ink-500">0 = off</p>
              </div>
              <div>
                <label className="label" htmlFor="yue-cache">
                  NAR cache
                </label>
                <input
                  id="yue-cache"
                  type="number"
                  step="0.05"
                  min="0"
                  max="0.9"
                  value={params.narCacheRatio}
                  onChange={(e) =>
                    setParams((p) => ({ ...p, narCacheRatio: Number(e.target.value) }))
                  }
                  className="field"
                />
                <p className="mt-1 text-[10px] text-ink-500">0 = every step for real</p>
              </div>
              <div>
                <label className="label" htmlFor="yue-cutoff">
                  Runaway cutoff
                </label>
                <input
                  id="yue-cutoff"
                  type="number"
                  step="0.1"
                  min="0"
                  value={params.planCutoff}
                  onChange={(e) =>
                    setParams((p) => ({ ...p, planCutoff: Number(e.target.value) }))
                  }
                  className="field"
                />
                <p className="mt-1 text-[10px] text-ink-500">
                  × the plan's own length. 0 = only the hard cap.
                </p>
              </div>
            </div>
          </div>
        </details>

        <details className="rounded-lg bg-ink-900/50 p-3">
          <summary className="cursor-pointer text-[11px] text-ink-400">
            Lead sheet (ABC)
          </summary>
          <div className="mt-3 space-y-2">
            <textarea
              rows={8}
              value={params.abc}
              onChange={(e) => setParams((p) => ({ ...p, abc: e.target.value }))}
              placeholder="Paste an ABC score to compose from, or plan one first."
              className="field resize-y font-mono text-[11px]"
            />
            <p className="text-[10px] leading-relaxed text-ink-500">
              Setting this skips the plan stage entirely: the composer is handed
              the score as written.
            </p>
            {score && (
              <div>
                <div className="mb-1 flex items-center justify-between">
                  <span className="label">Planned score</span>
                  <button
                    onClick={() => setParams((p) => ({ ...p, abc: score }))}
                    className="chip px-2 py-1 text-[10px]"
                  >
                    use this
                  </button>
                </div>
                <textarea
                  rows={8}
                  readOnly
                  value={score}
                  className="field resize-y font-mono text-[11px]"
                />
              </div>
            )}
          </div>
        </details>

        {/* Measured, not guessed: on this engine the composer alone loads about
            2.1 GB and the semantic stage's KV cache asks for another 2.4 GB, so
            a full render needs roughly 4.6 GB. On a 4 GB card the plan stage
            fits and the audio stage does not, and the failure is an allocation
            error deep in the backend rather than anything actionable. Saying so
            here beats letting someone spend a plan on a render that cannot fit. */}
        {ready && weights?.lm && weights.lm.bytes > 3_000_000_000 && (
          <div className="rounded-lg bg-ink-900/60 p-3 text-[11px] leading-relaxed text-ink-400 ring-1 ring-ink-800">
            <strong className="text-ink-300">About the audio stage on a 4 GB
            card.</strong> The composer fits — planning a score works. Rendering
            audio additionally allocates a {weights.lm.bytes > 3_000_000_000 ? "~2.4 GB" : ""}{" "}
            KV cache on top of the loaded composer, which does not. If a full
            render fails with an out-of-VRAM backend error, that is why; the
            score and the render are separate stages and only the second one
            needs the headroom.
          </div>
        )}

        {error && <p className="text-[11px] text-rose-300">{error}</p>}
        {notice && <p className="text-[11px] text-ink-400">{notice}</p>}

        <div className="flex flex-wrap gap-2">
          <button
            onClick={() => void run(false)}
            disabled={!canSubmit}
            className="btn btn-primary"
            title={
              canSubmit
                ? undefined
                : !ready
                  ? "YuE2 weights are not available to the engine"
                  : "Write a style sentence first"
            }
          >
            {busy ? "working…" : "Generate"}
          </button>
          <button
            onClick={() => void run(true)}
            disabled={!ready || busy || !params.style.trim()}
            className="btn btn-ghost"
            title="Write the lead sheet only, then look at it before spending minutes on audio"
          >
            plan the score
          </button>
          <button
            onClick={() => setParams(() => YUE2_DEFAULTS)}
            disabled={busy}
            className="btn btn-ghost"
          >
            reset
          </button>
        </div>
      </section>
    </div>
  );
}