import { useCallback, useEffect, useState } from "react";
import { annotations, type Section, type SongProject, type Track } from "../lib/api";
import { Select } from "./Select";

interface Props {
  tracks: Track[];
  /**
   * Queue a repaint that outpaints onto `sourcePath` from `startSecs` for
   * `lengthSecs`. Variants differ only by seed.
   */
  onBuildSection: (opts: {
    sourcePath: string;
    startSecs: number;
    lengthSecs: number;
    seed: number;
  }) => void;
  running: boolean;
}

const VARIANTS = 4;

/**
 * Build a song one section at a time.
 *
 * Each section is a repaint whose region begins where the audio built so far
 * ends, which the backend outpaints by padding the source with silence and
 * generating through the seam. That is why this works on any track in the
 * library rather than only on text-to-music output: the first section is just a
 * repaint of a chosen source starting past its end.
 *
 * Variants are separate queue jobs rather than a batch parameter, because a
 * planner batch of four wants a KV cache that does not fit on a 4 GB card. Four
 * queued repaints cost the same wall time and each can be cancelled on its own.
 */
export function SongBuilder({ tracks, onBuildSection, running }: Props) {
  const [songs, setSongs] = useState<SongProject[]>([]);
  const [activeId, setActiveId] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    void annotations.songs().then((s) => {
      if (!alive) return;
      setSongs(s);
      setActiveId((cur) => cur ?? s[0]?.id ?? null);
    });
    return () => {
      alive = false;
    };
  }, []);

  const song = songs.find((s) => s.id === activeId) ?? null;

  const persist = useCallback(async (next: SongProject) => {
    setSongs(await annotations.saveSong(next));
  }, []);

  /** The adopted take for a section, or nothing if none has been adopted. */
  const chosenPath = (section: Section): string | null =>
    section.chosen !== null && section.chosen !== undefined
      ? (section.variants[section.chosen] ?? null)
      : null;

  const durationOf = useCallback(
    (path: string) => tracks.find((t) => t.path === path)?.durationSecs ?? 0,
    [tracks],
  );
  const trackFor = useCallback(
    (path: string) => tracks.find((t) => t.path === path) ?? null,
    [tracks],
  );

  /** The audio built so far: the last section's adopted variant, or its base. */
  const currentPath = song
    ? ([...song.sections].reverse().find((s) => chosenPath(s) !== null)
        ? chosenPath([...song.sections].reverse().find((s) => chosenPath(s) !== null)!)
        : (song.sections[song.sections.length - 1]?.basePath ?? null))
    : null;

  function newSong() {
    const id = `song-${Date.now()}`;
    void persist({
      id,
      name: `song ${songs.length + 1}`,
      caption: "",
      bpm: 0,
      keyscale: "",
      timesignature: "",
      sections: [],
      updatedAt: 0,
    }).then(() => setActiveId(id));
  }

  function addSection() {
    if (!song) return;
    const section: Section = {
      id: `sec-${Date.now()}`,
      name: `section ${song.sections.length + 1}`,
      lyrics: "",
      lengthSecs: 8,
      variants: [],
      chosen: null,
      basePath: currentPath,
    };
    void persist({ ...song, sections: [...song.sections, section] });
  }

  function patchSection(id: string, patch: Partial<Section>) {
    if (!song) return;
    void persist({
      ...song,
      sections: song.sections.map((s) => (s.id === id ? { ...s, ...patch } : s)),
    });
  }

  /** Queue one render per variant, each with its own seed. */
  function buildVariants(section: Section) {
    if (!song || !currentPath) return;
    const start = durationOf(currentPath);
    const base = Math.floor(Math.random() * 2 ** 31);
    for (let i = 0; i < VARIANTS; i++) {
      onBuildSection({
        sourcePath: currentPath,
        startSecs: start,
        lengthSecs: section.lengthSecs,
        seed: base + i,
      });
    }
  }

  /** Adopt a finished render as this section's chosen variant. */
  function adopt(sectionId: string, path: string) {
    if (!song) return;
    void persist({
      ...song,
      sections: song.sections.map((s) => {
        if (s.id !== sectionId) return s;
        const variants = [path, ...s.variants.filter((v) => v !== path)];
        return { ...s, variants, chosen: 0 };
      }),
    });
  }

  function removeSection(id: string) {
    if (!song) return;
    void persist({
      ...song,
      sections: song.sections.filter((s) => s.id !== id),
    });
  }

  if (!song) {
    return (
      <div className="panel space-y-3 p-4">
        <span className="label">Song Builder</span>
        <p className="text-[11px] text-ink-500">
          Assemble a song section by section. Each section is generated as an
          extension of everything before it, so the arrangement stays coherent
          instead of being stitched together afterwards.
        </p>
        <button onClick={newSong} className="chip px-2 py-1 text-[11px]">
          start a song
        </button>
      </div>
    );
  }

  return (
    <div className="panel space-y-3 p-4">
      <div className="flex items-center justify-between gap-2">
        <div className="min-w-36">
          <Select
            size="sm"
            placeholder="no songs"
            value={activeId ?? ""}
            options={[
              { value: "", label: "no songs" },
              ...songs.map((s) => ({ value: s.id, label: s.name })),
            ]}
            onChange={(v) => setActiveId(v || null)}
          />
        </div>
        <div className="flex gap-1.5">
          <button onClick={newSong} className="chip px-2 py-1 text-[11px]">
            new
          </button>
          <button
            onClick={() =>
              void annotations.deleteSong(song.id).then((s) => {
                setSongs(s);
                setActiveId(s[0]?.id ?? null);
              })
            }
            className="chip px-2 py-1 text-[11px] hover:text-rose-300"
          >
            delete
          </button>
        </div>
      </div>

      <div className="grid grid-cols-2 gap-2">
        <input
          value={song.caption}
          onChange={(e) => persist({ ...song, caption: e.target.value })}
          placeholder="shared style for the whole song"
          className="field col-span-2 py-1 text-[11px]"
        />
        <input
          type="number"
          value={song.bpm || ""}
          onChange={(e) => persist({ ...song, bpm: Number(e.target.value) })}
          placeholder="bpm"
          className="field py-1 text-[11px]"
        />
        <input
          value={song.keyscale}
          onChange={(e) => persist({ ...song, keyscale: e.target.value })}
          placeholder="key"
          className="field py-1 text-[11px]"
        />
      </div>

      {!currentPath && (
        <p className="text-[11px] text-amber-400/80">
          No source yet. The first section needs a track to build onto — set a
          source in the form above and it becomes the foundation.
        </p>
      )}

      <ul className="space-y-2">
        {song.sections.map((section, i) => {
          const base = chosenPath(section);
          return (
            <li key={section.id} className="rounded-lg bg-ink-900 p-2.5">
              <div className="flex items-center gap-2">
                <span className="w-5 shrink-0 text-center font-mono text-[10px] text-ink-600">
                  {i + 1}
                </span>
                <input
                  value={section.name}
                  onChange={(e) => patchSection(section.id, { name: e.target.value })}
                  className="min-w-0 flex-1 bg-transparent text-[11px] text-ink-100 outline-none"
                />
                <input
                  type="number"
                  min={1}
                  step={0.5}
                  value={section.lengthSecs}
                  onChange={(e) =>
                    patchSection(section.id, { lengthSecs: Number(e.target.value) })
                  }
                  title="Section length in seconds"
                  className="field w-16 py-0.5 text-[11px]"
                />
                <button
                  onClick={() => removeSection(section.id)}
                  className="shrink-0 text-ink-600 hover:text-rose-300"
                >
                  ×
                </button>
              </div>

              <textarea
                value={section.lyrics}
                onChange={(e) => patchSection(section.id, { lyrics: e.target.value })}
                placeholder="lyrics for this section"
                rows={2}
                className="field mt-1.5 w-full resize-y py-1 font-mono text-[11px]"
              />

              <div className="mt-1.5 flex items-center gap-1.5">
                <button
                  onClick={() => buildVariants(section)}
                  disabled={running || !currentPath}
                  title={
                    !currentPath
                      ? "Nothing to build onto yet"
                      : `Queue ${VARIANTS} takes of this section`
                  }
                  className="chip px-2 py-1 text-[11px] disabled:opacity-40"
                >
                  {VARIANTS} variants
                </button>
                {section.variants.length === 0 && currentPath && (
                  <span className="text-[10px] text-ink-500">
                    add a finished render as a take
                  </span>
                )}
              </div>

              {section.variants.length > 0 && (
                <div className="mt-1.5 flex flex-wrap gap-1">
                  {section.variants.map((v, vi) => {
                    const t = trackFor(v);
                    const chosen = section.chosen === vi;
                    return (
                      <button
                        key={`${v}-${vi}`}
                        onClick={() => patchSection(section.id, { chosen: vi })}
                        title={
                          t
                            ? `${t.caption || "render"} — ${t.durationSecs.toFixed(1)}s`
                            : "This file is no longer in the library"
                        }
                        className={`chip px-1.5 py-0.5 text-[10px] ${
                          chosen
                            ? "bg-glow-500/20 text-glow-300 ring-1 ring-glow-500/40"
                            : "bg-ink-800 text-ink-400"
                        }`}
                      >
                        take {vi + 1}
                        {t ? ` · ${t.durationSecs.toFixed(0)}s` : " · missing"}
                      </button>
                    );
                  })}
                </div>
              )}

              {base && (
                <p className="mt-1 text-[10px] text-ink-600">
                  adopted take starts at {durationOf(section.basePath ?? "").toFixed(1)}s
                </p>
              )}
            </li>
          );
        })}
      </ul>

      <div className="flex gap-1.5 border-t border-ink-800 pt-2">
        <button onClick={addSection} className="chip px-2 py-1 text-[11px]">
          + add section
        </button>
        {tracks[0] && (
          <button
            onClick={() => {
              if (!song.sections.length) {
                // Seed the foundation from a library track.
                void persist({
                  ...song,
                  sections: [
                    {
                      id: `sec-${Date.now()}`,
                      name: "section 1",
                      lyrics: "",
                      lengthSecs: 8,
                      variants: [tracks[0].path],
                      chosen: 0,
                      basePath: null,
                    },
                  ],
                });
                return;
              }
              adopt(song.sections[song.sections.length - 1].id, tracks[0].path);
            }}
            className="chip px-2 py-1 text-[11px] text-glow-300"
            title={
              song.sections.length === 0
                ? "Use the newest library render as this song's foundation"
                : "Adopt the newest library render as the last section's take"
            }
          >
            + newest render
          </button>
        )}
      </div>
    </div>
  );
}