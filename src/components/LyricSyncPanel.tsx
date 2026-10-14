import { useCallback, useEffect, useState } from "react";
import { lyricSync, type SyncProgress } from "../lib/api";

interface Props {
  /** True when the engine is up; the sync needs it. */
  running: boolean;
  /** Re-run after a batch so newly written sidecars disappear from `pending`. */
  onChanged: () => void;
}

/**
 * Backfill lyric timings for songs that predate the engine's alignment.
 *
 * Each song costs a short DiT pass whose audio is thrown away -- only the
 * timestamps are kept -- so the batch is minutes rather than hours. It runs one
 * track at a time because every pass acquires the DiT, and two at once would
 * fight over VRAM on a 4 GB card.
 */
export function LyricSyncPanel({ running, onChanged }: Props) {
  const [progress, setProgress] = useState<SyncProgress | null>(null);
  const [pending, setPending] = useState<{ path: string; name: string }[]>([]);
  const [showList, setShowList] = useState(false);

  const refresh = useCallback(async () => {
    try {
      const list = await lyricSync.pending();
      setPending(
        list.map((t) => ({
          path: t.path,
          name: t.caption || t.id,
        })),
      );
    } catch {
      setPending([]);
    }
  }, []);

  useEffect(() => {
    void refresh();
    void lyricSync.progress().then(setProgress).catch(() => setProgress(null));
    let alive = true;
    const unlisten = lyricSync.onProgress((p) => {
      if (!alive) return;
      setProgress(p);
      if (!p.running && p.done > 0) {
        void refresh();
        onChanged();
      }
    });
    return () => {
      alive = false;
      void unlisten.then((f) => f());
    };
  }, [refresh, onChanged]);

  const active = progress?.running ?? false;
  const total = progress?.total ?? pending.length;
  const done = progress?.done ?? 0;

  // Nothing to do is the common case and should not shout about itself.
  if (pending.length === 0 && !active) return null;

  return (
    <div className="panel space-y-2 p-4">
      <div className="flex items-center justify-between gap-2">
        <div className="min-w-0">
          <span className="label">Synced lyrics</span>
          <p className="mt-0.5 text-[11px] text-ink-500">
            {active ? (
              <>
                timing lyrics for{" "}
                <span className="text-glow-300">{progress?.current}</span>
              </>
            ) : (
              <>
                {pending.length} song{pending.length === 1 ? "" : "s"} have lyrics
                but no timings
              </>
            )}
          </p>
        </div>
        <div className="flex shrink-0 gap-1.5">
          {active ? (
            <button
              onClick={() => void lyricSync.cancel()}
              className="chip px-2 py-1 text-[11px] hover:text-rose-300"
            >
              stop
            </button>
          ) : (
            <button
              onClick={() => void lyricSync.start(pending.map((p) => p.path))}
              disabled={!running || pending.length === 0}
              title={
                !running
                  ? "Start the engine first"
                  : `Time the lyrics of ${pending.length} songs, about 13 seconds each`
              }
              className="chip px-2 py-1 text-[11px] disabled:opacity-40"
            >
              sync all
            </button>
          )}
        </div>
      </div>

      {active && total > 0 && (
        <div>
          <div className="h-1 overflow-hidden rounded-full bg-ink-800">
            <div
              className="h-full bg-glow-500 transition-[width] duration-300"
              style={{ width: `${Math.round((done / total) * 100)}%` }}
            />
          </div>
          <p className="mt-1 text-[10px] text-ink-500">
            {done} of {total} · the audio is discarded, only the timings are kept
          </p>
        </div>
      )}

      {progress?.error && (
        <p className="text-[11px] text-amber-400/90">{progress.error}</p>
      )}

      {!active && pending.length > 0 && (
        <div>
          <button
            onClick={() => setShowList((v) => !v)}
            className="text-[11px] text-ink-400 underline-offset-2 hover:text-ink-200 hover:underline"
          >
            {showList ? "hide" : "show"} which songs
          </button>
          {showList && (
            <ul className="mt-1.5 max-h-40 space-y-0.5 overflow-auto">
              {pending.map((p) => (
                <li
                  key={p.path}
                  className="flex items-center gap-2 text-[11px]"
                >
                  <button
                    onClick={() => void lyricSync.start([p.path])}
                    disabled={!running}
                    title="Time this one song"
                    className="chip shrink-0 px-1.5 py-0.5 text-[10px] disabled:opacity-40"
                  >
                    sync
                  </button>
                  <span className="min-w-0 flex-1 truncate text-ink-300">
                    {p.name}
                  </span>
                </li>
              ))}
            </ul>
          )}
        </div>
      )}
    </div>
  );
}