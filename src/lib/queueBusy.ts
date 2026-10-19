import { useEffect, useState } from "react";
import { queue } from "./api";

/**
 * Whether the runner has work in hand.
 *
 * Exists so anything that talks to the engine can stand down while a render is
 * on. The engine serves one request at a time per worker, so a poll issued
 * during a render does not fail fast -- it queues behind the render and blocks
 * until its own timeout expires. A poll issued every few seconds for the length
 * of a YuE2 render turns into a hundred abandoned requests against a saturated
 * worker, which is what made the panel report a dead engine mid-render.
 *
 * Read from the queue rather than from a component's own state, so it is right
 * across tab switches and across a restart with a job left running.
 */
export function useQueueBusy(): boolean {
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    let alive = true;
    const read = async () => {
      try {
        const jobs = await queue.list();
        if (!alive) return;
        setBusy(
          jobs.some((j) => j.state === "running" || j.state === "queued"),
        );
      } catch {
        // An unreadable queue is not evidence that the engine is free. Assuming
        // busy here is the safe direction: it pauses a poll for a few seconds
        // rather than piling another request onto a busy worker.
        if (alive) setBusy(true);
      }
    };
    void read();
    const unlisten = queue.onChanged(() => void read());
    return () => {
      alive = false;
      void unlisten.then((f) => f());
    };
  }, []);

  return busy;
}