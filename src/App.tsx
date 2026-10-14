import { AudioCppPanel } from "./components/AudioCppPanel";
import { EngineSwitcher } from "./components/EngineSwitcher";
import { GeneratePanel } from "./components/GeneratePanel";
import { LibraryPanel } from "./components/LibraryPanel";
import { LogViewer } from "./components/LogViewer";
import { SettingsPanel } from "./components/SettingsPanel";
import { StorePanel } from "./components/StorePanel";
import { useEffect, useRef } from "react";
import { useApp } from "./store/useApp";

export default function App() {
  const app = useApp();
  const running = app.status?.ready ?? false;

  // Views stay mounted, so the scroll container keeps the scrollTop of
  // whichever view was last open. Switching to a shorter view then lands you
  // past its end, which paints as an empty page until you scroll back up.
  const scrollRef = useRef<HTMLElement>(null);
  useEffect(() => {
    const node = scrollRef.current;
    if (node) node.scrollTop = 0;
  }, [app.view]);

  // Switching while a download is in flight should not strand the bar.
  const viewRef = useRef(app.view);
  useEffect(() => {
    if (viewRef.current === app.view) return;
    viewRef.current = app.view;
  }, [app.view]);

  const TABS = [
    { id: "studio", label: "Studio", icon: IconSpark },
    { id: "library", label: "Library", icon: IconLibrary },
    { id: "models", label: "Models", icon: IconModel },
    { id: "settings", label: "Settings", icon: IconGear },
  ] as const;

  return (
    <div className="flex h-screen overflow-hidden">
      <aside className="flex w-52 shrink-0 flex-col gap-1 overflow-y-auto border-r border-ink-800 bg-ink-900/60 p-4 backdrop-blur-xl">
        <div className="mb-6 flex items-center gap-2.5 px-2 pt-1">
          <div className="flex size-8 items-center justify-center rounded-xl bg-gradient-to-br from-glow-500 to-indigo-600 shadow-lg shadow-glow-500/20">
            <IconWave />
          </div>
          <div className="leading-tight">
            <p className="text-sm font-semibold">AceForge</p>
            <p className="text-[10px] text-ink-500">ACE-Step 1.5</p>
          </div>
        </div>

        <nav className="space-y-1">
          {TABS.map(({ id, label, icon: Icon }) => (
            <button
              key={id}
              onClick={() => app.setView(id)}
              data-active={app.view === id}
              className="nav-item w-full"
            >
              <Icon />
              {label}
            </button>
          ))}
        </nav>

        <div className="mt-auto space-y-2 pt-4">
          <div className="panel p-3">
            <div className="flex items-center justify-between">
              <span className="text-[11px] text-ink-400">Runtime</span>
              <span
                className={`chip ${
                  running
                    ? "bg-mint-400/15 text-mint-400 ring-1 ring-mint-400/30"
                    : "bg-ink-800 text-ink-500"
                }`}
              >
                <span
                  className={`size-1.5 rounded-full ${running ? "bg-mint-400" : "bg-ink-500"}`}
                />
                {running ? "live" : "offline"}
              </span>
            </div>
            <p className="mt-1.5 truncate font-mono text-[10px] text-ink-500">
              {running ? `port ${app.port}` : "ace-server"}
            </p>
          </div>

          {running ? (
            <button onClick={app.stopServer} disabled={app.busy} className="btn btn-ghost w-full">
              Stop
            </button>
          ) : (
            <button onClick={app.startServer} disabled={app.busy} className="btn btn-ghost w-full">
              {app.busy ? "Starting…" : "Start"}
            </button>
          )}
        </div>
      </aside>

      <main ref={scrollRef} className="min-w-0 flex-1 overflow-y-auto overscroll-contain">
        <div className="mx-auto max-w-3xl px-8 py-8">
          <header className="mb-6">
            <h1 className="text-xl font-semibold tracking-tight">
              {app.view === "studio" && "Studio"}
              {app.view === "library" && "Library"}
              {app.view === "models" && "Models"}
              {app.view === "settings" && "Settings"}
            </h1>
            <p className="mt-0.5 text-xs text-ink-500">
              {app.view === "studio" && "Describe a track and render it locally."}
              {app.view === "library" &&
                `${app.tracks.length} track${app.tracks.length === 1 ? "" : "s"} on disk.`}
              {app.view === "models" && "Pick a preset, then fetch the weights yourself."}
              {app.view === "settings" && "Where weights live and how the runtime is launched."}
            </p>
          </header>

          {app.error && (
            <div className="panel mb-5 border-rose-500/25 bg-rose-500/5 p-3">
              <pre className="overflow-x-auto text-xs text-rose-300/90">{app.error}</pre>
            </div>
          )}

          {/* Every view stays mounted and is hidden instead of unmounted.
            Conditional mounting threw away form state and re-ran each panel's
            initial load on every tab switch, which is what made the library
            flash empty before repopulating. */}
          <div hidden={app.view !== "studio"} className="space-y-5">
            <EngineSwitcher
              selected={app.engine}
              onSelect={app.setEngine}
              engines={app.engineList}
            />

            {/* Per-engine panels. Conditional, not keep-alive: the form state
                lives in useApp, so unmounting costs nothing, and the `hidden`
                attribute collides with Tailwind's `[hidden]{display:none}`
                preflight rule. */}
            {app.engine === "acestep" && (
              <GeneratePanel
                port={app.port}
                running={running}
                catalog={app.snapshot?.files ?? []}
                params={app.params}
                setParams={app.setParams}
                onBrowseModels={() => app.setView("models")}
                onGenerated={app.handleGenerated}
                tracks={app.tracks}
              />
            )}

            {app.engine === "audiocpp" && (
              <AudioCppPanel
                modelDir={app.settingsView?.audiocpp_models_dir ?? ""}
                busy={app.busy}
                onSaveDir={app.saveAudioCppDir}
                onRendered={() => app.refreshLibrary()}
              />
            )}
          </div>

          <div hidden={app.view !== "library"} className="space-y-5">
            <LibraryPanel tracks={app.tracks} onChanged={app.refreshLibrary} />
            <LogViewer running={running} />
          </div>

          {app.snapshot && (
          <div hidden={app.view !== "models"}>
            <div className="space-y-5">
              {app.settingsView && (
                <SettingsPanel
                  view={app.settingsView}
                  busy={app.busy}
                  running={running}
                  onSave={app.saveModelsDir}
                  onSaveVaeChunk={app.saveVaeChunk}
                  onSaveMaxSeq={app.saveMaxSeq}
                  onChanged={app.reload}
                />
              )}
              <StorePanel snapshot={app.snapshot} onChanged={app.reload} />
            </div>
          </div>
          )}

          {app.settingsView && (
            <div hidden={app.view !== "settings"} className="space-y-5">
              <SettingsPanel
                view={app.settingsView}
                busy={app.busy}
                running={running}
                onSave={app.saveModelsDir}
                onSaveVaeChunk={app.saveVaeChunk}
                  onSaveMaxSeq={app.saveMaxSeq}
                onChanged={app.reload}
              />
              <RuntimePanel
                running={running}
                port={app.port}
                status={app.status}
              />
            </div>
          )}
        </div>
      </main>

      {app.toast && (
        <div className="pointer-events-none fixed bottom-6 left-1/2 -translate-x-1/2 rounded-xl bg-ink-800 px-4 py-2 text-xs text-ink-100 shadow-2xl ring-1 ring-ink-700">
          {app.toast}
        </div>
      )}
    </div>
  );
}

function RuntimePanel({
  running,
  port,
  status,
}: {
  running: boolean;
  port: number | null;
  status: { pid: number | null; log_tail: string[] } | null;
}) {
  return (
    <section className="panel space-y-3 p-5">
      <h3 className="text-sm font-medium">Runtime</h3>
      <dl className="space-y-1.5 text-xs">
        <Row label="State" value={running ? "running" : "stopped"} />
        <Row label="Port" value={port ? String(port) : "—"} />
        <Row label="PID" value={status?.pid ? String(status.pid) : "—"} />
        <Row label="Device" value="CUDA (sm_61)" />
      </dl>
      {status && status.log_tail.length > 0 && (
        <pre className="max-h-40 overflow-auto rounded-lg bg-ink-900 p-3 text-[11px] leading-relaxed text-ink-400">
          {status.log_tail.join("\n")}
        </pre>
      )}
      <p className="text-[11px] leading-relaxed text-ink-500">
        Weights are never fetched by the app. The store hands you a curl script
        and the server only reads the folder you point it at.
      </p>
    </section>
  );
}

function Row({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex justify-between gap-4">
      <dt className="text-ink-500">{label}</dt>
      <dd className="font-mono text-ink-200">{value}</dd>
    </div>
  );
}

function IconWave() {
  return (
    <svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="white" strokeWidth="1.8" strokeLinecap="round">
      <path d="M2 8h1.5M5.5 4v8M8 2v12M10.5 5.5v5M13.5 8H15" />
    </svg>
  );
}

function IconSpark() {
  return (
    <svg width="15" height="15" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
      <path d="M8 1.5 9.6 6 14 7.6 9.6 9.2 8 13.7 6.4 9.2 2 7.6 6.4 6z" />
    </svg>
  );
}

function IconLibrary() {
  return (
    <svg width="15" height="15" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
      <path d="M2 3.5h12M2 8h12M2 12.5h8" />
    </svg>
  );
}

function IconModel() {
  return (
    <svg width="15" height="15" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
      <path d="M8 1.5 14 5v6l-6 3.5L2 11V5zM2 5l6 3.5L14 5M8 8.5V14.5" />
    </svg>
  );
}

function IconGear() {
  return (
    <svg width="15" height="15" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
      <circle cx="8" cy="8" r="2.2" />
      <path d="M8 1.5v1.8M8 12.7v1.8M14.5 8h-1.8M3.3 8H1.5M12.6 3.4l-1.3 1.3M4.7 11.3l-1.3 1.3M12.6 12.6l-1.3-1.3M4.7 4.7 3.4 3.4" />
    </svg>
  );
}