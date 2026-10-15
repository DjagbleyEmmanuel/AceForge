import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { imports, type ImportedFile } from "../lib/api";
import { formatBytes } from "../lib/types";

interface Props {
  value: string;
  onChange: (path: string) => void;
  placeholder: string;
  label: string;
  disabled?: boolean;
  describe?: (file: ImportedFile) => string;
  onFiles?: (files: ImportedFile[]) => void;
}

/**
 * Source chooser for imported audio.
 *
 * Lists only files you brought in, not the songs the app has generated — the
 * generated library is a wall of near-identical CoT captions and useless as a
 * source menu.
 *
 * Stays in the page rather than using a native `<select>`: in this webview a
 * select renders as a viewport-height popup that covers the entire form.
 *
 * The list is a history, not a live view of disk, so it survives the library
 * being moved and can offer to remove an entry.
 */
export function SourcePicker({
  value,
  onChange,
  placeholder,
  label,
  disabled,
  describe,
  /** Reports what is currently importable, so the caller can tell an import
   *  from a path that no longer exists. */
  onFiles,
}: Props) {
  const [files, setFiles] = useState<ImportedFile[]>([]);
  const [expanded, setExpanded] = useState(false);
  const [query, setQuery] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const wrapRef = useRef<HTMLDivElement>(null);

  const reload = useCallback(async () => {
    try {
      const listed = await imports.list();
      setFiles(listed);
      onFiles?.(listed);
    } catch (e) {
      setError(String(e));
    }
  }, [onFiles]);

  useEffect(() => {
    void reload();
  }, [reload]);

  useEffect(() => {
    if (!expanded) return;
    const onDown = (e: PointerEvent) => {
      if (!wrapRef.current?.contains(e.target as Node)) setExpanded(false);
    };
    document.addEventListener("pointerdown", onDown);
    return () => document.removeEventListener("pointerdown", onDown);
  }, [expanded]);

  const filtered = useMemo(() => {
    const needle = query.trim().toLowerCase();
    if (!needle) return files;
    return files.filter((f) => f.name.toLowerCase().includes(needle));
  }, [files, query]);

  const selected = files.find((f) => f.path === value) ?? null;

  async function pick() {
    setError(null);
    setBusy(true);
    try {
      const picked = await open({
        multiple: true,
        title: "Import audio to use as a source",
        // The backend decodes RIFF/WAV or MP3 and nothing else, so offering
        // FLAC or OGG here would only produce a failure minutes later.
        filters: [{ name: "Audio (WAV or MP3)", extensions: ["wav", "mp3"] }],
      });
      const paths = Array.isArray(picked) ? picked : picked ? [picked] : [];
      if (paths.length === 0) return;
      const added = await imports.add(paths);
      await reload();
      // Select the first thing just imported; that is what it was opened for.
      if (added[0]) onChange(added[0].path);
      setExpanded(true);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function remove(f: ImportedFile) {
    setError(null);
    try {
      await imports.remove(f.path);
      // Clearing the selection first: leaving a path pointing at a deleted
      // file would make the next render submit audio that no longer exists.
      if (value === f.path) onChange("");
      await reload();
    } catch (e) {
      setError(String(e));
    }
  }

  return (
    <div className="space-y-1.5" ref={wrapRef}>
      <label className="label">{label}</label>

      <div className="flex gap-2">
        <button
          type="button"
          disabled={disabled}
          onClick={() => setExpanded((v) => !v)}
          aria-expanded={expanded}
          className="field flex min-w-0 flex-1 items-center justify-between gap-2 text-left disabled:cursor-not-allowed disabled:opacity-50"
        >
          <span
            className={
              selected ? "truncate text-ink-100" : "truncate text-ink-500"
            }
          >
            {selected ? selected.name : placeholder}
          </span>
          {/* How many are available. Without this the closed picker reads
              identically whether nothing was ever imported or five files are
              waiting one click away -- which is indistinguishable from the app
              having forgotten them. Generation settings are not persisted, so
              the selection is empty again on every launch and this is the only
              thing that says the files are still there. */}
          {files.length > 0 && !selected && (
            <span className="shrink-0 rounded-full bg-ink-800 px-1.5 py-0.5 font-mono text-[10px] text-ink-300 ring-1 ring-ink-700">
              {files.length} available
            </span>
          )}
          <span className="shrink-0 text-[10px] text-ink-500">
            {expanded ? "▲" : "▼"}
          </span>
        </button>
        <button
          type="button"
          onClick={() => void pick()}
          disabled={disabled || busy}
          className="btn btn-ghost shrink-0 px-3 py-1.5 text-xs disabled:opacity-40"
        >
          {busy ? "…" : "import…"}
        </button>
      </div>

      {expanded && (
        <div className="rounded-lg border border-ink-700 bg-ink-900 p-2">
          {files.length === 0 ? (
            <p className="px-1 py-2 text-[11px] text-ink-500">
              Nothing imported yet. Use <strong>import…</strong> to bring in a
              WAV or MP3 from anywhere on disk.
            </p>
          ) : (
            <>
              {!selected && (
                <p className="mb-1.5 px-1 text-[11px] text-glow-300">
                  {files.length} imported — pick one to use it
                </p>
              )}
              <input
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                placeholder={`filter ${files.length} imported…`}
                className="field w-full text-xs"
              />
              {filtered.length === 0 ? (
                <p className="px-1 py-2 text-[11px] text-ink-500">
                  Nothing matches “{query}”.
                </p>
              ) : (
                <ul className="mt-1.5 max-h-52 overflow-y-auto">
                  {filtered.map((f) => (
                    <li key={f.path}>
                      <div className="flex items-center gap-1">
                        <button
                          type="button"
                          onClick={() => {
                            onChange(f.path);
                            setExpanded(false);
                          }}
                          className={`min-w-0 flex-1 rounded px-1.5 py-1.5 text-left text-[11px] transition ${
                            f.path === value
                              ? "bg-ink-800 text-ink-100"
                              : "text-ink-300 hover:bg-ink-850"
                          }`}
                        >
                          <span className="block truncate">{f.name}</span>
                          <span className="block font-mono text-[10px] text-ink-500">
                            {formatBytes(f.bytes)}
                          </span>
                        </button>
                        <button
                          type="button"
                          onClick={() => void remove(f)}
                          aria-label={`Remove ${f.name}`}
                          className="shrink-0 px-1.5 py-1.5 text-[11px] text-ink-500 hover:text-rose-300"
                        >
                          ×
                        </button>
                      </div>
                    </li>
                  ))}
                </ul>
              )}
              {selected && describe && (
                <p className="mt-1.5 border-t border-ink-800 pt-1.5 text-[10px] text-ink-500">
                  {describe(selected)}
                </p>
              )}
            </>
          )}
        </div>
      )}

      {error && <p className="text-[11px] text-rose-300">{error}</p>}
    </div>
  );
}