import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { server } from "../lib/api";
import { readFlag, writeFlag } from "../lib/prefs";
import { CATEGORIES, categorise, type LogCategory } from "../lib/logcat";

/** How often to ask for new lines while following. */
const POLL_MS = 1000;
/** Lines kept in view. Older ones are dropped, not the whole file re-sent. */
const MAX_LINES = 2000;

interface Props {
  running: boolean;
}

/**
 * The sidecar's stderr, in the app, dressed as a terminal.
 *
 * This log is the only place the backend explains itself. A failed job record
 * carries no error at all -- `Job` in `ace-server.cpp` has nowhere to put one --
 * so before this, the only way to find out why a render died was to open a
 * terminal and tail a file in the models directory.
 *
 * Polling resumes from a byte cursor rather than re-reading the file, so a
 * long-running render costs one small IPC call per second.
 */
export function LogViewer({ running }: Props) {
  const [lines, setLines] = useState<string[]>([]);
  const [follow, setFollowRaw] = useState(() => readFlag("log.follow", true));
  const [collapsed, setCollapsed] = useState(() => readFlag("log.collapsed", false));
  const [busy, setBusy] = useState(false);
  const [clearNote, setClearNote] = useState("");

  const setFollow = useCallback((v: boolean) => {
    setFollowRaw(v);
    writeFlag("log.follow", v);
  }, []);
  const setCollapsedFlag = useCallback((v: boolean) => {
    setCollapsed(v);
    writeFlag("log.collapsed", v);
  }, []);

  const [filter, setFilter] = useState("");
  const [category, setCategory] = useState<LogCategory | "all">("all");

  /**
   * The byte offset lives in a ref, not in state. It advances on every tick,
   * and as state it was a dependency of the polling effect -- so the interval
   * was torn down and rebuilt once a second, forever. The value is read by the
   * timer and written by the timer; nothing renders it.
   */
  const cursor = useRef(0);

  const refresh = useCallback(async () => {
    try {
      const [fresh, end] = await Promise.all([server.logs(MAX_LINES), server.cursor()]);
      cursor.current = end;
      setLines(fresh);
    } catch {
      // The server may not have started yet; the next tick will pick it up.
    }
  }, []);

  // Full reload when the log is opened or the server restarts: a restart
  // truncates nothing, but the cursor is meaningless across one.
  useEffect(() => {
    void refresh();
  }, [refresh, running]);

  useEffect(() => {
    if (!running || !follow || collapsed) return;
    let alive = true;
    const tick = async () => {
      try {
        const at = await server.cursor();
        if (!alive) return;
        if (at < cursor.current) {
          // The log shrank: it was replaced, so start over rather than
          // resuming from a cursor that no longer means anything.
          await refresh();
          return;
        }
        if (at === cursor.current) return;
        const added = await server.logsSince(cursor.current, MAX_LINES);
        if (!alive || added.length === 0) return;
        cursor.current = at;
        setLines((prev) => {
          const next = [...prev, ...added];
          return next.length > MAX_LINES ? next.slice(next.length - MAX_LINES) : next;
        });
      } catch {
        // Transient; the next tick retries.
      }
    };
    const id = window.setInterval(() => void tick(), POLL_MS);
    return () => {
      alive = false;
      window.clearInterval(id);
    };
  }, [running, follow, collapsed, refresh]);

  const clearAll = useCallback(async () => {
    setBusy(true);
    try {
      const at = await server.clearLogs();
      cursor.current = at;
      setLines([]);
      setClearNote("cleared");
      window.setTimeout(() => setClearNote(""), 1500);
    } catch {
      setClearNote("could not clear");
    } finally {
      setBusy(false);
    }
  }, []);

  const boxRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!follow || collapsed) return;
    const el = boxRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [lines, follow, collapsed]);

  // Pausing is not the same as going back to the top: the box was left scrolled
  // to the bottom, and a category chosen from there opens at the end of it,
  // which is the same problem again by a different route. Read a chosen
  // category from its first line.
  useEffect(() => {
    if (follow) return;
    const el = boxRef.current;
    if (el) el.scrollTop = 0;
  }, [category, filter, follow, collapsed]);

  const needle = filter.trim().toLowerCase();
  /**
   * Filtered and classified in one pass. Recomputing per render is what this
   * avoids: with 2000 lines in view and a poll every second, two separate
   * `filter` walks were the difference between a panel that keeps up and one
   * that stutters while something else renders.
   */
  const shown = useMemo(() => {
    const out: { line: string; kind: LogCategory }[] = [];
    for (const line of lines) {
      const kind = categorise(line);
      if (category !== "all" && kind !== category) continue;
      if (needle && !line.toLowerCase().includes(needle)) continue;
      out.push({ line, kind });
    }
    return out;
  }, [lines, category, needle]);

  /** How many lines each bucket holds, for the picker. */
  const counts = useMemo(() => {
    const tally: Partial<Record<LogCategory, number>> = {};
    for (const line of lines) {
      const kind = categorise(line);
      tally[kind] = (tally[kind] ?? 0) + 1;
    }
    return tally;
  }, [lines]);

  /**
   * Picking a category is a request to read that category, and following the
   * tail is the opposite of it: the scroll effect below snaps to the bottom, so
   * the first thing you saw was the last few lines and everything above them
   * -- which is where a long warning starts -- was somewhere off screen. The
   * same applies to typing in the filter.
   *
   * Following resumes from the paused button, which is where it already was in
   * the code for scrolling up.
   */
  const pick = useCallback(
    (next: LogCategory | "all") => {
      setCategory(next);
      if (next !== "all") setFollow(false);
    },
    [setFollow],
  );

  const summary =
    category === "all" && !needle
      ? `${lines.length} lines`
      : `${shown.length} of ${lines.length}`;

  return (
    <section className="panel overflow-hidden">
      <div className="flex flex-wrap items-center justify-between gap-2 px-4 py-2.5">
        <button
          onClick={() => setCollapsedFlag(!collapsed)}
          className="flex items-center gap-2 text-left"
          aria-expanded={!collapsed}
        >
          <Caret open={!collapsed} />
          <span className="text-sm font-medium text-ink-200">Logs</span>
          <span className="text-[11px] text-ink-500">{collapsed ? summary : "backend stderr"}</span>
        </button>

        {!collapsed && (
          <div className="flex flex-wrap items-center gap-1.5">
            <input
              value={filter}
              onChange={(e) => {
                setFilter(e.target.value);
                setFollow(false);
              }}
              placeholder="filter"
              className="w-24 rounded border border-ink-700 bg-ink-900 px-2 py-1 font-mono text-[11px] text-ink-200 outline-none focus:border-glow-500/60"
            />
            <button
              onClick={() => setFollow(!follow)}
              className={`chip px-2 py-1 text-[10px] ${
                follow
                  ? "bg-glow-500/15 text-glow-300 ring-1 ring-glow-500/40"
                  : "bg-ink-800 text-ink-400 ring-1 ring-ink-700"
              }`}
            >
              {follow ? "following" : "paused"}
            </button>
            <button onClick={() => void refresh()} className="chip px-2 py-1 text-[10px]">
              reload
            </button>
            <button
              onClick={() => void clearAll()}
              disabled={busy || lines.length === 0}
              className="chip px-2 py-1 text-[10px] disabled:opacity-40"
              title="Empty the backend log. The file is truncated in place, so the server keeps writing to it."
            >
              clear
            </button>
          </div>
        )}
      </div>

      {!collapsed && (
        <>
          {/* Category picker. "all" is the default and the counts are what make
              it worth having: the error bucket on a real log is five lines in
              two thousand, and it is all five that matter. */}
          <div className="flex flex-wrap items-center gap-1 border-t border-ink-800 px-4 py-2">
            <CategoryChip
              label="all"
              count={lines.length}
              active={category === "all"}
              onClick={() => pick("all")}
            />
            {CATEGORIES.map(({ id, label }) => {
              const n = counts[id] ?? 0;
              return (
                <CategoryChip
                  key={id}
                  label={label}
                  count={n}
                  active={category === id}
                  empty={n === 0}
                  onClick={() => pick(id)}
                />
              );
            })}
          </div>

          <div
            ref={boxRef}
            className="term-scroll max-h-96 overflow-y-auto overscroll-contain border-t border-ink-800 bg-ink-950 px-3 py-2 font-mono text-[11px] leading-[1.55]"
            onScroll={(e) => {
              // Scrolling up is an implicit pause: yanking the view back to the
              // bottom while someone is reading a stack trace is infuriating.
              const el = e.currentTarget;
              const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 24;
              if (!atBottom && follow) setFollow(false);
            }}
          >
            {shown.length === 0 ? (
              <p className="text-ink-600">
                {lines.length === 0
                  ? clearNote === "cleared"
                    ? "Log cleared. New backend output appears here."
                    : "Nothing logged yet. Start the server to see backend output."
                  : `No lines in ${category === "all" ? "this log" : category}.`}
              </p>
            ) : (
              shown.map(({ line, kind }, i) => (
                <div
                  key={`${i}-${line.slice(0, 24)}`}
                  className={`term-line ${TERM_STYLE[kind] ?? TERM_STYLE.other}`}
                >
                  {line || " "}
                </div>
              ))
            )}
          </div>

          <p className="flex items-center gap-2 px-4 py-2 text-[10px] text-ink-500">
            <span>
              {summary}
              {needle && " (filtered)"}
            </span>
            {clearNote && <span className="text-mint-400">{clearNote}</span>}
          </p>
        </>
      )}
    </section>
  );
}

const TERM_STYLE: Record<LogCategory, string> = {
  error: "term-err",
  warn: "term-warn",
  memory: "term-mem",
  backend: "term-backend",
  audio: "term-audio",
  models: "term-models",
  generate: "term-gen",
  other: "text-ink-500",
};

function CategoryChip({
  label,
  count,
  active,
  empty,
  onClick,
}: {
  label: string;
  count: number;
  active: boolean;
  empty?: boolean;
  onClick: () => void;
}) {
  return (
    <button
      onClick={onClick}
      data-active={active}
      disabled={empty && !active}
      className={`chip px-2 py-0.5 font-mono text-[10px] disabled:opacity-30 ${
        active ? "bg-glow-500/15 text-glow-300 ring-1 ring-glow-500/40" : "text-ink-400"
      }`}
    >
      {label}
      <span className="ml-1 text-ink-600">{count}</span>
    </button>
  );
}

function Caret({ open }: { open: boolean }) {
  return (
    <svg
      width="10"
      height="10"
      viewBox="0 0 10 10"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.6"
      strokeLinecap="round"
      strokeLinejoin="round"
      className={`shrink-0 text-ink-500 transition-transform duration-150 ${open ? "rotate-90" : ""}`}
    >
      <path d="M3.5 1.5 7 5l-3.5 3.5" />
    </svg>
  );
}