import { useEffect, useMemo, useState } from "react";
import { annotations, type SavedSeed } from "../lib/api";

interface Props {
  /** The seed currently in the form, offered for saving. */
  currentSeed: number;
  /** Load a seed back into the generation form. */
  onLoad: (seed: number) => void;
}

/**
 * Saved seeds.
 *
 * Seeds are only useful in pairs with their prompt, so every row shows the
 * caption it was saved with and loading a seed brings back both. Searching is
 * over the caption rather than the seed number because nobody remembers a
 * number they did not write down.
 */
export function SeedManager({ currentSeed, onLoad }: Props) {
  const [seeds, setSeeds] = useState<SavedSeed[]>([]);
  const [query, setQuery] = useState("");
  const [onlyStarred, setOnlyStarred] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    void annotations
      .seeds()
      .then((s) => {
        if (alive) setSeeds(s);
      })
      .catch((e) => {
        if (alive) setError(String(e));
      });
    return () => {
      alive = false;
    };
  }, []);

  const visible = useMemo(() => {
    const q = query.trim().toLowerCase();
    return seeds.filter((s) => {
      if (onlyStarred && !s.starred) return false;
      if (!q) return true;
      return s.caption.toLowerCase().includes(q) || String(s.seed).includes(q);
    });
  }, [seeds, query, onlyStarred]);

  const alreadySaved = seeds.some((s) => s.seed === currentSeed);

  return (
    <div className="panel space-y-3 p-4">
      <div className="flex items-center justify-between gap-2">
        <span className="label">Seeds</span>
        <button
          onClick={() =>
            void annotations.saveSeed(currentSeed, "").then(setSeeds)
          }
          disabled={currentSeed < 0 || alreadySaved}
          title={
            alreadySaved
              ? "This seed is already saved"
              : "Keep this seed so you can come back to it"
          }
          className="chip px-2 py-1 text-[11px] disabled:opacity-40"
        >
          {alreadySaved ? "saved" : `save ${currentSeed}`}
        </button>
      </div>

      {seeds.length > 0 && (
        <div className="flex items-center gap-2">
          <input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="search prompts or seeds"
            className="field min-w-0 flex-1 py-1 text-xs"
          />
          <button
            onClick={() => setOnlyStarred((v) => !v)}
            aria-pressed={onlyStarred}
            title="Only starred seeds"
            className={`chip px-2 py-1 text-[11px] ${
              onlyStarred
                ? "bg-amber-500/20 text-amber-300 ring-1 ring-amber-500/40"
                : ""
            }`}
          >
            starred
          </button>
        </div>
      )}

      {seeds.length === 0 && (
        <p className="text-[11px] text-ink-500">
          No saved seeds. Every render gets a concrete seed recorded on its queue
          row — save the ones worth keeping.
        </p>
      )}

      {seeds.length > 0 && visible.length === 0 && (
        <p className="text-[11px] text-ink-500">Nothing matches that search.</p>
      )}

      <ul className="max-h-56 space-y-1 overflow-auto">
        {visible.map((s) => (
          <li
            key={s.seed}
            className="flex items-center gap-2 rounded bg-ink-900 px-2 py-1.5"
          >
            <button
              onClick={() =>
                void annotations.toggleStar(s.seed).then(setSeeds)
              }
              aria-pressed={s.starred}
              title={s.starred ? "Unstar" : "Star"}
              className={`shrink-0 text-xs ${s.starred ? "text-amber-300" : "text-ink-600 hover:text-ink-400"}`}
            >
              {s.starred ? "★" : "☆"}
            </button>
            <button
              onClick={() => onLoad(s.seed)}
              title="Load this seed into the form"
              className="min-w-0 flex-1 text-left"
            >
              <span className="block truncate text-[11px] text-ink-200">
                {s.caption || "(no caption)"}
              </span>
              <span className="block font-mono text-[10px] text-ink-500">
                {s.seed}
              </span>
            </button>
            <button
              onClick={() => void annotations.removeSeed(s.seed).then(setSeeds)}
              title="Forget this seed"
              className="shrink-0 text-ink-600 hover:text-rose-300"
            >
              ×
            </button>
          </li>
        ))}
      </ul>

      {error && <p className="text-[11px] text-rose-300">{error}</p>}
    </div>
  );
}