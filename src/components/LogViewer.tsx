import { useCallback, useEffect, useRef, useState } from "react";
import { server } from "../lib/api";
import { readFlag, writeFlag } from "../lib/prefs";

/** How often to ask for new lines while following. */
const POLL_MS = 1000;
/** Lines kept in view. Older ones are dropped, not the whole file re-sent. */
const MAX_LINES = 2000;

interface Props {
  running: boolean;
}

/**
 * The sidecar's stderr, in the app.
 *
 * This log is the only place the backend explains itself. A failed job record
 * carries no error at all -- `Job` in `ace-server.cpp` has nowhere to put one
 * -- so before this, the only way to find out why a render died was to open a
 * terminal and tail a file in the models directory.
 *
 * Polling resumes from a byte cursor rather than re-reading the file, so a
 * long-running render costs one small IPC call per second.
 */
export function LogViewer({ running }: Props) {
  const [lines, setLines] = useState<string[]>([]);
  const [follow, setFollowRaw] = useState(() => readFlag("log.follow", true));
  const setFollow = useCallback((v: boolean) => {
    setFollowRaw(v);
    writeFlag("log.follow", v);
  }, []);
  const [filter, setFilter] = useState("");
  const [cursor, setCursor] = useState(0);

  const refresh = useCallback(async () => {
    try {
      const [fresh, end] = await Promise.all([
        server.logs(MAX_LINES),
        server.cursor(),
      ]);
      setLines(fresh);
      setCursor(end);
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
    if (!running || !follow) return;
    let alive = true;
    const tick = async () => {
      try {
        const at = await server.cursor();
        if (!alive) return;
        if (at < cursor) {
          // The log shrank: it was replaced, so start over rather than
          // resuming from a cursor that no longer means anything.
          await refresh();
          return;
        }
        if (at === cursor) return;
        const added = await server.logsSince(cursor, MAX_LINES);
        if (!alive || added.length === 0) return;
        setCursor(at);
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
  }, [running, follow, cursor, refresh]);

  const boxRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!follow) return;
    const el = boxRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [lines, follow]);

  const needle = filter.trim().toLowerCase();
  const shown = needle ? lines.filter((l) => l.toLowerCase().includes(needle)) : lines;

  return (
    <section className="panel space-y-3 p-4">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h3 className="text-sm font-medium text-ink-200">Backend log</h3>
        <div className="flex items-center gap-2">
          <input
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            placeholder="filter"
            className="w-28 rounded border border-ink-700 bg-ink-900 px-2 py-1 text-[11px] text-ink-200 outline-none focus:border-glow-500/60"
          />
          <button
            onClick={() => setFollow(!follow)}
            className={`chip ${
              follow
                ? "bg-glow-500/15 text-glow-300 ring-1 ring-glow-500/40"
                : "bg-ink-800 text-ink-400 ring-1 ring-ink-700"
            }`}
          >
            {follow ? "following" : "paused"}
          </button>
          <button onClick={() => void refresh()} className="btn btn-ghost px-2 py-1 text-[10px]">
            reload
          </button>
        </div>
      </div>

      <div
        ref={boxRef}
        className="max-h-80 overflow-y-auto overscroll-contain rounded-lg bg-ink-950 p-2.5 font-mono text-[11px] leading-relaxed"
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
              ? "Nothing logged yet. Start the server to see backend output."
              : `No lines match “${filter}”.`}
          </p>
        ) : (
          shown.map((line, i) => (
            <div
              key={`${i}-${line.slice(0, 24)}`}
              className={
                /FATAL|ERROR/i.test(line)
                  ? "text-rose-300"
                  : /WARNING/i.test(line)
                    ? "text-amber-300"
                    : "text-ink-400"
              }
            >
              {line}
            </div>
          ))
        )}
      </div>

      <p className="text-[10px] text-ink-500">
        {shown.length} of {lines.length} lines
        {needle && " (filtered)"}
      </p>
    </section>
  );
}
