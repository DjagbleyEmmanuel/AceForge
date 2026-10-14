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
import { engines as enginesApi, type Engine, type EngineId } from "../lib/engines";

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
  solver: "euler",
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
};

export function useApp() {
  const [view, setView] = useState<"studio" | "library" | "models" | "settings">("studio");
  const [snapshot, setSnapshot] = useState<StoreSnapshot | null>(null);
  const [settingsView, setSettingsView] = useState<SettingsView | null>(null);
  const [status, setStatus] = useState<ServerStatus | null>(null);
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

  // Adopt the bundled runtime so a fresh install has a working engine, then
  // read the registry. Neither touches the network.
  useEffect(() => {
    void (async () => {
      try {
        await enginesApi.adoptBundled();
        // audio.cpp ships in the bundle too; adopting it is a no-op once the
        // user has their own copy installed.
        await enginesApi.adoptBundledAudioCpp();
        setEngineList(await enginesApi.list());
      } catch {
        // Engine list stays empty; the rest of the app still works.
      }
    })();
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
    setBusy(true);
    setError(null);
    try {
      const next = await serverApi.start();
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
  }, [reload]);

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

  const saveAudioCppDir = useCallback(async (path: string) => {
    setBusy(true);
    setError(null);
    try {
      setSettingsView(await settings.setAudioCppModelsDir(path));
      setToast("audio.cpp models folder saved");
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
    saveAudioCppDir,
    saveMaxSeq,
    handleGenerated,
    generateApi,
  };
}