import { useMemo } from "react";
import type { Engine } from "../lib/engines";
import type { StoreSnapshot } from "../lib/types";

interface Props {
  engines: Engine[];
  running: boolean;
  snapshot: StoreSnapshot | null;
  /** Hide once everything is in place; this is a first-run panel, not a nag. */
  modelsDir: string;
}

/** Why a row is coloured: green settled, amber worth knowing, red broken. */
type State = "ok" | "info" | "warn" | "bad";

interface Check {
  label: string;
  detail: string;
  state: State;
  /**
   * Whether this stops the app generating anything.
   *
   * This is what decides if the panel appears at all. A stopped runtime is the
   * normal state between sessions -- you press Start when you want to work --
   * and the optional second engine may simply not be wanted. Reporting either
   * as something "left to do" turned a working app into a nag that never
   * cleared.
   */
  blocks: boolean;
}

/**
 * What still needs doing.
 *
 * The app's problems used to surface one at a time and in the wrong place: a
 * missing engine showed as a disabled card, missing weights as an empty model
 * picker, a stopped runtime as a grey dot in the sidebar. A first run hit all
 * three without ever being told. This gathers them in one place and then gets
 * out of the way once they are resolved.
 */
export function SetupCheck({ engines, running, snapshot, modelsDir }: Props) {
  // `list()` always returns both engines, so an empty array means the registry
  // has not been read yet. Reporting "not installed" in that window would be a
  // false alarm during exactly the startup this panel is meant to explain.
  const loaded = engines.length > 0;

  const checks = useMemo<Check[]>(() => {
    const out: Check[] = [];

    const acestep = engines.find((e) => e.id === "acestep");
    const audioCpp = engines.find((e) => e.id === "audiocpp");

    out.push(
      acestep?.installed
        ? { label: "ACE-Step engine", detail: "installed", state: "ok", blocks: false }
        : {
            label: "ACE-Step engine",
            detail: "not installed — it ships with the app",
            state: "bad",
            blocks: true,
          },
    );

    out.push(
      audioCpp?.installed
        ? {
            label: "audio.cpp engine",
            detail: "installed",
            state: "ok",
            blocks: false,
          }
        : {
            label: "audio.cpp engine",
            detail: "not installed — optional, for YuE2",
            state: "info",
            blocks: false,
          },
    );

    const dit = snapshot?.files?.filter((f) => f.role === "Dit" && f.present) ?? [];
    const lm = snapshot?.files?.filter((f) => f.role === "Planner" && f.present) ?? [];
    if (dit.length === 0 || lm.length === 0) {
      const missing = [
        dit.length === 0 ? "a DiT" : null,
        lm.length === 0 ? "a planner" : null,
      ]
        .filter(Boolean)
        .join(" and ");
      out.push({
        label: "Models",
        detail: `missing ${missing} — nothing can be generated yet`,
        state: "bad",
        blocks: true,
      });
    } else {
      out.push({
        label: "Models",
        detail: `${dit.length} DiT, ${lm.length} planner in ${modelsDir || "the default folder"}`,
        state: "ok",
        blocks: false,
      });
    }

    out.push(
      running
        ? { label: "Runtime", detail: "running", state: "ok", blocks: false }
        : {
            label: "Runtime",
            detail: "stopped — press Start to generate",
            state: "info",
            blocks: false,
          },
    );

    return out;
  }, [engines, running, snapshot, modelsDir]);

  // Once everything is fine this has nothing to say, so it does not render at
  // all rather than sitting there saying "ok" four times.
  // Only genuine blockers. Everything else the app already says in the place
  // it is relevant: the runtime state in the sidebar, the Generate button
  // explaining that the server must be started, the model picker listing what
  // is present.
  const blocking = checks.filter((c) => c.blocks);
  // Same reasoning for the models: the catalogue scan is a round trip, and an
  // empty result before it lands means "unknown", not "missing".
  const modelsKnown = (snapshot?.files?.length ?? 0) > 0;
  if (!loaded || !modelsKnown) return null;
  if (blocking.length === 0) return null;

  return (
    <div className="panel space-y-2 border-glow-500/25 p-4">
      <div className="flex items-center justify-between">
        <span className="label">Before you start</span>
        <span className="text-[11px] text-ink-500">
          {blocking.length} thing{blocking.length === 1 ? "" : "s"} to fix
        </span>
      </div>
      <ul className="space-y-1.5">
        {checks.map((c) => (
          <li key={c.label} className="flex items-start gap-2 text-[11px]">
            <span
              className={`mt-1 size-1.5 shrink-0 rounded-full ${
                c.state === "bad"
                  ? "bg-rose-400"
                  : c.state === "warn"
                    ? "bg-amber-400"
                    : "bg-ink-500"
              }`}
            />
            <span className="shrink-0 text-ink-200">{c.label}</span>
            <span
              className={`min-w-0 flex-1 ${
                c.state === "ok" ? "text-ink-500" : "text-ink-400"
              }`}
            >
              {c.detail}
            </span>
          </li>
        ))}
      </ul>
    </div>
  );
}