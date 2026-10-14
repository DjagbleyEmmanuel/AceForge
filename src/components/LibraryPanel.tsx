
import { useEffect, useState } from "react";
import {
  LOUDNESS_TARGETS,
  annotations,
  library,
  coverUrl,
  mastering,
  type Track,
} from "../lib/api";
import { formatBytes, type LoudnessTarget, type NormaliseReport } from "../lib/types";
import { TrackPlayer } from "./TrackPlayer";
import { Select } from "./Select";
import { CropDialog } from "./CropDialog";
import { LyricSyncPanel } from "./LyricSyncPanel";

interface Props {
  tracks: Track[];
  onChanged: () => void;
  /** Whether the engine is up; the lyric backfill needs it. */
  running?: boolean;
}

export function LibraryPanel({ tracks, onChanged, running = false }: Props) {
  // `tracks` is the single source of truth: this panel stays mounted now, so a
  // local mirror would go stale instead of re-seeding on remount.
  const items = tracks;
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

      <LyricSyncPanel running={running} onChanged={onChanged} />

      {pinned.length === 2 && <AbCompare a={pinned[0]} b={pinned[1]} />}

      {cropping && (
        <CropDialog
          track={cropping}
          onClose={() => setCropping(null)}
          onSaved={refresh}
        />
      )}

      {items.map((track) => {
        const cover = coverUrl(track.cover);
        return (
        <div key={track.path} className="panel p-4">
          <div className="flex gap-3">
            {cover && (
              <img
                src={cover}
                alt=""
                className="size-20 shrink-0 rounded-lg object-cover ring-1 ring-ink-800"
                loading="lazy"
              />
            )}
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
