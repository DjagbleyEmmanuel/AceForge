
import { library, type Track } from "../lib/api";
import { TrackPlayer } from "./TrackPlayer";

interface Props {
  tracks: Track[];
  onChanged: () => void;
}

export function LibraryPanel({ tracks, onChanged }: Props) {
  // `tracks` is the single source of truth: this panel stays mounted now, so a
  // local mirror would go stale instead of re-seeding on remount.
  const items = tracks;

  async function refresh() {
    await onChanged();
  }

  async function remove(track: Track) {
    try {
      await library.remove(track.path);
      await refresh();
    } catch {
      // A failed delete leaves the list unchanged; the next scan re-reads disk
      // anyway, so just resync.
      await refresh();
    }
  }

  if (items.length === 0) {
    return (
      <div className="panel p-10 text-center">
        <p className="text-sm text-ink-400">No tracks yet.</p>
        <p className="mt-1 text-xs text-ink-500">
          Anything you generate lands here and stays on disk.
        </p>
      </div>
    );
  }

  return (
    <div className="space-y-3">
      {items.map((track) => (
        <div key={track.path} className="panel p-4">
          <TrackPlayer track={track} />

          {track.lyrics && (
            <details className="group mt-3">
              <summary className="cursor-pointer text-xs text-ink-400 hover:text-ink-200">
                Lyrics
              </summary>
              <pre className="mt-2 max-h-48 overflow-auto whitespace-pre-wrap rounded-lg bg-ink-900 p-3 font-mono text-xs leading-relaxed text-ink-200">
                {track.lyrics}
              </pre>
            </details>
          )}

          <div className="mt-3 flex justify-end gap-2">
            <button
              onClick={() => void library.reveal(track.path)}
              className="btn btn-ghost px-3 py-1 text-xs"
            >
              Show file
            </button>
            <button
              onClick={() => void remove(track)}
              className="btn btn-ghost px-3 py-1 text-xs hover:text-rose-300"
            >
              Delete
            </button>
          </div>
        </div>
      ))}
    </div>
  );
}