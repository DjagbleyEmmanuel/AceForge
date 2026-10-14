import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  audiocpp,
  type AudioCppModel,
  type ModelOption,
  type Rendered,
  type ScanResult,
} from "../lib/audiocpp";
import { formatBytes } from "../lib/types";

const BACKENDS = ["cpu", "cuda", "vulkan", "metal", "hip"] as const;
const COT_MODES = ["off", "melody", "full"] as const;
const STOP_AFTER = ["audio", "semantic", "abc"] as const;

interface Props {
  modelDir: string;
  busy: boolean;
  onSaveDir: (path: string) => Promise<void>;
  onRendered: (path: string) => void;
}

function Tag({ children }: { children: React.ReactNode }) {
  return (
    <span className="chip bg-ink-800 text-ink-400 ring-1 ring-ink-700">
      {children}
    </span>
  );
}

/** "Yue2 3B · Text-to-music · Q8_0", degrading gracefully per part. */
function describe(o: ModelOption): string {
  const quant = o.quant ? o.quant.toUpperCase().replace("_", "-") : "";
  return [o.label, o.taskLabel, quant].filter(Boolean).join("  ·  ");
}

/** Studio panel for audio.cpp / YuE2.
 *
 *  The parameter names are YuE2's own, not ACE-Step's: YuE2 calls a prompt
 *  "style", has no BPM or key field, and takes lyrics directly rather than
 *  writing them. Reusing the ACE-Step form here would send parameters the
 *  model ignores. Everything sent from here is declared in audio.cpp's
 *  `yue2_cli_interface()`; it has no length or tags control, so neither is
 *  offered rather than faked.
 */
export function AudioCppPanel({
  modelDir,
  busy: outerBusy,
  onSaveDir,
  onRendered,
}: Props) {
  const [scan, setScan] = useState<ScanResult | null>(null);
  const [chosen, setChosen] = useState<string>("");
  const [loaded, setLoaded] = useState<string[]>([]);
  const [status, setStatus] = useState<{ running: boolean; note: string } | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const [description, setDescription] = useState(
    "dreamy shoegaze, reverb-drenched guitars, hazy vocals",
  );
  const [lyrics, setLyrics] = useState("");
  const [instrumental, setInstrumental] = useState(false);
  const [seed, setSeed] = useState<string>("");
  const [guidance, setGuidance] = useState("1.0");
  const [steps, setSteps] = useState("8");
  const [cot, setCot] = useState("off");
  const [stopAfter, setStopAfter] = useState("audio");

  const [useGpu, setUseGpu] = useState(false);
  const [device, setDevice] = useState(0);
  const [threads, setThreads] = useState(0);
  const [result, setResult] = useState<Rendered | null>(null);

  // Which directory the current scan describes. Without this the auto-start
  // effect would fire again every render and restart the runtime underneath a
  // job in flight.
  const scannedDir = useRef<string>("");

  const options = scan?.options ?? [];
  const selected = useMemo(
    () => options.find((o) => o.entry.id === chosen) ?? null,
    [options, chosen],
  );

  useEffect(() => {
    // Guarded: a throw in an effect tears down this subtree and leaves an
    // empty panel, which is indistinguishable from a layout bug.
    try {
      void audiocpp
        .onStage((_s, detail) => setStatus({ running: true, note: detail }))
        .catch(() => {});
    } catch {
      // Progress reporting is optional; the panel still works without it.
    }
  }, []);

  const pushRuntime = useCallback(async (gpu: boolean, dev: number, thr: number) => {
    const backend = gpu ? "cuda" : "cpu";
    try {
      await audiocpp.setRuntime(backend, dev, thr);
      return backend;
    } catch (e) {
      setError(String(e));
      return backend;
    }
  }, []);

  /** Point the runtime at exactly `entries` and start it. */
  const startWith = useCallback(
    async (entries: AudioCppModel[]) => {
      if (entries.length === 0) {
        setError("No model to load. Assign a folder that holds one.");
        return;
      }
      setBusy(true);
      setError(null);
      try {
        await pushRuntime(useGpu, device, threads);
        const status = await audiocpp.start(entries);
        const live = await audiocpp.models();
        setLoaded(live.map((m) => m.id));
        setStatus({
          running: status.running,
          note: `${live.length} model${live.length === 1 ? "" : "s"} ready`,
        });
      } catch (e) {
        setError(String(e));
        setLoaded([]);
        setStatus({ running: false, note: "not running" });
      } finally {
        setBusy(false);
      }
    },
    [pushRuntime, useGpu, device, threads],
  );

  /** Assigning a folder is all it should take: scan it, wire up every model. */
  const assign = useCallback(
    async (dir: string) => {
      if (!dir.trim()) {
        setScan(null);
        setLoaded([]);
        return;
      }
      setBusy(true);
      setError(null);
      try {
        const found = await audiocpp.scan(dir.trim());
        setScan(found);
        scannedDir.current = dir.trim();

        if (found.options.length === 0) {
          setLoaded([]);
          setStatus({ running: false, note: "nothing found" });
          return;
        }
        // Every family found, so a directory holding several is usable without
        // the user hand-picking. audio.cpp keeps one resident at a time
        // (max_loaded_models), so listing them all costs no memory.
        await startWith(found.options.map((o) => o.entry));
        setChosen((current) =>
          found.options.some((o) => o.entry.id === current)
            ? current
            : found.options[0].entry.id,
        );
      } catch (e) {
        setError(String(e));
      } finally {
        setBusy(false);
      }
    },
    [startWith],
  );

  // Adopt whatever folder is already configured, and re-adopt when it changes.
  useEffect(() => {
    const dir = modelDir.trim();
    if (!dir || scannedDir.current === dir) return;
    void assign(dir);
  }, [modelDir, assign]);

  async function stop() {
    await audiocpp.stop();
    setLoaded([]);
    setStatus({ running: false, note: "stopped" });
  }

  async function render() {
    setError(null);
    setResult(null);
    if (!chosen) {
      setError("Pick a model first.");
      return;
    }
    setBusy(true);
    setStatus({
      running: true,
      note:
        stopAfter === "audio"
          ? "rendering — this can take many minutes on CPU"
          : `rendering, stopping after ${stopAfter}`,
    });
    try {
      const num = (v: string) => (v.trim() === "" ? null : Number(v));
      const out = await audiocpp.render(chosen, {
        style: description,
        // An instrumental is an empty lyric field to YuE2; sending the text
        // with a flag set would just sing it.
        lyrics: instrumental ? "" : lyrics,
        seed: num(seed),
        guidanceScale: num(guidance),
        numInferenceSteps: num(steps),
        cot,
        stopAfter,
      });
      setResult(out);
      onRendered(out.path);
      setStatus({ running: false, note: "done" });
    } catch (e) {
      setError(String(e));
      setStatus({ running: false, note: "failed" });
    } finally {
      setBusy(false);
    }
  }

  const live = loaded.length > 0;
  const isLoaded = (id: string) => loaded.includes(id);

  return (
    <div className="space-y-4">
      <section className="panel space-y-3 p-4">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <h3 className="text-sm font-medium">audio.cpp</h3>
          {live ? (
            <button onClick={stop} disabled={busy} className="btn btn-ghost px-3 py-1.5 text-xs">
              Unload all
            </button>
          ) : null}
        </div>

        <div>
          <label className="label" htmlFor="ac-dir">
            Models folder
          </label>
          <div className="mt-1.5 flex gap-2">
            <input
              id="ac-dir"
              defaultValue={modelDir}
              spellCheck={false}
              placeholder="/path/to/a-family-directory"
              onBlur={(e) => {
                if (e.target.value.trim() !== modelDir) {
                  void onSaveDir(e.target.value.trim()).then(() =>
                    assign(e.target.value.trim()),
                  );
                }
              }}
              className="field min-w-0 flex-1"
            />
            <button
              onClick={() => void onSaveDir(modelDir).then(() => assign(modelDir))}
              disabled={outerBusy || !modelDir}
              className="btn btn-ghost px-3 py-1.5 text-xs"
            >
              {busy ? "Scanning…" : "Apply"}
            </button>
          </div>
          <p className="mt-1 text-[10px] text-ink-500">
            Separate from ACE-Step: audio.cpp reads one fused GGUF per family
            plus sidecar configs, not four separate files.
          </p>
        </div>

        {options.length > 0 && (
          <div className="space-y-2">
            <label className="label" htmlFor="audiocpp-model">
              Model
            </label>
            <select
              id="audiocpp-model"
              value={chosen}
              onChange={(e) => setChosen(e.target.value)}
              className="w-full rounded-lg border border-ink-700 bg-ink-850 px-3 py-2 text-sm text-ink-100 outline-none transition focus:border-glow-500/60 focus:ring-1 focus:ring-glow-500/40"
            >
              {options.map((o) => (
                <option key={o.entry.id} value={o.entry.id}>
                  {describe(o)}
                  {isLoaded(o.entry.id) ? "" : "  (not loaded)"}
                </option>
              ))}
            </select>

            {selected && (
              <div className="flex flex-wrap items-center gap-1.5">
                <Tag>{formatBytes(selected.sizeBytes)}</Tag>
                {selected.quant && <Tag>{selected.quant.toUpperCase()}</Tag>}
                {selected.hasVae ? (
                  <Tag>VAE present</Tag>
                ) : (
                  <span className="chip bg-rose-500/15 text-rose-300 ring-1 ring-rose-500/30">
                    no VAE
                  </span>
                )}
                {selected.hasSidecars ? (
                  <Tag>sidecars present</Tag>
                ) : (
                  <span className="chip bg-rose-500/15 text-rose-300 ring-1 ring-rose-500/30">
                    no sidecars
                  </span>
                )}
                <Tag>{loaded.length} resident</Tag>
              </div>
            )}

            <p className="text-[10px] leading-relaxed text-ink-500">
              {live
                ? isLoaded(chosen)
                  ? "Ready. Only the model you pick is loaded; audio.cpp keeps one resident at a time."
                  : `${chosen} is not loaded. Press Load to bring just that one up.`
                : "Nothing is loaded. Assigning a folder starts the runtime with everything it finds."}
            </p>

            <button
              onClick={() =>
                selected && void startWith([selected.entry])
              }
              disabled={busy || !selected || isLoaded(chosen)}
              className="btn btn-ghost px-3 py-1.5 text-xs"
            >
              {isLoaded(chosen) ? "Loaded" : "Load this model"}
            </button>
          </div>
        )}

        {scan && scan.warnings.length > 0 && (
          <ul className="space-y-1 rounded-lg bg-amber-400/10 px-3 py-2 text-[11px] text-amber-200 ring-1 ring-amber-400/25">
            {scan.warnings.map((w) => (
              <li key={w}>{w}</li>
            ))}
          </ul>
        )}

        <div className="space-y-2">
          <label className="label">Load model into</label>
          <div className="grid grid-cols-2 gap-2">
            <button
              onClick={() => {
                setUseGpu(true);
                void pushRuntime(true, device, threads).then(() => {
                  if (options.length > 0) void startWith(options.map((o) => o.entry));
                });
              }}
              disabled={busy}
              className={`rounded-lg border px-3 py-2 text-xs transition ${
                useGpu
                  ? "border-glow-500/60 bg-glow-500/10 text-glow-200"
                  : "border-ink-700 bg-ink-850 text-ink-300 hover:border-ink-600"
              }`}
            >
              GPU when available
            </button>
            <button
              onClick={() => {
                setUseGpu(false);
                void pushRuntime(false, device, threads).then(() => {
                  if (options.length > 0) void startWith(options.map((o) => o.entry));
                });
              }}
              disabled={busy}
              className={`rounded-lg border px-3 py-2 text-xs transition ${
                !useGpu
                  ? "border-glow-500/60 bg-glow-500/10 text-glow-200"
                  : "border-ink-700 bg-ink-850 text-ink-300 hover:border-ink-600"
              }`}
            >
              CPU RAM
            </button>
          </div>
          <p className="text-[10px] leading-relaxed text-ink-500">
            CPU is the default on purpose. A 3B model wants about 3.97 GiB and
            this card has 4031 MiB, so a failed GPU allocation is worse than a
            slow render. Try GPU first if you have a Q4_0 quant.
          </p>
        </div>

        <details className="space-y-2">
          <summary className="cursor-pointer text-[11px] text-ink-400">
            Runtime flags
          </summary>
          <div className="grid grid-cols-3 gap-3 pt-1">
            <div>
              <label className="label" htmlFor="ac-backend">
                Backend
              </label>
              <select
                id="ac-backend"
                value={useGpu ? "cuda" : "cpu"}
                onChange={(e) => {
                  const gpu = e.target.value === "cuda";
                  setUseGpu(gpu);
                  void pushRuntime(gpu, device, threads);
                }}
                className="field mt-1.5"
              >
                {BACKENDS.map((b) => (
                  <option key={b} value={b}>
                    {b}
                  </option>
                ))}
              </select>
            </div>
            <div>
              <label className="label" htmlFor="ac-device">
                Device
              </label>
              <input
                id="ac-device"
                type="number"
                min={0}
                value={device}
                onChange={(e) => {
                  const d = Math.max(0, Number(e.target.value));
                  setDevice(d);
                  void pushRuntime(useGpu, d, threads);
                }}
                className="field mt-1.5"
              />
            </div>
            <div>
              <label className="label" htmlFor="ac-threads">
                Threads
              </label>
              <input
                id="ac-threads"
                type="number"
                min={0}
                value={threads}
                onChange={(e) => {
                  const t = Math.max(0, Number(e.target.value));
                  setThreads(t);
                  void pushRuntime(useGpu, device, t);
                }}
                className="field mt-1.5"
              />
            </div>
          </div>
          <p className="text-[10px] leading-relaxed text-ink-500">
            Threads 0 leaves the choice to audio.cpp. On a 4-core box, 4 is
            usually faster than letting it guess. Changes apply on the next
            start. audio.cpp has no llama.cpp-style layer offload, so there is
            no partial-offload slider to expose.
          </p>
        </details>

        {status && <p className="text-xs text-ink-400">{status.note}</p>}

        {error && (
          <p className="rounded-lg bg-rose-500/10 px-3 py-2 text-xs text-rose-300 ring-1 ring-rose-500/25">
            {error}
          </p>
        )}
      </section>

      <section className="panel space-y-3 p-4">
        <h3 className="text-sm font-medium">Create music</h3>

        <div>
          <label className="label" htmlFor="ac-desc">
            Description
          </label>
          <textarea
            id="ac-desc"
            rows={2}
            value={description}
            onChange={(e) => setDescription(e.target.value)}
            className="field mt-1.5 resize-y leading-relaxed"
          />
          <p className="mt-1 text-[10px] text-ink-500">
            Required. YuE2 calls this <span className="font-mono">style</span>{" "}
            and rejects an empty one outright. Genre, mood, instruments, voice
            and tempo all belong here.
          </p>
        </div>

        <div className="flex items-center justify-between gap-3">
          <div>
            <p className="text-xs text-ink-200">Instrumental</p>
            <p className="text-[10px] text-ink-500">
              No vocals. The lyrics are kept for later rather than sung.
            </p>
          </div>
          <button
            role="switch"
            aria-checked={instrumental}
            aria-label="Instrumental"
            onClick={() => setInstrumental((v) => !v)}
            className={`relative h-5 w-9 shrink-0 rounded-full transition ${
              instrumental ? "bg-glow-500" : "bg-ink-700"
            }`}
          >
            <span
              className={`absolute top-0.5 size-4 rounded-full bg-white transition-all ${
                instrumental ? "left-4.5" : "left-0.5"
              }`}
            />
          </button>
        </div>

        <div className={instrumental ? "opacity-40" : ""}>
          <label className="label" htmlFor="ac-lyrics">
            Lyrics
          </label>
          <textarea
            id="ac-lyrics"
            rows={5}
            value={lyrics}
            onChange={(e) => setLyrics(e.target.value)}
            disabled={instrumental}
            placeholder={"[verse]\nMorning light through the pines…\n\n[chorus]\n…"}
            className="field mt-1.5 resize-y font-mono text-xs leading-relaxed disabled:cursor-not-allowed"
          />
          <p className="mt-1 text-[10px] text-ink-500">
            Section tags shape the song:{" "}
            <span className="font-mono">[verse]</span>,{" "}
            <span className="font-mono">[chorus]</span>,{" "}
            <span className="font-mono">[bridge]</span>. YuE2 sings these
            directly — there is no separate planner.
          </p>
        </div>

        <div className="grid grid-cols-2 gap-3">
          <div>
            <label className="label" htmlFor="ac-seed">
              Seed
            </label>
            <input
              id="ac-seed"
              value={seed}
              onChange={(e) => setSeed(e.target.value)}
              placeholder="random"
              inputMode="numeric"
              className="field mt-1.5"
            />
          </div>
          <div>
            <label className="label" htmlFor="ac-cot">
              Planning
            </label>
            <select
              id="ac-cot"
              value={cot}
              onChange={(e) => setCot(e.target.value)}
              className="field mt-1.5"
            >
              {COT_MODES.map((c) => (
                <option key={c} value={c}>
                  {c}
                </option>
              ))}
            </select>
          </div>
          <div>
            <label className="label" htmlFor="ac-guidance">
              Guidance
            </label>
            <input
              id="ac-guidance"
              type="number"
              min={0}
              max={20}
              step={0.1}
              value={guidance}
              onChange={(e) => setGuidance(e.target.value)}
              className="field mt-1.5"
            />
          </div>
          <div>
            <label className="label" htmlFor="ac-steps">
              Steps
            </label>
            <input
              id="ac-steps"
              type="number"
              min={1}
              value={steps}
              onChange={(e) => setSteps(e.target.value)}
              className="field mt-1.5"
            />
          </div>
        </div>

        <div>
          <label className="label" htmlFor="ac-stop">
            Stop after
          </label>
          <select
            id="ac-stop"
            value={stopAfter}
            onChange={(e) => setStopAfter(e.target.value)}
            className="field mt-1.5"
          >
            {STOP_AFTER.map((s) => (
              <option key={s} value={s}>
                {s === "audio" ? "audio (full render)" : `${s} (skips decode)`}
              </option>
            ))}
          </select>
          <p className="mt-1 text-[10px] leading-relaxed text-ink-500">
            Use <span className="font-mono">semantic</span> to check a model
            really loaded without paying for the VAE decode — it stops after the
            token stages. YuE2 has no length control, so there is no seconds
            slider here.
          </p>
        </div>

        <div className="flex items-center justify-between gap-3">
          <p className="text-[10px] leading-relaxed text-ink-500">
            Cannot be cancelled once started — stopping means killing the
            runtime and losing the partial render.
          </p>
          {busy ? (
            <button
              onClick={() => void audiocpp.cancel()}
              className="btn btn-ghost min-w-28"
            >
              cancel
            </button>
          ) : (
            <button
              onClick={render}
              disabled={!description.trim() || !live}
              className="btn btn-primary min-w-28"
            >
              Generate
            </button>
          )}
        </div>

        {busy && (
          <div className="rounded-lg bg-ink-900 px-3 py-2 text-[11px] text-ink-300">
            <span className="mr-2 inline-block size-1.5 rounded-full bg-glow-400 pulsing" />
            {status?.note ?? "working"}
          </div>
        )}

        {result && (
          <div className="rounded-lg bg-mint-400/10 p-3 text-xs ring-1 ring-mint-400/25">
            <p className="text-mint-300">
              {result.seconds.toFixed(1)}s at {result.sampleRate} Hz ·{" "}
              {Math.round(result.elapsedSecs)}s to render
            </p>
            <p className="mt-1 truncate font-mono text-[10px] text-ink-400">
              {result.path}
            </p>
          </div>
        )}
      </section>
    </div>
  );
}