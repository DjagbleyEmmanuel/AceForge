import { useCallback, useEffect, useState } from "react";
import { annotations, type LayerStack, type Track } from "../lib/api";
import { LayeredPreview, type LayerAudio } from "./LayeredPreview";
import { Select } from "./Select";

interface Props {
  tracks: Track[];
  /** Drives the generate form: this stem, generated over `sourcePath`. */
  onRenderStem: (stem: string, sourcePath: string) => void;
  running: boolean;
}

const STEM_NAMES = [
  "vocals",
  "backing_vocals",
  "drums",
  "bass",
  "guitar",
  "keyboard",
  "percussion",
  "strings",
  "synth",
  "fx",
  "brass",
  "woodwinds",
] as const;

/**
 * Build an arrangement one instrument at a time.
 *
 * Each new layer is generated over the mix built so far and then carried
 * forward as the source for the next one, which is the only way to stack stems
 * without every layer fighting the last. The stack is stored by path rather
 * than by copying tracks, so the library stays the single source of truth.
 */
export function StemBuilder({ tracks, onRenderStem, running }: Props) {
  const [stacks, setStacks] = useState<LayerStack[]>([]);
  const [activeId, setActiveId] = useState<string | null>(null);
  const [stem, setStem] = useState<string>("drums");
  const [mix, setMix] = useState<Record<string, { volume: number; muted: boolean }>>({});

  useEffect(() => {
    let alive = true;
    void annotations.layerStacks().then((s) => {
      if (!alive) return;
      setStacks(s);
      setActiveId((cur) => cur ?? s[0]?.id ?? null);
    });
    return () => {
      alive = false;
    };
  }, []);

  const active = stacks.find((s) => s.id === activeId) ?? null;

  const persist = useCallback(async (stack: LayerStack) => {
    setStacks(await annotations.saveLayerStack(stack));
  }, []);

  const trackFor = useCallback(
    (path: string) => tracks.find((t) => t.path === path) ?? null,
    [tracks],
  );

  /** Add a finished render to the top of the stack. */
  const push = useCallback(
    (track: Track, label: string) => {
      if (!active) return;
      const layer = {
        id: `${track.path}-${Date.now()}`,
        label,
        path: track.path,
      };
      void persist({ ...active, layers: [...active.layers, layer] });
    },
    [active, persist],
  );

  // Offer whatever is newest in the library and not yet in the stack. This is
  // the hand-off from a lego render back into the arrangement, and noticing it
  // automatically is what makes the loop bearable.
  const newest = tracks[0];
  const canPush =
    !!active && !!newest && !active.layers.some((l) => l.path === newest.path);

  const layers: LayerAudio[] = (active?.layers ?? [])
    .map((l) => {
      const track = trackFor(l.path);
      if (!track) return null;
      const m = mix[l.id] ?? { volume: 1, muted: false };
      return {
        id: l.id,
        label: l.label,
        track,
        volume: m.volume,
        muted: m.muted,
        onVolume: (v: number) =>
          setMix((prev) => ({ ...prev, [l.id]: { ...m, volume: v } })),
        onToggleMute: () =>
          setMix((prev) => ({ ...prev, [l.id]: { ...m, muted: !m.muted } })),
      };
    })
    .filter((l): l is LayerAudio => l !== null);

  return (
    <div className="panel space-y-3 p-4">
      <div className="flex items-center justify-between gap-2">
        <span className="label">Stem Builder</span>
        <div className="flex items-center gap-1.5">
          <div className="min-w-36">
            <Select
              size="sm"
              placeholder="no stacks"
              value={activeId ?? ""}
              options={[
                { value: "", label: "no stacks" },
                ...stacks.map((s) => ({ value: s.id, label: s.name })),
              ]}
              onChange={(v) => setActiveId(v || null)}
            />
          </div>
          <button
            onClick={() => {
              const id = `stack-${Date.now()}`;
              void persist({
                id,
                name: `arrangement ${stacks.length + 1}`,
                layers: [],
                updatedAt: 0,
              }).then(() => setActiveId(id));
            }}
            className="chip px-2 py-1 text-[11px]"
          >
            new stack
          </button>
          {active && (
            <button
              onClick={() =>
                void annotations.deleteLayerStack(active.id).then((s) => {
                  setStacks(s);
                  setActiveId(s[0]?.id ?? null);
                })
              }
              title="Delete this stack"
              className="chip px-2 py-1 text-[11px] hover:text-rose-300"
            >
              delete
            </button>
          )}
        </div>
      </div>

      {active && active.layers.length > 0 && (
        <>
          <LayeredPreview layers={layers} />

          <ul className="space-y-1">
            {active.layers.map((l, i) => (
              <li
                key={l.id}
                className="flex items-center gap-2 rounded bg-ink-900 px-2 py-1.5"
              >
                <span className="w-5 shrink-0 text-center font-mono text-[10px] text-ink-600">
                  {i + 1}
                </span>
                <span className="min-w-0 flex-1 truncate text-[11px] text-ink-200">
                  {l.label}
                </span>
                {!trackFor(l.path) && (
                  <span
                    className="shrink-0 text-[10px] text-rose-400"
                    title="This file is no longer in the library"
                  >
                    missing
                  </span>
                )}
                <button
                  onClick={() =>
                    void persist({
                      ...active,
                      layers: active.layers.filter((x) => x.id !== l.id),
                    })
                  }
                  className="shrink-0 text-ink-600 hover:text-rose-300"
                >
                  ×
                </button>
              </li>
            ))}
          </ul>
        </>
      )}

      {active && active.layers.length === 0 && (
        <p className="text-[11px] text-ink-500">
          An empty stack. Generate the first stem over a source track, then add
          the result here — each layer is generated over the mix above it.
        </p>
      )}

      {active && (
        <div className="space-y-2 border-t border-ink-800 pt-3">
          <div className="flex flex-wrap items-center gap-2">
            <div className="min-w-32">
              <Select
                size="sm"
                value={stem}
                options={STEM_NAMES.map((s) => ({ value: s, label: s.replace("_", " ") }))}
                onChange={setStem}
              />
            </div>
            <button
              onClick={() => {
                const source = active.layers[active.layers.length - 1];
                if (!source) return;
                onRenderStem(stem, source.path);
              }}
              disabled={running || active.layers.length === 0}
              title={
                active.layers.length === 0
                  ? "Add a first layer before building on it"
                  : `Generate ${stem} over the current top layer`
              }
              className="chip px-2 py-1 text-[11px] disabled:opacity-40"
            >
              build next layer
            </button>
            {canPush && (
              <button
                onClick={() =>
                  push(
                    newest,
                    trackFor(newest.path)?.caption?.slice(0, 20) || "layer",
                  )
                }
                title="Add the newest library render to this stack"
                className="chip px-2 py-1 text-[11px] text-glow-300"
              >
                + add newest render
              </button>
            )}
          </div>
          <p className="text-[10px] leading-relaxed text-ink-500">
            “build next layer” sets up a lego render of{" "}
            <span className="font-mono">{stem}</span> over the top of the stack.
            Generate it, then add the result back to keep going.
          </p>
        </div>
      )}
    </div>
  );
}