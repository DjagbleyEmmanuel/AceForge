import { useEffect, useMemo, useRef, useState } from "react";
import { lyricsApi, type LyricLine, type LyricTimings } from "../lib/api";

interface Props {
  trackPath: string;
  /** Raw lyrics, used to derive the lines when nothing is timed yet. */
  lyrics: string;
  /** Playback position in seconds. */
  currentTime: number;
}

/** Split a lyrics blob into displayable lines. */
export function lyricLines(raw: string): string[] {
  return raw
    .split(/\r?\n/)
    .map((l) => l.replace(/^\s*\[[a-z_]+\]\s*/i, "").trim())
    .filter((l) => l.length > 0 && l.toLowerCase() !== "[instrumental]");
}

/** Index of the line that should be lit at `time`, or -1 before the first. */
export function activeLineIndex(lines: LyricLine[], time: number): number {
  let found = -1;
  for (let i = 0; i < lines.length; i++) {
    if (lines[i].atSecs <= time + 0.05) found = i;
    else break;
  }
  return found;
}

/**
 * Lyrics that follow the playback position.
 *
 * Timings come from a `.lrc` beside the audio if there is one, otherwise from
 * taps the user makes. The engine emits no lyric timestamps of its own --
 * acestep.cpp has no such code path -- so this cannot be derived from a render.
 * When neither exists the lyrics are shown as plain text rather than advancing
 * on a guess, because a highlight that is subtly out of time is worse than no
 * highlight: it looks authoritative and is not.
 */
export function SyncedLyrics({
  trackPath,
  lyrics,
  currentTime,
}: Props) {
  const [timings, setTimings] = useState<LyricTimings | null>(null);
  const [tapping, setTapping] = useState(false);
  const [draft, setDraft] = useState<{ at: number; text: string }[]>([]);
  const listRef = useRef<HTMLOListElement>(null);
  const activeRef = useRef<HTMLLIElement>(null);

  const plain = useMemo(() => lyricLines(lyrics), [lyrics]);

  useEffect(() => {
    let alive = true;
    setTimings(null);
    setTapping(false);
    setDraft([]);
    void lyricsApi
      .timings(trackPath)
      .then((t) => {
        if (alive) setTimings(t);
      })
      .catch(() => {
        if (alive) setTimings(null);
      });
    return () => {
      alive = false;
    };
  }, [trackPath]);

  /** Entering tap mode seeds from the plain text so every line needs a time. */
  function startTapping() {
    setDraft(plain.map((text) => ({ at: -1, text })));
    setTapping(true);
    setTimings(null);
  }

  async function saveTaps() {
    const lines = draft
      .filter((l) => l.at >= 0)
      .map((l) => ({ atSecs: l.at, text: l.text }));
    if (lines.length === 0) {
      setTapping(false);
      setDraft([]);
      return;
    }
    try {
      await lyricsApi.save(trackPath, lines);
      setTimings({ lines, source: "manual" });
      setTapping(false);
      setDraft([]);
    } catch {
      // Leave the draft in place so the taps are not lost on a failed write.
    }
  }

  // While tap mode is open, M marks the next untimed line at the playhead.
  // Scoped to this component because every library card keeps one mounted, and
  // a document-wide listener would stamp a time onto tracks the user cannot
  // see.
  useEffect(() => {
    if (!tapping) return;
    const onKey = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement | null;
      if (target && /^(INPUT|TEXTAREA)$/.test(target.tagName)) return;
      if (e.key !== "m" && e.key !== "M") return;
      e.preventDefault();
      setDraft((d) => {
        const next = [...d];
        const at = next.findIndex((l) => l.at < 0);
        // Nothing untimed left: let the last line absorb repeats rather than
        // doing nothing, which is what someone holding M at the end expects.
        const idx = at >= 0 ? at : next.length - 1;
        if (idx < 0) return d;
        next[idx] = { ...next[idx], at: currentTime };
        return next;
      });
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [tapping, currentTime]);

  // Keep the current line visible without yanking the page: scroll only the
  // list's own container.
  useEffect(() => {
    activeRef.current?.scrollIntoView({ block: "center", behavior: "smooth" });
  }, [timings, currentTime]);

  if (plain.length === 0) return null;

  if (tapping) {
    return (
      <div className="space-y-2 rounded-lg bg-ink-900/70 p-3 ring-1 ring-ink-800">
        <p className="text-[11px] leading-relaxed text-ink-400">
          Play the song and press <kbd className="rounded bg-ink-800 px-1">M</kbd>{" "}
          each time a line starts. Untimed lines are skipped when saving.
        </p>
        <ul className="max-h-48 space-y-1 overflow-auto">
          {draft.map((line, i) => (
            <li key={i} className="flex items-center gap-2 text-[11px]">
              <span className="w-12 shrink-0 font-mono text-ink-500">
                {line.at >= 0 ? `${line.at.toFixed(1)}s` : "—"}
              </span>
              <span
                className={
                  line.at >= 0 ? "min-w-0 flex-1 truncate text-ink-200" : "min-w-0 flex-1 truncate text-ink-500"
                }
              >
                {line.text}
              </span>
              <button
                onClick={() => setDraft((d) => d.map((l, j) => (j === i ? { ...l, at: currentTime } : l)))}
                className="chip shrink-0 px-1.5 py-0.5 text-[10px]"
                title="Set this line to the playhead"
              >
                set
              </button>
            </li>
          ))}
        </ul>
        <div className="flex justify-end gap-2">
          <button onClick={() => setTapping(false)} className="chip px-2 py-1 text-[11px]">
            cancel
          </button>
          <button onClick={() => void saveTaps()} className="chip px-2 py-1 text-[11px]">
            save timings
          </button>
        </div>
      </div>
    );
  }

  if (!timings) {
    return (
      <details className="group">
        <summary className="cursor-pointer text-xs text-ink-400 hover:text-ink-200">
          Lyrics
        </summary>
        <pre className="mt-2 max-h-48 overflow-auto whitespace-pre-wrap rounded-lg bg-ink-900 p-3 font-mono text-xs leading-relaxed text-ink-200">
          {plain.join("\n")}
        </pre>
        <button
          onClick={startTapping}
          className="mt-2 text-[11px] text-glow-300 underline-offset-2 hover:underline"
        >
          make these follow the music
        </button>
      </details>
    );
  }

  const active = activeLineIndex(timings.lines, currentTime);

  return (
    <div className="space-y-1.5">
      <div className="flex items-center justify-between">
        <span className="text-[11px] text-ink-400">Lyrics</span>
        <button
          onClick={startTapping}
          className="text-[10px] text-ink-500 underline-offset-2 hover:text-ink-300 hover:underline"
        >
          re-time
        </button>
      </div>
      <ol
        ref={listRef}
        className="max-h-40 space-y-0.5 overflow-auto rounded-lg bg-ink-900 p-2"
      >
        {timings.lines.map((line, i) => {
          const isActive = i === active;
          const isPast = i < active;
          return (
            <li
              key={`${line.atSecs}-${i}`}
              ref={isActive ? activeRef : undefined}
              className={`rounded px-1.5 py-0.5 text-xs transition ${
                isActive
                  ? "bg-glow-500/20 font-medium text-glow-200"
                  : isPast
                    ? "text-ink-500"
                    : "text-ink-400"
              }`}
            >
              {line.text}
            </li>
          );
        })}
      </ol>
      {timings.source === "sidecar" ? (
        <p className="text-[10px] text-ink-600">timed from the .lrc beside the audio</p>
      ) : (
        <p className="text-[10px] text-ink-600">timed by you</p>
      )}
    </div>
  );
}