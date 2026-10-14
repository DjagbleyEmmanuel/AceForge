
import { useCallback, useEffect, useMemo, useState } from "react";
import {
  LOUDNESS_TARGETS,
  annotations,
  library,
  lyricSync,
  mastering,
  type Track,
} from "../lib/api";
import { formatBytes, type LoudnessTarget, type NormaliseReport } from "../lib/types";
import { readText, writeText } from "../lib/prefs";
import { TrackPlayer } from "./TrackPlayer";
import { Select } from "./Select";
import { CoverArt } from "./CoverArt";
import { CropDialog } from "./CropDialog";
import { LyricSyncPanel } from "./LyricSyncPanel";

interface Props {
  tracks: Track[];
  onChanged: () => void;
  /** Whether the engine is up; the lyric backfill needs it. */
  running?: boolean;
}

type Sort = "newest" | "oldest" | "longest" | "name";
type Filter = "all" | "unsynced" | "synced" | "instrumental" | "wav";

export function LibraryPanel({ tracks, onChanged, running = false }: Props) {
  // `tracks` is the single source of truth: this panel stays mounted now, so a
  // local mirror would go stale instead of re-seeding on remount.
  // How the library was left last time. Re-sorting to newest and dropping the
  // filter on every launch means the list rearranges under the user.
  const [sort, setSort] = useState<Sort>(() => readText("library.sort", "newest") as Sort);
  const [filter, setFilter] = useState<Filter>(
    () => readText("library.filter", "all") as Filter,
  );
  const [search, setSearch] = useState(() => readText("library.search", ""));
  // Paths of songs that have lyrics but no timings. Asked for once here rather
  // than in both the filter and the sync panel.
  const [unsynced, setUnsynced] = useState<Set<string>>(new Set());

  useEffect(() => writeText("library.sort", sort), [sort]);
  useEffect(() => writeText("library.filter", filter), [filter]);
  useEffect(() => writeText("library.search", search), [search]);

  const refreshUnsynced = useCallback(async () => {
    try {
      const list = await lyricSync.pending();
      setUnsynced(new Set(list.map((t) => t.path)));
    } catch {
      setUnsynced(new Set());
    }
  }, []);

  useEffect(() => {
    void refreshUnsynced();
  }, [refreshUnsynced, tracks]);

  const items = useMemo(() => {
    // The library passes sorted-by-newest, which is right by default but makes
    // everything else awkward to find once there are dozens of tracks.
    const needle = search.trim().toLowerCase();
    const out = tracks.filter((t) => {
      if (filter === "unsynced" && !unsynced.has(t.path)) return false;
      if (filter === "synced" && unsynced.has(t.path)) return false;
      if (filter === "instrumental" && !t.lyrics.includes("[Instrumental]")) {
        return false;
      }
      if (filter === "wav" && t.format !== "wav") return false;
      if (needle) {
        const hay = `${t.caption} ${t.id} ${t.lyrics.slice(0, 120)}`.toLowerCase();
        if (!hay.includes(needle)) return false;
      }
      return true;
    });

    const byTime = (a: Track, b: Track) => b.createdAt - a.createdAt;
    switch (sort) {
      case "oldest":
        return [...out].sort((a, b) => -byTime(a, b));
      case "longest":
        return [...out].sort((a, b) => b.durationSecs - a.durationSecs);
      case "name":
        return [...out].sort((a, b) =>
          (a.caption || a.id).localeCompare(b.caption || b.id),
        );
      default:
        return [...out].sort(byTime);
    }
  }, [tracks, sort, filter, search, unsynced]);
  const [slots, setSlots] = useState<[string, string] | null>(null);
  const [normalising, setNormalising] = useState<string | null>(null);
  const [cropping, setCropping] = useState<Track | null>(null);
  const [reference, setReference] = useState("");
  const [matchReference, setMatchReference] = useState(false);
  const [report, setReport] = useState<NormaliseReport | null>(null);
  const [masterError, setMasterError] = useState<string | null>(null);

  async function runNormalise(track: Track, target: LoudnessTarget) {
    setNormalising(track.path);
    setMasterError(null);
    try {
      setReport(
        await mastering.normalise(
          track.path,
          target,
          matchReference ? reference : undefined,
        ),
      );
      await refresh();
    } catch (e) {
      setMasterError(String(e));
    } finally {
      setNormalising(null);
    }
  }

  useEffect(() => {
    void annotations.abSlots().then(setSlots).catch(() => setSlots(null));
  }, []);

  const pinned = slots
    ?.map((id) => items.find((t) => t.id === id))
    .filter((t): t is Track => t !== undefined) ?? [];

  async function refresh() {
    await onChanged();
  }

  async function remove(track: Track) {
    try {
      await library.remove(track.path);
      await refresh();
    } catch {
      // A failed delete leaves the list unchanged; the next scan re-reads disk
      // anyway, so just resync.
      await refresh();
    }
  }

  if (items.length === 0) {
    return (
      <div className="panel p-10 text-center">
        <p className="text-sm text-ink-400">No tracks yet.</p>
        <p className="mt-1 text-xs text-ink-500">
          Anything you generate lands here and stays on disk.
        </p>
      </div>
    );
  }

  return (
    <div className="space-y-3">
      {report && (
        <div className="panel border-glow-500/25 bg-glow-500/5 px-4 py-3 text-xs text-ink-300">
          <span className="label">Normalised</span>{" "}
          {report.measuredLufs === null ? (
            <span className="text-ink-400">
              measured nothing loud enough to act on — left unchanged.
            </span>
          ) : (
            <>
              measured{" "}
              <span className="font-mono text-glow-300">
                {report.measuredLufs.toFixed(1)} LUFS
              </span>{" "}
              → target{" "}
              <span className="font-mono text-glow-300">{report.targetLufs}</span>,
              applied{" "}
              <span className="font-mono">
                {report.appliedGainDb >= 0 ? "+" : ""}
                {report.appliedGainDb.toFixed(1)} dB
              </span>
              {report.brightnessTiltDb !== null &&
                Math.abs(report.brightnessTiltDb) > 0.1 && (
                  <>
                    , brightness matched by{" "}
                    <span className="font-mono">
                      {report.brightnessTiltDb > 0 ? "+" : ""}
                      {report.brightnessTiltDb.toFixed(1)} dB
                    </span>
                  </>
                )}
              {report.limited && (
                <>
                  , peak limited to {report.peakAfterDbfs.toFixed(1)} dBFS
                </>
              )}
            </>
          )}
          . A normalised copy was added to the library; the original is untouched.
        </div>
      )}

      {masterError && (
        <div className="panel border-rose-500/30 bg-rose-500/5 px-4 py-3 text-xs text-rose-300">
          {masterError}
        </div>
      )}

      <div className="panel flex flex-wrap items-center gap-2 p-3">
        <input
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          placeholder="search songs"
          aria-label="Search songs"
          className="field min-w-40 flex-1 py-1 text-xs"
        />
        <div className="w-40">
          <Select
            size="sm"
            value={filter}
            options={[
              { value: "all", label: "all songs" },
              { value: "synced", label: "lyrics synced" },
              { value: "unsynced", label: "lyrics not synced" },
              { value: "instrumental", label: "instrumental" },
              { value: "wav", label: "wav only" },
            ]}
            onChange={(v) => setFilter(v as Filter)}
          />
        </div>
        <div className="w-36">
          <Select
            size="sm"
            value={sort}
            options={[
              { value: "newest", label: "newest first" },
              { value: "oldest", label: "oldest first" },
              { value: "longest", label: "longest first" },
              { value: "name", label: "by name" },
            ]}
            onChange={(v) => setSort(v as Sort)}
          />
        </div>
        <span className="text-[11px] text-ink-500">
          {items.length} of {tracks.length}
        </span>
      </div>

      <LyricSyncPanel
        running={running}
        onChanged={onChanged}
        unsyncedCount={unsynced.size}
        onRefresh={refreshUnsynced}
      />

      {pinned.length === 2 && <AbCompare a={pinned[0]} b={pinned[1]} />}

      {cropping && (
        <CropDialog
          track={cropping}
          onClose={() => setCropping(null)}
          onSaved={refresh}
        />
      )}

      {items.length === 0 && tracks.length > 0 && (
        <div className="panel p-8 text-center">
          <p className="text-sm text-ink-400">Nothing matches that.</p>
          <button
            onClick={() => {
              setSearch("");
              setFilter("all");
            }}
            className="mt-2 text-xs text-glow-300 underline-offset-2 hover:underline"
          >
            clear the search and filter
          </button>
        </div>
      )}

      {items.map((track) => {
        return (
        <div key={track.path} className="panel p-4">
          <div className="flex gap-3">
            <CoverArt path={track.cover} />
            <div className="min-w-0 flex-1">
              <TrackPlayer track={track} />
            </div>
          </div>

          <div className="mt-3 flex flex-wrap items-center justify-end gap-2">
            <button
              onClick={() =>
                void annotations
                  .setAbSlot(0, track.id)
                  .then((next) => setSlots(next as [string, string]))
              }
              title="Pin this track as A"
              className={`btn btn-ghost px-2 py-1 text-xs ${
                slots?.[0] === track.id ? "text-glow-300 ring-1 ring-glow-500/40" : ""
              }`}
            >
              A
            </button>
            <button
              onClick={() =>
                void annotations
                  .setAbSlot(1, track.id)
                  .then((next) => setSlots(next as [string, string]))
              }
              title="Pin this track as B"
              className={`btn btn-ghost px-2 py-1 text-xs ${
                slots?.[1] === track.id ? "text-glow-300 ring-1 ring-glow-500/40" : ""
              }`}
            >
              B
            </button>
            <label
              className="flex items-center gap-1.5 text-[10px] text-ink-500"
              title="Match the mix's brightness to a reference track"
            >
              <input
                type="checkbox"
                checked={matchReference}
                onChange={(e) => setMatchReference(e.target.checked)}
                className="accent-glow-500"
              />
              match
            </label>
            {matchReference && (
              <div className="min-w-44">
                <Select
                  size="sm"
                  placeholder="pick a reference…"
                  value={reference}
                  options={[
                    { value: "", label: "no reference" },
                    ...items
                      .filter((t) => t.path !== track.path)
                      .map((t) => ({
                        value: t.path,
                        label: (t.caption || t.id).slice(0, 48),
                      })),
                  ]}
                  onChange={setReference}
                />
              </div>
            )}
            <div className="flex items-center gap-1">
              {LOUDNESS_TARGETS.map((t) => (
                <button
                  key={t.id}
                  onClick={() => void runNormalise(track, t.id)}
                  disabled={normalising === track.path}
                  title={`Normalise a copy to ${t.note} (-${t.label.replace("-", "")} LUFS). The original is kept.`}
                  className="btn btn-ghost px-2 py-1 font-mono text-[11px] disabled:opacity-40"
                >
                  {t.label}
                </button>
              ))}
            </div>
            <button
              onClick={() => setCropping(track)}
              title="Keep only part of this song"
              className="btn btn-ghost px-3 py-1 text-xs"
            >
              Crop
            </button>
            <button
              onClick={() => void library.reveal(track.path)}
              className="btn btn-ghost px-3 py-1 text-xs"
            >
              Show file
            </button>
            <button
              onClick={() => void remove(track)}
              className="btn btn-ghost px-3 py-1 text-xs hover:text-rose-300"
            >
              Delete
            </button>
          </div>
        </div>
        );
      })}
    </div>
  );
}

/** Parameter diff between two pinned tracks.
 *
 *  The point of an A/B is answering "what actually changed", so this compares
 *  only fields where the two differ rather than dumping both parameter sets.
 */
function AbCompare({ a, b }: { a: Track; b: Track }) {
  const fields: [string, (t: Track) => string][] = [
    ["caption", (t) => t.caption || "—"],
    ["lyrics", (t) => (t.lyrics ? `${t.lyrics.split("\n").length} lines` : "—")],
    ["bpm", (t) => (t.bpm > 0 ? String(t.bpm) : "—")],
    ["key", (t) => t.keyscale || "—"],
    ["time", (t) => t.timesignature || "—"],
    ["language", (t) => t.vocalLanguage || "—"],
    ["length", (t) => `${t.durationSecs.toFixed(1)}s`],
    ["size", (t) => formatBytes(t.bytes)],
  ];
  const diffs = fields.filter(([, get]) => get(a) !== get(b));

  return (
    <div className="panel p-4">
      <div className="flex items-baseline justify-between">
        <span className="label">A / B</span>
        <span className="text-[11px] text-ink-500">
          {diffs.length === 0
            ? "these two are identical"
            : `${diffs.length} field${diffs.length > 1 ? "s" : ""} differ`}
        </span>
      </div>
      <div className="mt-3 space-y-1.5">
        {diffs.map(([name, get]) => (
          <div key={name} className="grid grid-cols-[7rem_1fr_1fr] gap-2 text-xs">
            <span className="text-ink-500">{name}</span>
            <span className="truncate text-ink-300" title={get(a)}>
              A: {get(a)}
            </span>
            <span className="truncate text-ink-300" title={get(b)}>
              B: {get(b)}
            </span>
          </div>
        ))}
      </div>
    </div>
  );
}
