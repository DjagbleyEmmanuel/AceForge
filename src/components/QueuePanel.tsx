import { useCallback, useEffect, useState } from "react";
import { queue, renderRatio } from "../lib/api";
import { readFlag, writeFlag } from "../lib/prefs";
import type { JobState, QueuedJob } from "../lib/types";

interface Props {
  /** Fired when a job finishes so the library can pick up new tracks. */
  onGenerated: () => void;
}

const STATE_STYLE: Record<JobState, string> = {
  queued: "bg-ink-800 text-ink-300 ring-ink-700",
  running: "bg-glow-500/20 text-glow-300 ring-glow-500/45",
  done: "bg-emerald-500/15 text-emerald-300 ring-emerald-500/40",
  failed: "bg-rose-500/15 text-rose-300 ring-rose-500/40",
  cancelled: "bg-ink-800 text-ink-500 ring-ink-700",
  interrupted: "bg-amber-500/15 text-amber-300 ring-amber-500/40",
};

const STATE_LABEL: Record<JobState, string> = {
  queued: "queued",
  running: "running",
  done: "done",
  failed: "failed",
  cancelled: "cancelled",
  interrupted: "interrupted",
};

/** Truncated caption for the row. Mirrors `QueuedJob::label` on the Rust side;
 *  `label` is a method, so it is not present in the serialised job. */
function jobLabel(job: QueuedJob): string {
  // Mirrors QueuedJob::label: a YuE2 row is named by its title or style, since
  // it has no caption. See the Rust side for why.
  const yue2 = job.yue2_params;
  const source =
    job.backend === "yue2"
      ? yue2?.title.trim() || yue2?.style.trim() || "YuE2 render"
      : job.params.caption.trim();
  const base = source || "(no caption)";
  return [...base].length > 60 ? `${[...base].slice(0, 59).join("")}…` : base;
}

function clock(secs: number): string {
  if (secs < 1) return "—";
  if (secs < 60) return `${Math.round(secs)}s`;
  const m = Math.floor(secs / 60);
  return `${m}m ${Math.round(secs % 60)}s`;
}

export function QueuePanel({ onGenerated }: Props) {
  const [jobs, setJobs] = useState<QueuedJob[]>([]);
  const [stages, setStages] = useState<Record<string, string>>({});
  const [collapsed, setCollapsedRaw] = useState(() => readFlag("queue.collapsed", false));
  const setCollapsed = useCallback((v: boolean) => {
    setCollapsedRaw(v);
    writeFlag("queue.collapsed", v);
  }, []);

  const refresh = useCallback(async () => {
    setJobs(await queue.list());
  }, []);

  useEffect(() => {
    void refresh();
    let alive = true;
    // A stalled job has to become visible as failed without the user having to
    // click anything, so the list is driven by backend events rather than by a
    // poll of its own.
    const unlistenChange = queue.onChanged(() => {
      if (alive) void refresh();
    });
    const unlistenStage = queue.onStage((jobId, stage, detail) => {
      if (!alive) return;
      setStages((prev) => ({ ...prev, [jobId]: detail || stage }));
      if (stage === "done" || stage === "error") void onGenerated();
    });
    return () => {
      alive = false;
      void unlistenChange.then((f) => f());
      void unlistenStage.then((f) => f());
    };
  }, [refresh, onGenerated]);

  // Newest first: the job you just queued is the one you are watching.
  const ordered = [...jobs].reverse();
  const active = jobs.filter((j) => j.state === "running" || j.state === "queued");
  const interrupted = jobs.filter((j) => j.state === "interrupted");
  const doneCount = jobs.filter((j) => j.state === "done").length;
  const failedCount = jobs.filter((j) => j.state === "failed").length;

  // Nothing to show and nothing to collapse: an empty queue gets the invitation
  // rather than a one-line header that collapses into itself.
  if (ordered.length === 0) {
    return (
      <div className="panel p-8 text-center">
        <p className="text-sm text-ink-400">The queue is empty.</p>
        <p className="mt-1 text-xs text-ink-500">
          Generating adds a row here. Jobs run one at a time and survive a
          restart.
        </p>
      </div>
    );
  }

  return (
    <div className="panel overflow-hidden">
      <div className="flex flex-wrap items-center justify-between gap-2 px-4 py-2.5">
        <button
          onClick={() => setCollapsed(!collapsed)}
          className="flex items-center gap-2 text-left"
          aria-expanded={!collapsed}
        >
          <Caret open={!collapsed} />
          <span className="label">Queue</span>
          <span className="text-[11px] text-ink-500">
            {active.length} pending · {doneCount} done
            {failedCount > 0 && ` · ${failedCount} failed`}
          </span>
        </button>

        {!collapsed && (
          <div className="flex flex-wrap gap-1.5">
            <button
              onClick={() => void queue.clearFailed()}
              disabled={failedCount === 0}
              className="chip px-2 py-1 text-[11px] disabled:opacity-40"
              title="Remove failed rows from the list. The files, if any, stay on disk."
            >
              Clear failed
            </button>
            <button
              onClick={() => void queue.clearDone()}
              disabled={doneCount === 0}
              className="chip px-2 py-1 text-[11px] disabled:opacity-40"
              title="Remove every finished row: done, failed and cancelled. The files stay on disk."
            >
              Clear done
            </button>
            <button
              onClick={() => void queue.reset()}
              disabled={active.length === 0}
              className="chip px-2 py-1 text-[11px] disabled:opacity-40"
              title="Cancel everything running and pending"
            >
              Reset
            </button>
          </div>
        )}
      </div>

      {collapsed ? null : (
        <>
          {interrupted.length > 0 && (
            <div className="border-t border-amber-500/25 bg-amber-500/5 px-4 py-3">
              <p className="text-xs text-amber-300">
                {interrupted.length} job{interrupted.length > 1 ? "s" : ""} were
                interrupted when the app closed. Nothing restarts on its own — retry
                the ones you still want.
              </p>
            </div>
          )}

          <ul className="divide-y divide-ink-800 border-t border-ink-800">
            {ordered.map((job) => {
              const ratio = renderRatio(job);
              const stage = stages[job.id];
              return (
                <li key={job.id} className="flex items-start gap-3 px-4 py-3">
                  <span
                    className={`mt-0.5 shrink-0 rounded px-1.5 py-0.5 text-[10px] ring-1 ${
                      STATE_STYLE[job.state]
                    }`}
                  >
                    {STATE_LABEL[job.state]}
                  </span>

                  <div className="min-w-0 flex-1">
                    <p
                      className="truncate text-xs text-ink-200"
                      title={
                        job.backend === "yue2"
                          ? job.yue2_params?.title || job.yue2_params?.style
                          : job.params.caption
                      }
                    >
                      {jobLabel(job)}
                    </p>
                    <p className="mt-0.5 text-[11px] text-ink-500">
                      {job.state === "running" && stage ? (
                        <span className="text-glow-400">{stage}</span>
                      ) : job.error ? (
                        <span className="text-rose-400">{job.error}</span>
                      ) : (
                        <>
                          {clock(job.elapsed_secs)}
                          {ratio !== null && (
                            <>
                              {" · "}
                              <span className="text-ink-400">
                                {ratio.toFixed(1)}×
                              </span>
                              {" realtime"}
                            </>
                          )}
                        </>
                      )}
                    </p>
                  </div>

                  <div className="flex shrink-0 gap-1.5">
                    {job.state === "interrupted" && (
                      <>
                        <button
                          onClick={() => void queue.retry(job.id)}
                          className="chip px-2 py-1 text-[11px]"
                          title="Queue this job again with the same settings"
                        >
                          Retry
                        </button>
                        <button
                          onClick={() => void queue.discard(job.id)}
                          className="chip px-2 py-1 text-[11px]"
                          title="Remove this row without running it"
                        >
                          Discard
                        </button>
                      </>
                    )}
                    {job.state === "failed" && (
                      <button
                        onClick={() => void queue.retry(job.id)}
                        className="chip px-2 py-1 text-[11px]"
                        title="Queue this job again with the same settings"
                      >
                        Retry
                      </button>
                    )}
                    {(job.state === "queued" || job.state === "running") && (
                      <button
                        onClick={() => void queue.cancel(job.id)}
                        className="chip px-2 py-1 text-[11px]"
                        title="Stop this job"
                      >
                        Cancel
                      </button>
                    )}
                  </div>
                </li>
              );
            })}
          </ul>
        </>
      )}
    </div>
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