import { useEffect, useMemo, useRef, useState } from "react";
import { downloads, store } from "../lib/api";
import type { DownloadStatus } from "../lib/api";
import {
  formatBytes,
  roleLabel,
  type Preset,
  type StoreSnapshot,
  type WeightFile,
} from "../lib/types";

type RoleFilter = "all" | "Planner" | "TextEncoder" | "Dit" | "Vae";

interface Props {
  snapshot: StoreSnapshot;
  onChanged: () => void;
}

export function StorePanel({ snapshot, onChanged }: Props) {
  // Re-reads the directory once a transfer finishes.
  const [query, setQuery] = useState("");
  const [role, setRole] = useState<RoleFilter>("all");
  const [hideInstalled, setHideInstalled] = useState(false);
  const [onlySupported, setOnlySupported] = useState(false);
  const [free, setFree] = useState<number | null>(null);
  const [progress, setProgress] = useState<Record<string, DownloadStatus>>({});
  const [script, setScript] = useState<string | null>(null);
  const [showAll, setShowAll] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const liveRef = useRef<string | null>(null);

  useEffect(() => setScript(null), [snapshot.models_dir]);

  useEffect(() => {
    void store.freeSpace().then(setFree).catch(() => setFree(null));
  }, [snapshot.models_dir]);

  // Live progress comes over an event channel; seed from the registry so a
  // download started before this view mounted still shows a bar.
  useEffect(() => {
    let off: (() => void) | null = null;
    void downloads
      .statuses()
      .then((list) => {
        setProgress(Object.fromEntries(list.map((s) => [s.id, s])));
      })
      .catch(() => {});
    void downloads.onProgress((s) => {
      setProgress((prev) => ({ ...prev, [s.id]: s }));
      // A finished transfer changes what is installed, so rescan the folder
      // rather than waiting for the user to press Rescan.
      if (s.state === "done" || s.state === "cancelled") {
        onChanged();
        void downloads.clearFinished();
      }
    })
      .then((f) => {
        off = f;
        liveRef.current = "on";
      });
    // `onChanged` is stable from the parent; re-running on its identity is
    // what keeps a rescan triggered after each completed download.
    return () => {
      off?.();
      liveRef.current = null;
    };
  }, [onChanged]);

  const files = snapshot.files;

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    return files.filter((f) => {
      if (role !== "all" && f.role !== role) return false;
      if (hideInstalled && f.present) return false;
      if (onlySupported && !f.backend_supported) return false;
      if (!q) return true;
      return (
        f.filename.toLowerCase().includes(q) ||
        f.variant.toLowerCase().includes(q) ||
        f.quant.toLowerCase().includes(q) ||
        roleLabel(f.role).toLowerCase().includes(q)
      );
    });
  }, [files, query, role, hideInstalled, onlySupported]);

  const installed = files.filter((f) => f.present).length;
  const byRole = useMemo(() => {
    const acc: Record<string, number> = {};
    for (const f of files) acc[f.role] = (acc[f.role] ?? 0) + 1;
    return acc;
  }, [files]);

  async function loadScript(preset: Preset) {
    setScript(await store.downloadScript(preset.id));
  }

  async function begin(file: WeightFile) {
    setBusy(file.filename);
    setError(null);
    try {
      await downloads.start(file);
      setProgress((p) => ({
        ...p,
        [file.filename]: {
          id: file.filename,
          filename: file.filename,
          received: file.partial_bytes,
          total: file.size_bytes,
          state: "running",
          error: null,
          rate_bps: 0,
          eta_secs: null,
        },
      }));
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  }

  async function cancel(file: WeightFile) {
    await downloads.cancel(file.filename);
  }

  const shown = showAll ? filtered : filtered.slice(0, 40);

  return (
    <section className="space-y-5">
      <header className="flex flex-wrap items-end justify-between gap-3">
        <div>
          <h2 className="text-sm font-medium text-ink-200">Available weights</h2>
          <p className="mt-0.5 text-xs text-ink-500">
            <code className="text-ink-300">{snapshot.repo}</code> ·{" "}
            {installed} of {files.length} installed
          </p>
        </div>
        {free !== null && (
          <p className="text-xs text-ink-500">
            {formatBytes(free)} free
          </p>
        )}
      </header>

      {error && (
        <p className="rounded-lg bg-rose-500/10 px-3 py-2 text-xs text-rose-300 ring-1 ring-rose-500/25">
          {error}
        </p>
      )}

      {/* Search + filters */}
      <div className="space-y-2">
        <input
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="Search turbo, sft, xl, Q4_K_M, VAE…"
          spellCheck={false}
          className="field"
        />
        <div className="flex flex-wrap items-center gap-1.5">
          {(["all", "Planner", "TextEncoder", "Dit", "Vae"] as RoleFilter[]).map(
            (r) => {
              const count = r === "all" ? files.length : (byRole[r] ?? 0);
              return (
                <button
                  key={r}
                  onClick={() => setRole(r)}
                  className={`chip px-2.5 py-1 transition ${
                    role === r
                      ? "bg-glow-500/20 text-glow-300 ring-1 ring-glow-500/45"
                      : "bg-ink-800 text-ink-300 ring-1 ring-ink-700 hover:bg-ink-750"
                  }`}
                >
                  {r === "all" ? "All" : roleLabel(r as never)}
                  <span className="text-ink-500">{count}</span>
                </button>
              );
            },
          )}
          <span className="mx-1 h-4 w-px bg-ink-700" />
          <Toggle
            active={hideInstalled}
            onClick={() => setHideInstalled((v) => !v)}
            label="Hide installed"
          />
          <Toggle
            active={onlySupported}
            onClick={() => setOnlySupported((v) => !v)}
            label="Only backend-supported"
          />
        </div>
      </div>

      {/* Presets */}
      <div className="space-y-2">
        <h3 className="text-sm font-medium text-ink-200">Presets</h3>
        <div className="space-y-2">
          {snapshot.presets.map((preset) => {
            const have = preset.files.filter((f) =>
              files.some((w) => w.filename === f && w.present),
            ).length;
            const complete = have === preset.files.length;
            return (
              <div key={preset.id} className="panel p-4">
                <div className="flex flex-wrap items-center justify-between gap-2">
                  <div className="flex items-center gap-2">
                    <span className="text-sm font-medium text-ink-100">
                      {preset.label}
                    </span>
                    {preset.fits_4gb && (
                      <span className="chip bg-mint-400/15 text-mint-400 ring-1 ring-mint-400/30">
                        fits 4 GB
                      </span>
                    )}
                    <span className="text-xs text-ink-500">
                      {formatBytes(preset.total_bytes)}
                    </span>
                  </div>
                  <div className="flex gap-2">
                    <button
                      onClick={() => loadScript(preset)}
                      className="chip bg-ink-800 text-ink-300 ring-1 ring-ink-700 hover:bg-ink-750"
                    >
                      script
                    </button>
                    <button
                      disabled={complete || busy !== null}
                      onClick={async () => {
                        for (const name of preset.files) {
                          const file = files.find((w) => w.filename === name);
                          if (file && !file.present) await begin(file);
                        }
                      }}
                      className="chip bg-glow-500/20 text-glow-300 ring-1 ring-glow-500/40 hover:brightness-110 disabled:opacity-40"
                    >
                      {complete ? "complete" : `fetch ${preset.files.length - have}`}
                    </button>
                  </div>
                </div>
                {preset.description && (
                  <p className="mt-2 text-xs leading-relaxed text-ink-500">
                    {preset.description}
                  </p>
                )}
                <ul className="mt-3 space-y-1">
                  {preset.files.map((name) => {
                    const file = files.find((w) => w.filename === name);
                    const present = file?.present ?? false;
                    return (
                      <li key={name} className="flex items-center gap-2 font-mono text-[11px]">
                        <span className={present ? "text-mint-400" : "text-ink-600"}>
                          {present ? "●" : "○"}
                        </span>
                        <span className="text-ink-300">{name}</span>
                      </li>
                    );
                  })}
                </ul>
              </div>
            );
          })}
        </div>
      </div>

      {script && (
        <div className="panel bg-ink-900 p-4">
          <div className="mb-2 flex items-center justify-between">
            <span className="text-xs font-medium text-ink-300">
              Run in your terminal
            </span>
            <button
              className="chip bg-ink-800 text-ink-200 ring-1 ring-ink-700 hover:bg-ink-750"
              onClick={() => void navigator.clipboard.writeText(script)}
            >
              Copy
            </button>
          </div>
          <pre className="overflow-x-auto text-[11px] leading-relaxed text-ink-200">
            {script}
          </pre>
        </div>
      )}

      {/* Catalogue */}
      <div className="space-y-2">
        <div className="flex items-center justify-between">
          <h3 className="text-sm font-medium text-ink-200">
            Catalogue <span className="text-ink-500">({filtered.length})</span>
          </h3>
          {filtered.length > 40 && (
            <button
              onClick={() => setShowAll((v) => !v)}
              className="text-xs text-ink-400 hover:text-ink-200"
            >
              {showAll ? "show less" : `show all ${filtered.length}`}
            </button>
          )}
        </div>

        <div className="space-y-1.5">
          {shown.map((file) => (
            <FileRow
              key={file.filename}
              file={file}
              progress={progress[file.filename]}
              busy={busy === file.filename}
              onDownload={() => begin(file)}
              onCancel={() => cancel(file)}
            />
          ))}
          {shown.length === 0 && (
            <p className="py-6 text-center text-xs text-ink-500">
              Nothing matches those filters.
            </p>
          )}
        </div>
      </div>
    </section>
  );
}

function Toggle({
  active,
  onClick,
  label,
}: {
  active: boolean;
  onClick: () => void;
  label: string;
}) {
  return (
    <button
      onClick={onClick}
      className={`chip px-2.5 py-1 transition ${
        active
          ? "bg-glow-500/20 text-glow-300 ring-1 ring-glow-500/45"
          : "bg-ink-800 text-ink-300 ring-1 ring-ink-700 hover:bg-ink-750"
      }`}
    >
      {active ? "✓ " : ""}
      {label}
    </button>
  );
}

function FileRow({
  file,
  progress,
  busy,
  onDownload,
  onCancel,
}: {
  file: WeightFile;
  progress?: DownloadStatus;
  busy: boolean;
  onDownload: () => void;
  onCancel: () => void;
}) {
  const running = progress?.state === "running";
  const done = file.present;
  const pct =
    progress && progress.total > 0
      ? Math.min(100, (progress.received / progress.total) * 100)
      : done
        ? 100
        : 0;

  return (
    <div className="panel p-3">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2">
            <span
              className={`size-1.5 shrink-0 rounded-full ${
                done ? "bg-mint-400" : running ? "bg-glow-400 pulsing" : "bg-ink-600"
              }`}
            />
            <span className="truncate font-mono text-[11px] text-ink-100">
              {file.filename}
            </span>
          </div>
          <div className="mt-1 flex flex-wrap items-center gap-1.5">
            <Tag>{roleLabel(file.role)}</Tag>
            <Tag>{file.variant}</Tag>
            <Tag>{file.quant}</Tag>
            {!file.backend_supported && (
              <span className="chip bg-amber-400/15 text-amber-300 ring-1 ring-amber-400/30">
                not in backend registry
              </span>
            )}
            {file.size_mismatch && (
              <span
                className="chip bg-rose-500/15 text-rose-300 ring-1 ring-rose-500/30"
                title="This file is on disk but is not the size it should be. It is most likely a partial download. The server will pick it by name and then fail to load it, so it has been excluded from the model pickers. Download it again."
              >
                truncated — re-download
              </span>
            )}
            {file.partial_bytes > 0 && !done && (
              <span className="chip bg-ink-800 text-ink-400 ring-1 ring-ink-700">
                {formatBytes(file.partial_bytes)} fetched
              </span>
            )}
          </div>
        </div>

        <div className="flex shrink-0 items-center gap-2">
          <span className="text-[11px] text-ink-500">
            {formatBytes(file.size_bytes)}
          </span>
          {done ? (
            <span className="chip bg-mint-400/15 text-mint-400 ring-1 ring-mint-400/30">
              installed
            </span>
          ) : running ? (
            <button
              onClick={onCancel}
              className="chip bg-ink-800 text-ink-200 ring-1 ring-ink-700 hover:bg-ink-750"
            >
              cancel
            </button>
          ) : (
            <button
              onClick={onDownload}
              disabled={busy}
              className="chip bg-glow-500/20 text-glow-300 ring-1 ring-glow-500/40 hover:brightness-110 disabled:opacity-40"
            >
              {file.partial_bytes > 0 ? "resume" : "download"}
            </button>
          )}
        </div>
      </div>

      {(running || pct > 0) && (
        <div className="mt-2 space-y-1">
          <div className="h-1 overflow-hidden rounded-full bg-ink-700">
            <div
              className="h-full rounded-full bg-glow-500 transition-[width] duration-300"
              style={{ width: `${pct}%` }}
            />
          </div>
          <div className="flex justify-between text-[10px] text-ink-500">
            <span>
              {formatBytes(progress?.received ?? file.partial_bytes)} /{" "}
              {formatBytes(file.size_bytes)}
            </span>
            {running && progress?.rate_bps ? (
              <span>
                {formatBytes(progress.rate_bps)}/s
                {progress.eta_secs != null &&
                  ` · ${Math.round(progress.eta_secs)}s left`}
              </span>
            ) : null}
          </div>
        </div>
      )}

      {progress?.state === "failed" && progress.error && (
        <p className="mt-2 text-[11px] text-rose-300">{progress.error}</p>
      )}
    </div>
  );
}

function Tag({ children }: { children: React.ReactNode }) {
  return (
    <span className="chip bg-ink-800 text-ink-400 ring-1 ring-ink-700">
      {children}
    </span>
  );
}