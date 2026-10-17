import { useCallback, useEffect, useState } from "react";
import {
  generate as generateApi,
  library,
  onStage,
  server as serverApi,
  settings,
  store,
  type SettingsView,
  type Track,
} from "../lib/api";
import type { ServerStatus, StoreSnapshot } from "../lib/types";
import type { GenerateParams } from "../lib/api";
import { recall, remember } from "../lib/session";
import { engines as enginesApi, type Engine, type EngineId } from "../lib/engines";
import type { QueuedJob } from "../lib/types";
import { queue as queueApi } from "../lib/api";

/** Studio form defaults. Owned here, not in the panel: panels unmount on tab
 *  switches, and form state must outlive that. */
export const GENERATE_DEFAULTS: GenerateParams = {
  caption: "",
  lyrics: "",
  duration: 30,
  bpm: 0,
  keyscale: "",
  timesignature: "",
  vocalLanguage: "en",
  inferenceSteps: 0,
  guidanceScale: 0,
  shift: 0,
  seed: -1,
  outputFormat: "mp3",
  synthBatchSize: 1,
  lmModel: "",
  synthModel: "",
  solver: "", // empty = whichever engine is running picks its own default
  taskType: "text2music",
  repaintingStart: 0,
  // -1 is the backend's "to the end of the source" sentinel, not a value the
  // user can type; RepaintRegion renders it as an open-ended region.
  repaintingEnd: -1,
  audioCoverStrength: 1,
  sourceAudio: "",
  refAudio: "",
  title: "",
  vocalGender: "",
  lmTemperature: 0.85,
  lmCfgScale: 2.0,
  lmTopP: 0.9,
  lmTopK: 0,
  lmNegativePrompt: "",
  useCotCaption: true,
  audioCodes: "",
  adapter: "",
  adapterScale: 1.0,
  track: "",
  // HOT-Step CPP extras. Left at their neutral values so a render on the
  // bundled engine is byte-identical to one from before they existed.
  negativePrompt: "",
  vae: "",
  ppVaeReencode: false,
  lmAdapter: "",
  lmAdapterScale: 1.0,
  postprocessPlugin: "",
  lmCfgCutoffRatio: 1.0,
  customTimesteps: "",
  mp3Bitrate: 128,
  ditSlidingWindow: -1,
};

export function useApp() {
  const [view, setView] = useState<"studio" | "library" | "models" | "settings">("studio");
  const [snapshot, setSnapshot] = useState<StoreSnapshot | null>(null);
  const [settingsView, setSettingsView] = useState<SettingsView | null>(null);
  const [status, setStatus] = useState<ServerStatus | null>(null);
  // The job the side panel's progress card describes. Tracked here rather than
  // lifted from QueuePanel: the card lives outside the queue's subtree and
  // re-rendering the whole queue list once a second to feed it would be the
  // opposite of cheap.
  const [activeJob, setActiveJob] = useState<QueuedJob | null>(null);
  // Updated only when the *identity* of the running job changes. Queue events
  // fire on every stage; copying the row into state on each one would re-render
  // the app several times a second for a card that only needs it once.
  useEffect(() => {
    let alive = true;
    const check = async () => {
      try {
        const list = await queueApi.list();
        if (!alive) return;
        const running = list.find((j) => j.state === "running");
        setActiveJob((prev) => {
          if (!running) return prev === null ? prev : null;
          if (prev && prev.id === running.id) return prev;
          return running;
        });
      } catch {
        // The queue command failing is transient; the next event retries.
      }
    };
    void check();
    const un = queueApi.onChanged(() => void check());
    return () => {
      alive = false;
      void un.then((f) => f());
    };
  }, []);
  const [port, setPort] = useState<number | null>(null);
  const [tracks, setTracks] = useState<Track[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [toast, setToast] = useState<string | null>(null);
  // Studio form state lives above the view so it survives tab switches.
  const [params, setParams] = useState<GenerateParams>(GENERATE_DEFAULTS);
  // Which runtime drives the Studio. Per-engine panels hang off this.
  const [engine, setEngine] = useState<EngineId>("acestep");
  const [engineList, setEngineList] = useState<Engine[]>([]);

  const reload = useCallback(async () => {
    try {
      const [snap, cfg, lib] = await Promise.all([
        store.snapshot(),
        settings.get(),
        library.list(),
      ]);
      setSnapshot(snap);
      setSettingsView(cfg);
      setTracks(lib);
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  // Remember the file selections as they change. Sampling knobs and the prompt
  // stay out of it on purpose: those are decisions made fresh each time, while
  // a source track is usually the same one for a run of work.
  useEffect(() => {
    remember({
      sourceAudio: params.sourceAudio,
      refAudio: params.refAudio,
      track: params.track,
      title: params.title,
    });
  }, [params.sourceAudio, params.refAudio, params.track, params.title]);

  // Restore which files the last session was working with. Only pointers are
  // kept, and a pointer whose file has since been deleted is dropped by the
  // picker rather than shown as a broken selection.
  useEffect(() => {
    const saved = recall();
    setParams((p) => ({
      ...p,
      sourceAudio: saved.sourceAudio ?? p.sourceAudio,
      refAudio: saved.refAudio ?? p.refAudio,
      track: saved.track ?? p.track,
      title: saved.title ?? p.title,
    }));
  }, []);

  // Reading the registry is cheap; adopting the bundled runtime is not. After
  // an upgrade the refresh copies ~250 MB of engine binaries, and the Studio's
  // runtime cards render nothing until that finishes -- so they used to be the
  // last thing on screen. List first, adopt in the background, list again.
  useEffect(() => {
    let alive = true;
    void (async () => {
      try {
        setEngineList(await enginesApi.list());
      } catch {
        // The registry stays empty; the rest of the app still works.
      }
      try {
        await enginesApi.adoptBundled();
        // HOT-Step ships in the bundle too. Adopting is a no-op once the user
        // has their own copy of that engine installed.
        await enginesApi.adoptBundledHotStep();
      } catch {
        // Adoption failing must not block the app: the engine list below still
        // reports whatever is installed.
      }
      if (!alive) return;
      try {
        setEngineList(await enginesApi.list());
      } catch {
        // Leave the first list in place.
      }
    })();
    return () => {
      alive = false;
    };
  }, []);

  // First paint comes from the local scan alone — no network on this path,
  // which is what makes the app usable offline and stops the launch stall.
  useEffect(() => {
    void reload();
    const unlisten = onStage(() => {});
    return () => {
      void unlisten.then((off) => off());
    };
  }, [reload]);

  // The Hub catalogue is enrichment: it fills in weights you could download.
  // Refreshed after the UI is already usable, and failures are silent.
  useEffect(() => {
    let alive = true;
    void (async () => {
      try {
        const [stale, list] = await Promise.all([store.catalogueIsStale(), store.refresh()]);
        if (!alive || !stale || list.length === 0) return;
        const snap = await store.snapshot();
        if (alive) setSnapshot(snap);
      } catch {
        // Offline. The local scan already gave us everything that matters.
      }
    })();
    return () => {
      alive = false;
    };
  }, []);

  // Adopt a populated folder the user already has, so a fresh install points
  // at working weights instead of an empty default directory.
  useEffect(() => {
    if (settingsView) return;
    const known = "/media/ghost/Pride/AceStep Music Models";
    void (async () => {
      try {
        const cfg = await settings.get();
        if (cfg.models_dir.trim() === "") {
          setSettingsView(await settings.setModelsDir(known));
          setSnapshot(await store.snapshot());
        }
      } catch {
        // A missing path here is not fatal; the default still works.
      }
    })();
  }, [settingsView]);

  const startServer = useCallback(async () => {
    // The engine is passed explicitly: both engines ship a binary called
    // `ace-server`, so starting "the server" without saying which one picks
    // whichever is resolved first.
    setBusy(true);
    setError(null);
    try {
      const next = await serverApi.start(engine);
      setStatus(next);
      setPort(await serverApi.port());
      if (!next.ready) {
        setError("server started but never became ready");
      }
      await reload();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }, [engine, reload]);

  const stopServer = useCallback(async () => {
    setBusy(true);
    try {
      await serverApi.stop();
      setStatus(null);
      setPort(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }, []);

  const saveVaeChunk = useCallback(async (chunk: number) => {
    setBusy(true);
    setError(null);
    try {
      setSettingsView(await settings.setVaeChunk(chunk));
      setToast("tile size saved — restart the runtime to apply");
    } catch (e) {
      setError(String(e));
      throw e;
    } finally {
      setBusy(false);
    }
  }, []);

  const saveHotStepDir = useCallback(async (path: string) => {
    setBusy(true);
    setError(null);
    try {
      setSettingsView(await settings.setHotStepModelsDir(path));
      setToast("HOT-Step models folder saved");
    } catch (e) {
      setError(String(e));
      throw e;
    } finally {
      setBusy(false);
    }
  }, []);

  const saveMaxSeq = useCallback(async (value: number) => {
    setBusy(true);
    setError(null);
    try {
      setSettingsView(await settings.setMaxSeq(value));
      setToast("context size saved — restart the runtime to apply");
    } catch (e) {
      setError(String(e));
      throw e;
    } finally {
      setBusy(false);
    }
  }, []);

  const saveModelsDir = useCallback(async (path: string) => {
    setBusy(true);
    setError(null);
    try {
      setSettingsView(await settings.setModelsDir(path));
      setSnapshot(await store.snapshot());
      setToast("models folder updated");
    } catch (e) {
      setError(String(e));
      throw e;
    } finally {
      setBusy(false);
    }
  }, []);

  // Both "auto" defaults resolve to the first registry entry, which is
  // alphabetical — and alphabetical picks the wrong model in both cases.
  // Pin the better one whenever it is installed.
  useEffect(() => {
    if (!snapshot) return;
    setParams((current) => {
      let next = current;

      if (next.synthModel === "") {
        // sft sorts before turbo, so this defaulted to 50 steps on a card
        // where turbo is 8.
        const turbo = snapshot.files.find(
          (f) => f.role === "Dit" && f.present && f.variant === "turbo",
        );
        if (turbo) {
          next = { ...next, synthModel: turbo.filename };
          // Match the sampling preset to the DiT the app just chose.
          if (next.inferenceSteps === 0 || next.inferenceSteps === 8) {
            next = { ...next, inferenceSteps: 8, guidanceScale: 1.0, shift: 3.0 };
          }
        }
      }

      if (next.lmModel === "") {
        // The 0.6B planner writes pseudo-language rather than lyrics: measured
        // "Vieler's steps on the mountain high" for English and
        // "Lutanga, putang luna" on a shorter run. The 1.7B writes real English.
        const big = snapshot.files.find(
          (f) =>
            f.role === "Planner" &&
            f.present &&
            f.variant !== "0.6B" &&
            f.variant !== "0.6",
        );
        if (big) next = { ...next, lmModel: big.filename };
      }

      return next;
    });
  }, [snapshot]);

  const handleGenerated = useCallback(
    async (fresh: Track[]) => {
      setTracks((current) => fresh.concat(current));
      // Only navigate if the user is still where the render started. Yanking
      // someone out of the tab they deliberately moved to is hostile.
      setView((current) => (current === "studio" ? "library" : current));
      await reload();
    },
    [reload],
  );

  const refreshLibrary = useCallback(async () => {
    setTracks(await library.list());
  }, []);

  useEffect(() => {
    if (!toast) return;
    const t = setTimeout(() => setToast(null), 2600);
    return () => clearTimeout(t);
  }, [toast]);

  return {
    view,
    setView,
    engine,
    setEngine,
    engineList,
    snapshot,
    settingsView,
    status,
    port,
    tracks,
    params,
    setParams,
    busy,
    error,
    toast,
    reload,
    refreshLibrary,
    startServer,
    stopServer,
    saveModelsDir,
    saveVaeChunk,
    saveHotStepDir,
    activeJob,
    saveMaxSeq,
    handleGenerated,
    generateApi,
  };
}