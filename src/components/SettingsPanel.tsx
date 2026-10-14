import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { settings, type SettingsView } from "../lib/api";

interface Props {
  view: SettingsView;
  busy: boolean;
  running: boolean;
  onSave: (path: string) => Promise<void>;
  onSaveVaeChunk: (chunk: number) => Promise<void>;
  onSaveMaxSeq: (value: number) => Promise<void>;
  onChanged: () => void;
}

export function SettingsPanel({
  view,
  busy,
  running,
  onSave,
  onSaveVaeChunk,
  onSaveMaxSeq,
  onChanged,
}: Props) {
  // Seed the input from the saved value, falling back to what is live now.
  const [draft, setDraft] = useState(view.models_dir);
  const [saved, setSaved] = useState(false);
  const [libDraft, setLibDraft] = useState(view.library_dir);
  const [libBusy, setLibBusy] = useState(false);
  const [libError, setLibError] = useState<string | null>(null);
  const [chunk, setChunk] = useState(view.vae_chunk);
  const [max_seq, setMaxSeq] = useState(view.max_seq);

  useEffect(() => {
    setDraft(view.models_dir);
  }, [view.models_dir]);

  useEffect(() => {
    setLibDraft(view.library_dir);
  }, [view.library_dir]);

  useEffect(() => {
    setChunk(view.vae_chunk);
  }, [view.vae_chunk]);

  useEffect(() => {
    setMaxSeq(view.max_seq);
  }, [view.max_seq]);

  useEffect(() => {
    if (!saved) return;
    const t = setTimeout(() => setSaved(false), 2000);
    return () => clearTimeout(t);
  }, [saved]);

  const usingDefault = view.models_dir.trim() === "";
  const dirty = draft.trim() !== view.models_dir.trim();

  async function submit() {
    try {
      await onSave(draft);
      setSaved(true);
      onChanged();
    } catch {
      setSaved(false);
    }
  }

  async function saveLibrary(path: string) {
    setLibBusy(true);
    setLibError(null);
    try {
      await settings.setLibraryDir(path);
      onChanged();
    } catch (e) {
      setLibError(String(e));
    } finally {
      setLibBusy(false);
    }
  }

  async function browseLibrary() {
    setLibBusy(true);
    setLibError(null);
    try {
      const picked = await open({
        directory: true,
        multiple: false,
        title: "Choose where finished songs are saved",
      });
      if (typeof picked === "string") setLibDraft(picked);
    } catch (e) {
      setLibError(String(e));
    } finally {
      setLibBusy(false);
    }
  }

  return (
    <section className="panel space-y-5 p-5">
      <div>
        <h3 className="text-sm font-semibold">Models folder</h3>
        <p className="mt-1 text-xs text-ink-500">
          Where AceForge looks for GGUF weights. Leave empty to use the default
          location.
        </p>
      </div>

      {view.env_override && (
        <p className="rounded bg-amber-400/10 px-3 py-2 text-xs text-amber-300 ring-1 ring-amber-400/25">
          <code>ACEFORGE_MODELS_DIR</code> is set in this environment
          (<code>{view.env_override}</code>) and overrides the setting below.
        </p>
      )}

      <div className="flex flex-col gap-2 sm:flex-row">
        <input
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          placeholder={view.default_models_dir}
          spellCheck={false}
          aria-label="Models folder path"
          className="field min-w-0 flex-1"
        />
        <div className="flex gap-2">
          <button
            onClick={submit}
            disabled={busy || !dirty}
            className="btn btn-primary px-4 py-1.5 text-xs"
          >
            {saved ? "Saved" : "Save"}
          </button>
          {!usingDefault && (
            <button
              onClick={() => setDraft("")}
              disabled={busy}
              className="btn btn-ghost px-3 py-1.5 text-xs"
            >
              Use default
            </button>
          )}
        </div>
      </div>

      <dl className="space-y-1 text-xs">
        <div className="flex gap-2">
          <dt className="shrink-0 text-ink-500">Default:</dt>
          <dd className="truncate font-mono text-ink-400">
            {view.default_models_dir}
          </dd>
        </div>
        <div className="flex gap-2">
          <dt className="shrink-0 text-ink-500">Active:</dt>
          <dd className="truncate font-mono text-ink-200">
            {view.resolved_models_dir}
          </dd>
        </div>
      </dl>

      {view.resolved_models_dir !== view.default_models_dir && (
        <p className="text-xs text-ink-500">
          Custom folder — the download scripts below target it.
        </p>
      )}

      <div className="space-y-2 border-t border-ink-800 pt-4">
        <div>
          <h3 className="text-sm font-semibold">Music folder</h3>
          <p className="mt-1 text-xs text-ink-500">
            Where finished songs and imported audio are kept. Leave empty to
            use the default.
          </p>
        </div>

        <div className="flex flex-col gap-2 sm:flex-row">
          <input
            value={libDraft}
            onChange={(e) => setLibDraft(e.target.value)}
            placeholder={view.resolved_library_dir}
            spellCheck={false}
            aria-label="Music folder path"
            className="field min-w-0 flex-1"
          />
          <div className="flex gap-2">
            <button
              onClick={() => void browseLibrary()}
              disabled={libBusy}
              className="btn btn-ghost px-3 py-1.5 text-xs disabled:opacity-40"
            >
              browse…
            </button>
            <button
              onClick={() => void saveLibrary(libDraft)}
              disabled={libBusy || libDraft.trim() === view.library_dir.trim()}
              className="btn btn-primary px-4 py-1.5 text-xs disabled:opacity-40"
            >
              Save
            </button>
          </div>
        </div>

        <dl className="space-y-1 text-xs">
          <div className="flex gap-2">
            <dt className="shrink-0 text-ink-500">Active:</dt>
            <dd className="truncate font-mono text-ink-200">
              {view.resolved_library_dir}
            </dd>
          </div>
        </dl>

        {view.library_dir.trim() !== "" && (
          <p className="text-xs text-ink-500">
            Songs already generated stay in the old folder. Imported audio moves
            with this setting and will need re-importing.
          </p>
        )}
        {libError && (
          <p className="text-xs text-rose-300">{libError}</p>
        )}
      </div>

      <div className="border-t border-ink-800 pt-3">
        <label className="label" htmlFor="chunk">
          VAE tile size
        </label>
        <div className="mt-1.5 flex items-center gap-3">
          <input
            id="chunk"
            type="range"
            min={64}
            max={1024}
            step={32}
            value={chunk}
            onChange={(e) => setChunk(Number(e.target.value))}
            onMouseUp={() => void onSaveVaeChunk(chunk)}
            onTouchEnd={() => void onSaveVaeChunk(chunk)}
            className="flex-1"
          />
          <span className="w-12 shrink-0 text-right font-mono text-xs text-ink-300">
            {chunk}
          </span>
        </div>
        <p className="mt-2 text-[11px] leading-relaxed text-ink-500">
          Bigger tiles decode faster but need more VRAM. The upstream default
          of 1024 crashes a 4 GB card during decode; 256 is verified working.
        </p>
      </div>

      <div className="border-t border-ink-800 pt-3">
        <label className="label" htmlFor="maxseq">
          Planner context
        </label>
        <div className="mt-1.5 flex items-center gap-3">
          <input
            id="maxseq"
            type="range"
            min={1024}
            max={8192}
            step={512}
            value={max_seq}
            onChange={(e) => setMaxSeq(Number(e.target.value))}
            onMouseUp={() => void onSaveMaxSeq(max_seq)}
            onTouchEnd={() => void onSaveMaxSeq(max_seq)}
            className="flex-1"
          />
          <span className="w-12 shrink-0 text-right font-mono text-xs text-ink-300">
            {max_seq}
          </span>
        </div>
        <p className="mt-2 text-[11px] leading-relaxed text-ink-500">
          LM KV cache size, in tokens. Upstream defaults to 8192, which the
          1.7B planner cannot prefill on a 4 GB card — it dies allocating its
          graph buffer. 4096 is verified working with both planners.
          {running && " Restart the runtime to apply a change."}
        </p>
      </div>
    </section>
  );
}