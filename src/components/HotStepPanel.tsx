import { useCallback, useEffect, useState } from "react";
import type { GenerateParams } from "../lib/api";
import { server } from "../lib/api";
import { open } from "@tauri-apps/plugin-dialog";
import { useYue2Props, YUE2_DEFAULTS, type Yue2Params } from "../lib/yue2";
import { Select } from "./Select";
import { Yue2Panel } from "./Yue2Panel";

interface Props {
  port: number | null;
  running: boolean;
  params: GenerateParams;
  setParams: React.Dispatch<React.SetStateAction<GenerateParams>>;
  modelsDir: string;
  onSaveDir: (path: string) => Promise<void>;
  onGenerated: () => void;
  children: React.ReactNode;
}

/** Which pipeline to drive. One binary serves all of them; the weights decide
 *  which pipelines it can actually run, and each has its own request schema. */
type Backend = "acestep" | "yue2";

/** HOT-Step CPP settings.
 *
 *  The engine speaks the same HTTP surface as the bundled acestep.cpp one, so
 *  generation goes through the ordinary panel rather than a second driver. What
 *  is genuinely engine-specific is the folder of weights, and the handful of
 *  request fields the bundled engine has no use for.
 *
 *  Both engines ship a file called `ace-server` and HOT-Step links a different
 *  ggml, so "which folder" and "which engine" have to agree; the weights line
 *  says which is in play rather than leaving it to be inferred from the tab.
 */
export function HotStepPanel({
  port,
  params,
  setParams,
  modelsDir,
  onSaveDir,
  onGenerated,
  children,
}: Props) {
  const [busy, setBusy] = useState(false);
  const [backend, setBackend] = useState<Backend>("acestep");
  const [yue2, setYue2] = useState<Yue2Params>(YUE2_DEFAULTS);
  // Asked once here so the switch can say "no weights" before the form is
  // opened, rather than after someone has filled the style box in.
  const { props: yue2Props } = useYue2Props(port, true);
  const yue2Ready = !!yue2Props?.available;

  /** Solvers this engine will actually run, asked of the engine rather than
   *  hardcoded. HOT-Step resolves the name out of its Lua plugin registry, and
   *  a name it does not have is fatal: it logs "unknown solver", falls back to
   *  `euler`, misses again, and dereferences the null. Its own default is that
   *  same missing name, which is why the list is shown instead of trusted. */
  const [solvers, setSolvers] = useState<string[]>([]);
  useEffect(() => {
    let alive = true;
    if (!port) {
      setSolvers([]);
      return;
    }
    void server
      .solvers(port)
      .then((names) => {
        if (alive) setSolvers(names);
      })
      .catch(() => {
        /* The engine may be mid-restart; the next mount asks again. */
      });
    return () => {
      alive = false;
    };
  }, [port]);

  const browse = useCallback(async () => {
    setBusy(true);
    try {
      const picked = await open({
        directory: true,
        multiple: false,
        title: "Folder of HOT-Step weights",
      });
      if (typeof picked === "string") await onSaveDir(picked);
    } finally {
      setBusy(false);
    }
  }, [onSaveDir]);

  const set = useCallback(
    <K extends keyof GenerateParams>(key: K, value: GenerateParams[K]) =>
      setParams((p) => ({ ...p, [key]: value })),
    [setParams],
  );

  return (
    <div className="space-y-3">
      <section className="panel space-y-3 p-4">
        <div className="flex gap-1.5" role="tablist" aria-label="HOT-Step backend">
          {(
            [
              ["acestep", "ACE-Step 1.5", true],
              ["yue2", yue2Ready ? "YuE2" : "YuE2 (no weights)", yue2Ready],
            ] as const
          ).map(([id, label, ok]) => (
            <button
              key={id}
              role="tab"
              aria-selected={backend === id}
              onClick={() => setBackend(id)}
              className={`chip px-3 py-1.5 text-[11px] ${
                backend === id
                  ? "bg-glow-500/15 text-glow-300 ring-1 ring-glow-500/40"
                  : ""
              } ${ok ? "" : "opacity-50"}`}
            >
              {label}
            </button>
          ))}
        </div>

        <div className="flex flex-wrap items-center justify-between gap-2">
          <h3 className="text-sm font-medium text-ink-200">HOT-Step CPP</h3>
          <span className="text-[10px] text-ink-500">
            A further-developed acestep.cpp. Same request surface, more of it.
          </span>
        </div>

        <div>
          <label className="label" htmlFor="hotstep-models">
            Weights folder
          </label>
          <div className="flex gap-2">
            <input
              id="hotstep-models"
              readOnly
              value={modelsDir}
              placeholder="uses the ACE-Step folder"
              className="field flex-1"
            />
            <button
              onClick={() => void browse()}
              disabled={busy}
              className="btn btn-ghost px-3 py-1.5 text-[11px]"
            >
              {busy ? "…" : "browse…"}
            </button>
          </div>
          <p className="mt-1 text-[10px] leading-relaxed text-ink-500">
            This is the models <em>root</em>, not a family subfolder. The engine
            scans only the top level and skips weights that belong to another
            backend, so pointing this at{" "}
            <code className="font-mono">yue2/</code> or{" "}
            <code className="font-mono">mm3/</code> leaves it with no ACE-Step
            models and every submit refused. YuE2 finds its own weights in the{" "}
            <code className="font-mono">yue2/</code> subfolder automatically, and
            MiniMax in <code className="font-mono">mm3/</code>. Leave it empty to
            share the ACE-Step folder.
          </p>
        </div>

        <details className="rounded-lg bg-ink-900/50 p-3">
          <summary className="cursor-pointer text-[11px] text-ink-400">
            HOT-Step only parameters
          </summary>

          <div className="mt-3 space-y-3">
            <div>
              <label className="label" htmlFor="hs-negative">
                Negative prompt
              </label>
              <input
                id="hs-negative"
                value={params.negativePrompt}
                onChange={(e) => set("negativePrompt", e.target.value)}
                placeholder="encoded as the unconditional branch"
                className="field"
              />
              <p className="mt-1 text-[10px] text-ink-500">
                How to steer away from a sound, rather than describing what you
                want.
              </p>
            </div>

            <div className="grid gap-3 sm:grid-cols-2">
              <div>
                <label className="label" htmlFor="hs-vae">
                  Decoder
                </label>
                <input
                  id="hs-vae"
                  value={params.vae}
                  onChange={(e) => set("vae", e.target.value)}
                  placeholder="keep the loaded VAE"
                  className="field"
                />
              </div>
              <div className="flex items-end">
                <label className="flex items-center gap-2 pb-2 text-[11px] text-ink-300">
                  <input
                    type="checkbox"
                    checked={params.ppVaeReencode}
                    onChange={(e) => set("ppVaeReencode", e.target.checked)}
                  />
                  second VAE pass (takes the fizz off)
                </label>
              </div>
            </div>

            <div className="grid gap-3 sm:grid-cols-2">
              <div>
                <label className="label" htmlFor="hs-lm-adapter">
                  Planner adapter
                </label>
                <input
                  id="hs-lm-adapter"
                  value={params.lmAdapter}
                  onChange={(e) => set("lmAdapter", e.target.value)}
                  placeholder="none"
                  className="field"
                />
                <p className="mt-1 text-[10px] text-ink-500">
                  A separate slot from the DiT adapter above.
                </p>
              </div>
              <div>
                <label className="label" htmlFor="hs-postprocess">
                  Postprocess plugin
                </label>
                <input
                  id="hs-postprocess"
                  value={params.postprocessPlugin}
                  onChange={(e) => set("postprocessPlugin", e.target.value)}
                  placeholder="none"
                  className="field"
                />
              </div>
            </div>

            <div className="grid gap-3 sm:grid-cols-2">
              <div>
                <label className="label" htmlFor="hs-cfg-cutoff">
                  CFG cutoff
                </label>
                <input
                  id="hs-cfg-cutoff"
                  type="number"
                  step="0.05"
                  min="0"
                  max="1"
                  value={params.lmCfgCutoffRatio}
                  onChange={(e) => set("lmCfgCutoffRatio", Number(e.target.value))}
                  className="field"
                />
                <p className="mt-1 text-[10px] text-ink-500">
                  Fraction of the sequence guidance applies to. 1 is all of it.
                </p>
              </div>
              <div>
                <label className="label" htmlFor="hs-mp3">
                  MP3 bitrate (kbps)
                </label>
                <input
                  id="hs-mp3"
                  type="number"
                  step="16"
                  value={params.mp3Bitrate}
                  onChange={(e) => set("mp3Bitrate", Number(e.target.value))}
                  className="field"
                />
              </div>
            </div>

            <div className="grid gap-3 sm:grid-cols-2">
              {solvers.length > 0 && (
                <div>
                  <Select
                    label="Solver"
                    value={params.solver}
                    onChange={(v) => set("solver", v)}
                    options={[
                      { value: "", label: "engine default" },
                      ...solvers.map((s) => ({ value: s, label: s })),
                    ]}
                    note={
                      params.solver
                        ? undefined
                        : "Empty asks the engine, which is resolved against this list when the job is submitted."
                    }
                  />
                </div>
              )}

              <div>
                <label className="label" htmlFor="hs-timesteps">
                  Custom timesteps
                </label>
                <input
                  id="hs-timesteps"
                  value={params.customTimesteps}
                  onChange={(e) => set("customTimesteps", e.target.value)}
                  placeholder="solver default"
                  className="field"
                />
              </div>
            </div>
          </div>
        </details>
      </section>

      {backend === "yue2" ? (
        <Yue2Panel
          port={port}
          params={yue2}
          setParams={setYue2}
          onGenerated={onGenerated}
        />
      ) : (
        children
      )}
    </div>
  );
}