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

type State = "ok" | "warn" | "bad";

interface Check {
  label: string;
  detail: string;
  state: State;
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
        ? { label: "ACE-Step engine", detail: "installed", state: "ok" }
        : {
            label: "ACE-Step engine",
            detail: "not installed — it ships with the app",
            state: "bad",
          },
    );

    out.push(
      audioCpp?.installed
        ? {
            label: "audio.cpp engine",
            detail: "installed",
            state: "ok",
          }
        : {
            label: "audio.cpp engine",
            detail: "not installed — optional, for YuE2",
            state: "warn",
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
      });
    } else {
      out.push({
        label: "Models",
        detail: `${dit.length} DiT, ${lm.length} planner in ${modelsDir || "the default folder"}`,
        state: "ok",
      });
    }

    out.push(
      running
        ? { label: "Runtime", detail: "running", state: "ok" }
        : {
            label: "Runtime",
            detail: "stopped — press Start to generate",
            state: "warn",
          },
    );

    return out;
  }, [engines, running, snapshot, modelsDir]);

  // Once everything is fine this has nothing to say, so it does not render at
  // all rather than sitting there saying "ok" four times.
  const outstanding = checks.filter((c) => c.state !== "ok");
  // Same reasoning for the models: the catalogue scan is a round trip, and an
  // empty result before it lands means "unknown", not "missing".
  const modelsKnown = (snapshot?.files?.length ?? 0) > 0;
  if (!loaded || !modelsKnown) return null;
  if (outstanding.length === 0) return null;

  return (
    <div className="panel space-y-2 border-glow-500/25 p-4">
      <div className="flex items-center justify-between">
        <span className="label">Before you start</span>
        <span className="text-[11px] text-ink-500">
          {outstanding.length} item{outstanding.length === 1 ? "" : "s"} left
        </span>
      </div>
      <ul className="space-y-1.5">
        {checks.map((c) => (
          <li key={c.label} className="flex items-start gap-2 text-[11px]">
            <span
              className={`mt-1 size-1.5 shrink-0 rounded-full ${
                c.state === "ok"
                  ? "bg-mint-400"
                  : c.state === "warn"
                    ? "bg-amber-400"
                    : "bg-rose-400"
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