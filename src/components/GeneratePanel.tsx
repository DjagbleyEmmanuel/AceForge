import { useCallback, useEffect, useRef, useState } from "react";
import {
  queue,
  wildcards as wildcardsApi,
  music,
  presets as presetApi,
  userPresets,
  type GenerateParams,
  type GenerationPreset,
  type MusicReference,
  type SamplingPreset,
  type Track,
  type UserPreset,
  adapters,
  type WeightFile,
} from "../lib/api";
import { EditableSlider } from "./EditableSlider";
import { RepaintRegion } from "./RepaintRegion";
import { SeedManager } from "./SeedManager";
import { Select } from "./Select";
import { StemBuilder } from "./StemBuilder";
import { SongBuilder } from "./SongBuilder";
import { SourcePicker } from "./SourcePicker";
import type { ImportedFile } from "../lib/api";
import { SamplingGuide, presetTitle, samplingSummary } from "./SamplingGuide";
import { formatBytes } from "../lib/types";

interface Props {
  port: number | null;
  /** Applies a stem layer request from StemBuilder to the form. */
  onStemLayer?: (stem: string, sourcePath: string) => void;
  /** Queues the repaints SongBuilder needs for one section. */
  onBuildSection?: (opts: {
    sourcePath: string;
    startSecs: number;
    lengthSecs: number;
    seed: number;
  }) => void;
  running: boolean;
  catalog: WeightFile[];
  onBrowseModels: () => void;
  params: GenerateParams;
  setParams: React.Dispatch<React.SetStateAction<GenerateParams>>;
  /** Finished songs, so a region of one can be repainted. */
  tracks: Track[];
}

/** Task types `AceRequest::task_type` accepts. Anything else is refused
 *  server-side, so the list here is the whole vocabulary, not a subset. */
/** Every `task_type` `AceRequest` accepts.
 *
 *  - `stem`: how many stems the instruction takes. `complete` is the only one
 *    that accepts several, joined with " | ".
 *  - `needsBase`: the task calls `warn_if_turbo_stem`, which logs and renders
 *    anyway, so the UI blocks it instead.
 *
 *  `cover-nofsq` is `cover` without the FSQ roundtrip: the DiT works on clean
 *  25 Hz VAE latents and the output stays close to the source's structure and
 *  timbre instead of diverging from it. */
const TASK_TYPES = [
  { id: "text2music", label: "Text to music", needsSource: false },
  { id: "cover", label: "Cover", needsSource: true },
  { id: "cover-nofsq", label: "Cover (faithful)", needsSource: true },
  { id: "repaint", label: "Repaint a section", needsSource: true },
  { id: "lego", label: "Lego (add a stem)", needsSource: true, stem: "single" },
  { id: "extract", label: "Extract a stem", needsSource: true, stem: "single" },
  {
    id: "complete",
    label: "Complete a stem",
    needsSource: true,
    stem: "multi",
  },
] as const;

/** Tasks the backend refuses to serve coherently on a turbo DiT.
 *
 *  `run_lego`, `run_extract` and `run_complete` all call `warn_if_turbo_stem`,
 *  which prints "requires base model, turbo output incoherent" and then runs
 *  it anyway. So the warning is in a log file, not the UI, and the result is
 *  mud. `run_repaint` deliberately has no such check. */
const STEM_TASKS = new Set(["lego", "extract", "complete"]);

/** The stems `TRACK_NAMES` in `task-types.h` accepts.
 *
 *  Anything else still reaches the model, but the backend logs
 *  "'x' is not a standard track name" and presses on, so an open text field
 *  would let most entries through with a warning nobody sees. This list is
 *  that array, verbatim. */
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

export function GeneratePanel({
  port,
  running,
  catalog,
  onBrowseModels,
  params,
  setParams,
  tracks,
  onStemLayer,
  onBuildSection,
}: Props) {
  const [notice, setNotice] = useState<string | null>(null);
  // Imports live in `library/imports`, which the generated-song list does not
  // walk, so both source pickers need it to tell a live import from a path that
  // has since been deleted.
  const [imported, setImported] = useState<ImportedFile[]>([]);
  const [preview, setPreview] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [showAdvanced, setShowAdvanced] = useState(false);
  const [presets, setPresets] = useState<GenerationPreset[]>([]);
  const [activePreset, setActivePreset] = useState<string | null>(null);
  const [reference, setReference] = useState<MusicReference | null>(null);
  const [activeSampling, setActiveSampling] = useState<string | null>(null);
  const [showGuide, setShowGuide] = useState(false);

  useEffect(() => {
    let alive = true;
    void presetApi.list().then((list) => {
      if (alive) setPresets(list);
    });
    void music.reference().then((ref) => {
      if (alive) setReference(ref);
    });
    return () => {
      alive = false;
    };
  }, []);

  const instrumental = params.lyrics.trim() === "[Instrumental]";

  // The DiT and the sampling preset are halves of one decision. Turbo is a
  // distilled 8-step model; running it on 50-step SFT settings, or SFT on
  // turbo settings, silently wastes the model. Derived from the model that is
  // actually selected rather than from which chip was last pressed.
  const ditVariant = catalog.find((f) => f.filename === params.synthModel)?.variant ?? "";
  const expectedSampling = /^turbo/.test(ditVariant)
    ? "turbo"
    : /sft|base|xl/.test(ditVariant)
      ? "sft"
      : null;
  const mismatch =
    expectedSampling !== null && activeSampling !== null && activeSampling !== expectedSampling;

  function set<K extends keyof GenerateParams>(key: K, value: GenerateParams[K]) {
    setParams((p) => ({ ...p, [key]: value }));
  }

  /** Load a saved set of settings without touching the text the user wrote.
   *
   *  Both the caption and the lyrics stored with a preset are what someone was
   *  thinking about when they hit save, not part of the configuration. Applying
   *  them was wiping work in progress: the caption was already spared, and
   *  lyrics were not, so picking a preset mid-write destroyed the pasted text.
   */
  function restore(saved: GenerateParams) {
    setParams((cur) => ({ ...saved, caption: cur.caption, lyrics: cur.lyrics }));
  }

  // Cover, repaint and lego are all "render this source audio" tasks. Sending
  // one without a source is a backend error the user cannot act on, so say so
  // here instead.
  // Every task except text-to-music is "here is audio, do something to it".
  const taskNeedsSource = params.taskType !== "text2music";
  // A stem task on turbo renders anyway and produces mud; the backend only
  // writes a line to stderr. Block it rather than let it fail quietly.
  const stemOnTurbo =
    STEM_TASKS.has(params.taskType) && /^turbo/.test(ditVariant);
  const blockedReason = !running
    ? "Start the server first"
    : stemOnTurbo
      ? `${ditVariant} is a turbo model — ${params.taskType} needs base or SFT`
      : !params.caption.trim() && !params.sourceAudio
        ? "A caption is required"
        : taskNeedsSource && !params.sourceAudio
          ? "This task needs a source track"
          : undefined;

  // Submission is now a queue hand-off rather than an awaited render. The
  // button returns immediately, the job row in QueuePanel owns progress and
  // failure, and the engine renders in submission order on its own thread.
  async function submit() {
    if (port === null) return;
    setError(null);
    try {
      await queue.add(params);
      setNotice("Added to the queue — progress is in the queue below.");
      setTimeout(() => setNotice(null), 2600);
    } catch (e) {
      setError(String(e));
      setNotice(null);
    }
  }

  return (
    <div className="space-y-5">
      <section className="panel p-5 space-y-4">
        {/* Presets and sampling presets sit side by side and stay narrow. Both
            were full-panel-width boxes for a one-word choice, which read as an
            empty field and pushed everything below them down the page. */}
        <div className="flex flex-wrap items-end gap-3">
          <Select
            label="Preset"
            width="md"
            placeholder="choose a preset…"
            value={activePreset ?? ""}
            options={[
              { value: "", label: "none" },
              ...presets.map((preset) => ({
                value: preset.id,
                label: preset.label,
                note: preset.description,
                meta: preset.approxSeconds,
              })),
            ]}
            onChange={(id) => {
              const preset = presets.find((p) => p.id === id);
              if (!preset) {
                setActivePreset(null);
                return;
              }
              // A preset is a set of rendering knobs. It must not silently
              // replace the caption the user typed or the lyrics they pasted --
              // only a preset that explicitly owns the vocal mode may set the
              // lyrics field. Restoring `caption` alone used to leave lyrics
              // being wiped, which is what lost pasted text.
              setParams((p) => ({
                ...preset.params,
                caption: p.caption,
                lyrics: preset.setsLyrics ? preset.params.lyrics : p.lyrics,
              }));
              setActivePreset(preset.id);
            }}
          />

          <div className="min-w-0">
            <div className="flex items-center justify-between gap-2">
              <span className="label mb-1.5 block">Sampling</span>
              <button
                onClick={() => setShowGuide((v) => !v)}
                title="What each sampling preset does"
                className="text-[10px] text-glow-300 underline-offset-2 hover:underline"
              >
                {showGuide ? "hide" : "?"}
              </button>
            </div>
            <Select
              size="sm"
              width="md"
              placeholder="no preset"
              value={activeSampling ?? ""}
              options={[
                { value: "", label: "none" },
                ...(reference?.samplingPresets ?? []).map(
                  (preset: SamplingPreset) => ({
                    value: preset.id,
                    label: preset.label,
                    note: presetTitle(preset),
                  }),
                ),
              ]}
              onChange={async (id) => {
                if (!id) {
                  setActiveSampling(null);
                  return;
                }
                // Rust owns the merge so a sampling preset can never clobber
                // the caption or lyrics.
                const merged = await music.applySampling(id, params);
                setParams(merged);
                setActiveSampling(id);
              }}
            />
          </div>
        </div>

        <div>
          <p className="mt-2 text-[10px] text-ink-500">
            {samplingSummary(params, activeSampling ?? undefined)}
          </p>
          {mismatch && (
            <p className="mt-1.5 rounded-lg bg-amber-400/10 px-2.5 py-1.5 text-[11px] leading-relaxed text-amber-300 ring-1 ring-amber-400/25">
              <strong>{ditVariant}</strong> is paired with the{" "}
              <strong>{activeSampling}</strong> sampling preset. It wants{" "}
              {expectedSampling === "sft"
                ? "50 steps at shift 1.0"
                : "8 steps at shift 3.0"}
              . Generating as-is will sound worse than the model is capable of.
            </p>
          )}
        </div>

        {showGuide && <SamplingGuide onClose={() => setShowGuide(false)} />}

        <div>
          <label className="label" htmlFor="title">
            Title
          </label>
          <input
            id="title"
            value={params.title}
            onChange={(e) => set("title", e.target.value)}
            placeholder="Leave empty and the caption names the file"
            className="field mt-1.5"
          />
          <p className="mt-1 text-[10px] text-ink-500">
            Used for the filename and the library entry.
          </p>
        </div>

        <div>
          <label className="label" htmlFor="caption">
            Describe the track
          </label>
          <textarea
            id="caption"
            rows={3}
            value={params.caption}
            onChange={(e) => set("caption", e.target.value)}
            placeholder="Moody lo-fi hip hop, dusty rhodes, vinyl crackle, slow and warm"
            className="field mt-1.5 resize-y leading-relaxed"
          />
          {(wildcardsApi.hasWildcards(params.caption) ||
            wildcardsApi.hasWildcards(params.lyrics)) && (
            <div className="mt-2">
              <button
                onClick={async () => {
                  // Preview both fields against one seed, exactly as the engine
                  // path does, so what is shown is what will be rendered.
                  const seed = params.seed;
                  const cap = await wildcardsApi.preview(params.caption, seed);
                  const lyr = params.lyrics
                    ? await wildcardsApi.preview(params.lyrics, seed)
                    : "";
                  setPreview(
                    lyr ? `caption: ${cap}\nlyrics: ${lyr}` : `caption: ${cap}`,
                  );
                }}
                className="chip px-2 py-1 text-[11px]"
                title="Show one expansion of the wildcards for the current seed"
              >
                preview wildcards
              </button>
              {preview && (
                <pre className="mt-2 whitespace-pre-wrap break-words rounded bg-ink-900 p-2.5 text-[11px] leading-relaxed text-ink-300">
                  {preview}
                </pre>
              )}
              <p className="mt-1.5 text-[11px] text-ink-500">
                {`{a|b|c}`} picks one option from the seed. Leave a seed in place
                and Retry reuses it, so the same prompt gives the same song.
              </p>
            </div>
          )}
        </div>

        <TaskSection
          tracks={tracks}
          params={params}
          setParams={setParams}
          catalog={catalog}
          imported={imported}
          onImported={setImported}
        />
        <VocalGenderToggle params={params} setParams={setParams} />
        <VoiceSection
          tracks={tracks}
          params={params}
          setParams={setParams}
          imported={imported}
          onImported={setImported}
        />

        <div>
          <label className="label" htmlFor="lyrics">
            Lyrics
          </label>
          <textarea
            id="lyrics"
            rows={4}
            value={params.lyrics}
            onChange={(e) => set("lyrics", e.target.value)}
            placeholder="Leave empty to let the planner write them, or use [Instrumental] for no vocals"
            className="field mt-1.5 resize-y leading-relaxed font-mono text-xs"
          />
          <div className="mt-2 flex items-center gap-3">
            <button
              onClick={() =>
                set("lyrics", instrumental ? "" : "[Instrumental]")
              }
              className="chip bg-ink-800 text-ink-200 ring-1 ring-ink-700 hover:bg-ink-750"
            >
              {instrumental ? "✓ instrumental" : "mark as instrumental"}
            </button>
            <span className="text-[11px] text-ink-500">
              {params.lyrics.trim()
                ? instrumental
                  ? "no vocals"
                  : "using your lyrics"
                : "planner will write lyrics"}
            </span>
          </div>
        </div>

        <LoadParamsButton setParams={setParams} />

        <AdapterSection params={params} setParams={setParams} port={port} />

        <LmSamplingPanel params={params} setParams={setParams} />

        <UserPresetBar params={params} restore={restore} />

        <div className="grid grid-cols-2 gap-3 sm:grid-cols-4">
          <div>
            <label className="label" htmlFor="duration">
              Seconds
            </label>
            <input
              id="duration"
              type="number"
              min={10}
              max={600}
              value={params.duration || ""}
              placeholder="auto"
              onChange={(e) => set("duration", Number(e.target.value) || 0)}
              className="field mt-1.5"
            />
          </div>
          <PickField
            id="bpm"
            label="BPM"
            value={params.bpm === 0 ? "" : String(params.bpm)}
            placeholder="auto"
            onChange={(v) => set("bpm", Number(v) || 0)}
            inputMode="numeric"
            suggestions={
              reference?.bpmPresets.map((b) => ({
                value: String(b.value),
                label: `${b.value} — ${b.label}`,
              })) ?? []
            }
          />
          <PickField
            id="keyscale"
            label="Key"
            value={params.keyscale}
            placeholder="auto"
            onChange={(v) => set("keyscale", v)}
            suggestions={
              reference?.keys.map((k) => ({ value: k.value, label: k.value })) ?? []
            }
          />
          <LanguageField
            value={params.vocalLanguage}
            onChange={(v) => set("vocalLanguage", v)}
            languages={reference?.languages ?? []}
          />
        </div>

        <div className="grid gap-3 sm:grid-cols-2">
          <ModelPicker
                        label="Planner LM"
            hint="writes lyrics and audio codes"
            value={params.lmModel}
            onChange={(v) => set("lmModel", v)}
            goToModels={onBrowseModels}
            files={catalog.filter((f) => f.role === "Planner" && f.present)}
          />
          <ModelPicker
                        label="DiT"
            hint="renders the audio"
            value={params.synthModel}
            onChange={(v) => {
              set("synthModel", v);
              // Carry the matching sampling preset along, unless the user has
              // deliberately pinned one.
              if (activeSampling === null || activeSampling === "turbo" || activeSampling === "sft") {
                const picked = catalog.find((f) => f.filename === v)?.variant ?? "";
                const wanted = /^turbo/.test(picked) ? "turbo" : "sft";
                void music.applySampling(wanted, { ...params, synthModel: v }).then(setParams);
                setActiveSampling(wanted);
              }
            }}
            goToModels={onBrowseModels}
            files={catalog.filter((f) => f.role === "Dit" && f.present)}
          />
        </div>

        <div className="flex items-center justify-between pt-1">
          <button
            onClick={() => setShowAdvanced((v) => !v)}
            className="text-xs text-ink-400 underline-offset-4 hover:text-ink-200 hover:underline"
          >
            {showAdvanced ? "hide" : "show"} advanced
          </button>

          <button
            onClick={submit}
            disabled={blockedReason !== undefined}
            title={blockedReason}
            className="btn btn-primary min-w-36"
          >
            {running ? "Generate" : "server offline"}
          </button>
        </div>
      </section>

      {notice && (
        <p className="text-xs text-glow-400">{notice}</p>
      )}

      {error && (
        <section className="panel border-rose-500/30 bg-rose-500/5 p-4">
          <p className="text-sm text-rose-300">Generation failed</p>
          <pre className="mt-1.5 overflow-x-auto text-xs text-rose-400/80">{error}</pre>
        </section>
      )}

      {showAdvanced && (
        <section className="panel space-y-4 p-5">
          <h3 className="text-sm font-medium text-ink-200">Sampling</h3>
          <div className="grid grid-cols-2 gap-4 lg:grid-cols-3">
            <EditableSlider
              label="Steps"
              value={params.inferenceSteps}
              min={0}
              max={100}
              onChange={(v) => set("inferenceSteps", Math.round(v))}
              display={(v) => (v === 0 ? "0 · auto" : String(v))}
              help="0 resolves from the model: turbo 8, SFT and base 50."
            />
            <EditableSlider
              label="Guidance"
              value={params.guidanceScale}
              min={0}
              max={10}
              step={0.1}
              onChange={(v) => set("guidanceScale", v)}
              display={(v) => (v === 0 ? "0 · off" : v.toFixed(1))}
              help="0 turns CFG off. Ignored above 1.0 on turbo."
            />
            <EditableSlider
              label="Shift"
              value={params.shift}
              min={0}
              max={10}
              step={0.1}
              onChange={(v) => set("shift", v)}
              display={(v) => (v === 0 ? "0 · auto" : v.toFixed(1))}
              help="0 resolves from the model: turbo 3.0, SFT and base 1.0."
            />
            <EditableSlider
              label="Takes"
              value={params.synthBatchSize}
              min={1}
              max={9}
              onChange={(v) => set("synthBatchSize", Math.round(v))}
              help="Renders several takes from one plan."
            />
            <EditableSlider
              label="Seed"
              value={params.seed}
              min={-1}
              max={999999}
              onChange={(v) => set("seed", Math.round(v))}
              display={(v) => (v === -1 ? "-1 · random" : String(v))}
            />
            <div>
              <Select
                label="Format"
                value={params.outputFormat}
                options={[
                  { value: "mp3", label: "mp3" },
                  { value: "wav16", label: "wav 16-bit" },
                  { value: "wav24", label: "wav 24-bit" },
                  { value: "wav32", label: "wav 32-bit" },
                ]}
                onChange={(v) => set("outputFormat", v)}
              />
            </div>
          </div>
          <p className="text-[11px] leading-relaxed text-ink-500">
            Turbo resolves to 8 steps, SFT and base to 50. Guidance above 1.0
            is ignored on turbo models. The 4B planner has no Q4_K_M because
            that quant breaks audio code generation.
          </p>
        </section>
      )}

      {/* Studio panels sit below the form rather than inside it. These are
          whole tools, not fields; interleaving them pushed the advanced
          sampling controls a screen away from everything else. */}
      {onBuildSection && (
        <SongBuilder
          tracks={tracks}
          running={!running}
          onBuildSection={onBuildSection}
        />
      )}

      {onStemLayer && (
        <StemBuilder
          tracks={tracks}
          running={!running}
          onRenderStem={onStemLayer}
        />
      )}

      <SeedManager
        currentSeed={params.seed}
        onLoad={(seed) => set("seed", seed)}
      />

    </div>
  );
}

/** Text input backed by suggestions. The model accepts any value, so this is
 *  a datalist rather than a closed select — presets guide, they do not
 *  constrain. */
function PickField({
  id,
  label,
  value,
  placeholder,
  onChange,
  suggestions,
  inputMode,
}: {
  id: string;
  label: string;
  value: string;
  placeholder: string;
  onChange: (v: string) => void;
  suggestions: { value: string; label: string }[];
  inputMode?: "numeric";
}) {
  return (
    <div>
      <label className="label" htmlFor={id}>
        {label}
      </label>
      <input
        id={id}
        list={`${id}-suggestions`}
        value={value}
        placeholder={placeholder}
        inputMode={inputMode}
        spellCheck={false}
        onChange={(e) => onChange(e.target.value)}
        className="field mt-1.5"
      />
      <datalist id={`${id}-suggestions`}>
        {suggestions.map((s) => (
          <option key={s.value} value={s.value}>
            {s.label}
          </option>
        ))}
      </datalist>
    </div>
  );
}

/** Language is an enum the planner constrains, so a closed select is correct
 *  here rather than a free-text field. */
function LanguageField({
  value,
  onChange,
  languages,
}: {
  value: string;
  onChange: (v: string) => void;
  languages: { code: string; label: string }[];
}) {
  const auto = value === "";
  return (
    <div>
      <Select
        label="Vocal language"
        value={value}
        options={[
          { value: "en", label: "English" },
          { value: "", label: "auto — let the planner guess" },
          ...languages
            .filter((l) => l.code !== "en")
            .map((l) => ({ value: l.code, label: l.label })),
        ]}
        onChange={onChange}
      />
      <p className="mt-1 text-[10px] text-ink-500">
        {auto
          ? "the planner guesses from the caption, and guesses badly"
          : "constrains the planner to this language"}
      </p>
    </div>
  );
}

function ModelPicker({
  label,
  hint,
  value,
  onChange,
  files,
  goToModels,
}: {
  label: string;
  hint: string;
  value: string;
  onChange: (v: string) => void;
  files: WeightFile[];
  goToModels: () => void;
}) {
  // The backend's registry is keyed by the full filename including `.gguf`,
  // so that is what must be sent. Only installed weights are offered: picking
  // an absent file fails at load time with nothing useful to act on.
  const sorted = [...files].sort((a, b) => a.filename.localeCompare(b.filename));
  const selectedMissing = value !== "" && !sorted.some((f) => f.filename === value);
  const none = sorted.length === 0;

  // The 0.6B planner is measurably worse at lyrics, so say so where the choice
  // is made rather than letting it surface as a bad render later.
  const warning =
    label === "Planner LM" && /0\.6/.test(value)
      ? "0.6B writes pseudo-language, not lyrics — measured English output \"Vieler's steps on the mountain high\". Fine for instrumental only."
      : undefined;

  return (
    <div>
      {/* The Select renders its own label; this row only carries the count
          chip beside it. The label used to be here as well, and both appeared. */}
      <div className="flex items-center justify-end gap-2">
        <span
          className={`chip ${
            none
              ? "bg-ink-800 text-ink-500 ring-1 ring-ink-700"
              : "bg-mint-400/15 text-mint-400 ring-1 ring-mint-400/30"
          }`}
        >
          <span className={`size-1.5 rounded-full ${none ? "bg-ink-500" : "bg-mint-400"}`} />
          {none ? "none installed" : `${sorted.length} installed`}
        </span>
      </div>

      {/* Empty means "whatever is resident, else the first registry entry",
          which is not necessarily the one you want, so it is an explicit
          option rather than a blank row. */}
      <Select
        label={label}
        value={selectedMissing ? "" : value}
        disabled={none}
        options={[
          { value: "", label: "auto (backend default)" },
          ...sorted.map((file) => ({
            value: file.filename,
            label: `${file.variant} · ${file.quant}`,
          })),
        ]}
        onChange={onChange}
      />

      {selectedMissing ? (
        <p className="mt-1 text-[10px] text-amber-400">
          Selected file is no longer on disk — reverted to auto.
        </p>
      ) : warning ? (
        <p className="mt-1 text-[10px] leading-relaxed text-amber-400">
          {warning}
        </p>
      ) : none ? (
        <button
          type="button"
          onClick={goToModels}
          className="mt-1 text-[10px] text-glow-300 underline underline-offset-2"
        >
          Get weights from the Models tab
        </button>
      ) : (
        <p className="mt-1 text-[10px] text-ink-500">{hint}</p>
      )}
    </div>
  );
}

/**
 * Short, stable label for a track in a `<select>`.
 *
 * The planner writes CoT captions that run to several hundred words, and a
 * `<select>` renders every option's full text: using the caption directly made
 * the dropdown grow tall enough to cover the whole window. The id is already
 * the slugified title or caption, so it is the short form by construction.
 */
function trackLabel(t: Track): string {
  const base = (t.caption || "").trim().replace(/\s+/g, " ");
  if (base) return base.length > 48 ? `${base.slice(0, 48)}…` : base;
  return t.id || "untitled";
}

/** Task type, source audio, and the repaint region.
 *
 *  Kept out of the main form because it is conditional: three of the four task
 *  types cannot run without a source track, and offering them unconditionally
 *  just produces a backend error the user has to decode.
 */
function TaskSection({
  tracks,
  params,
  setParams,
  catalog,
  imported,
  onImported,
}: {
  tracks: Track[];
  params: GenerateParams;
  setParams: React.Dispatch<React.SetStateAction<GenerateParams>>;
  catalog: WeightFile[];
  imported: ImportedFile[];
  onImported: (files: ImportedFile[]) => void;
}) {
  /** Turbo cannot serve a stem task coherently. Derived from the chosen
   *  filename, the same way the sampling warning already is. */
  const ditIsTurbo = /^turbo/.test(
    catalog.find((f) => f.filename === params.synthModel)?.variant ?? "",
  );
  const stemTask = STEM_TASKS.has(params.taskType);
  const turboBlocked = stemTask && ditIsTurbo;

  /** First installed non-turbo DiT, for the one-click fix. */
  const baseModel = catalog.find(
    (f) =>
      f.role === "Dit" &&
      f.present &&
      !/^turbo/.test(f.variant) &&
      /sft|base|xl/.test(f.variant),
  );
  const set = useCallback(
    <K extends keyof GenerateParams>(key: K, value: GenerateParams[K]) =>
      setParams((p) => ({ ...p, [key]: value })),
    [setParams],
  );

  const task = TASK_TYPES.find((t) => t.id === params.taskType) ?? TASK_TYPES[0];
  const needsSource = task.needsSource;
  const stemMode = "stem" in task ? task.stem : null;

  /** " | " is the separator `validate_track_names` splits on, and only
   *  `complete` takes more than one. */
  const stems = params.track.split(" | ").filter(Boolean);
  const toggleStem = (name: string) => {
    const next = stems.includes(name)
      ? stems.filter((n) => n !== name)
      : [...stems, name];
    set("track", next.join(" | "));
  };

  // Imports live in `library/imports`, which `list_tracks` deliberately does not
  // walk, so an imported path is in neither list on its own. Checking only
  // `tracks` made this clear the selection the instant a file was imported,
  // which is why "cover" kept reporting that it needed a source track.
  // `source` stays a Track because repaint needs its caption and timings; an
  // import has no metadata, which is what `sourceExists` is for.
  const source = tracks.find((t) => t.path === params.sourceAudio) ?? null;
  const sourceExists =
    !!source || imported.some((f) => f.path === params.sourceAudio);
  // On by default: a repaint almost always wants the source's own style, and a
  // caption left over from the previous render is the wrong one far more often
  // than not.
  const [reuseSourceStyle, setReuseSourceStyle] = useState(true);
  useEffect(() => {
    if (params.sourceAudio && !sourceExists) set("sourceAudio", "");
  }, [params.sourceAudio, sourceExists, set]);

  const endAt =
    params.repaintingEnd < 0 ? null : params.repaintingEnd;

  return (
    <div className="space-y-3 rounded-xl bg-ink-900/40 p-3 ring-1 ring-ink-800">
      <div>
        <Select
          label="Task"
          width="md"
          value={params.taskType}
          options={TASK_TYPES.map((t) => ({ value: t.id, label: t.label }))}
          onChange={(v) => set("taskType", v)}
        />
      </div>

      {turboBlocked && (
        <div className="rounded-lg bg-amber-400/10 px-3 py-2 text-[11px] text-amber-200 ring-1 ring-amber-400/30">
          <p>
            <strong>{task.label}</strong> needs a base or SFT model. The
            backend only logs “turbo output incoherent” and renders it anyway,
            so the result would be mud rather than a clean stem.
          </p>
          {baseModel ? (
            <button
              onClick={() => set("synthModel", baseModel.filename)}
              className="mt-2 chip bg-amber-400/20 text-amber-100 ring-1 ring-amber-400/40 hover:brightness-110"
            >
              switch to {baseModel.variant} · {baseModel.quant}
            </button>
          ) : (
            <p className="mt-1 text-amber-300/80">
              No SFT or base DiT installed. Pick a different task, or install
              one from Models.
            </p>
          )}
        </div>
      )}

      {needsSource && (
        <div className="space-y-3">
          <SourcePicker
            value={params.sourceAudio}
            onChange={(p) => set("sourceAudio", p)}
            label="Source audio"
            placeholder="Import or choose a file…"
            describe={(f) => `${f.name} · ${formatBytes(f.bytes)}`}
            onFiles={onImported}
          />

          {stemMode === "single" && (
            <div>
              <Select
                label="Stem"
                value={params.track}
                options={[
                  { value: "", label: "Unnamed stem" },
                  ...STEM_NAMES.map((n) => ({
                    value: n as string,
                    label: n.replace("_", " "),
                  })),
                ]}
                onChange={(v) => set("track", v)}
              />
              <p className="mt-1 text-[10px] leading-relaxed text-ink-500">
                Becomes the instruction the model is given:{" "}
                <span className="font-mono text-ink-400">
                  {task.id === "extract"
                    ? `“Extract the ${params.track || "…"} from the audio”`
                    : `“Generate the ${params.track || "…"} track based on the audio context”`}
                </span>
                . These twelve are the only names the backend recognises.
              </p>
            </div>
          )}

          {stemMode === "multi" && (
            <div>
              <span className="label">Stems to complete</span>
              <div className="mt-1.5 flex flex-wrap gap-1.5">
                {STEM_NAMES.map((n) => {
                  const on = stems.includes(n);
                  return (
                    <button
                      key={n}
                      type="button"
                      onClick={() => toggleStem(n)}
                      className={`rounded-full px-2.5 py-1 text-[11px] transition ${
                        on
                          ? "bg-glow-500/20 text-glow-200 ring-1 ring-glow-500/45"
                          : "bg-ink-850 text-ink-400 ring-1 ring-ink-700 hover:ring-ink-600"
                      }`}
                    >
                      {n.replace("_", " ")}
                    </button>
                  );
                })}
              </div>
              <p className="mt-1.5 text-[10px] leading-relaxed text-ink-500">
                <span className="font-mono text-ink-400">
                  “Complete the input track with {params.track || "…"}”
                </span>
                . This is the only task that takes more than one stem; they are
                joined with{" "}
                <span className="font-mono">" | "</span> on the way in.
              </p>
            </div>
          )}

          {params.taskType === "repaint" && source && (
            <>
              <RepaintRegion
                track={source}
                start={params.repaintingStart}
                end={endAt}
                onChange={(s, e) => {
                  setParams((p) => ({
                    ...p,
                    repaintingStart: s,
                    repaintingEnd: e ?? -1,
                  }));
                }}
              />

              {/* Style reuse. A repaint usually wants the source's own style,
                  not a retyped version of it, and leaving this on retypes
                  whatever the caption box happened to hold from the previous
                  render -- which is worse than either extreme. */}
              <div className="flex items-center justify-between gap-2 rounded-lg bg-ink-900 px-3 py-2">
                <label className="flex items-center gap-2 text-[11px] text-ink-300">
                  <input
                    type="checkbox"
                    checked={reuseSourceStyle}
                    onChange={(e) => setReuseSourceStyle(e.target.checked)}
                    className="accent-glow-500"
                  />
                  reuse the source&apos;s style
                </label>
                {reuseSourceStyle && (
                  <button
                    onClick={() =>
                      setParams((p) => ({ ...p, caption: source.caption }))
                    }
                    className="chip px-2 py-1 text-[10px]"
                    title={
                      source.caption
                        ? `Fill the caption with "${source.caption.slice(0, 60)}"`
                        : "This track has no stored caption"
                    }
                    disabled={!source.caption}
                  >
                    {params.caption === source.caption ? "filled" : "fill caption"}
                  </button>
                )}
              </div>
              {reuseSourceStyle && params.caption !== source.caption && (
                <p className="text-[10px] text-amber-400/80">
                  The caption still differs from the source&apos;s, so the
                  repaint will use yours rather than its style.
                </p>
              )}
              <div>
                <label className="label" htmlFor="cover-strength">
                  Keep source
                </label>
                <input
                  id="cover-strength"
                  type="range"
                  min={0}
                  max={1}
                  step={0.05}
                  value={params.audioCoverStrength}
                  onChange={(e) =>
                    set("audioCoverStrength", Number(e.target.value))
                  }
                  className="mt-1.5 w-full accent-glow-500"
                />
                <p className="mt-0.5 text-[10px] leading-relaxed text-ink-500">
                  {params.audioCoverStrength.toFixed(2)} — the fraction of
                  diffusion steps that hear the original. Higher sticks closer
                  to it.
                </p>
              </div>
            </>
          )}
        </div>
      )}
    </div>
  );
}

/** Save and recall parameter bundles.
 *
 *  Separate from the built-in `GenerationPreset` chips: those are curated and
 *  owned by the app, these are whatever the form happened to hold.
 */
function UserPresetBar({
  params,
  restore,
}: {
  params: GenerateParams;
  restore: (saved: GenerateParams) => void;
}) {
  const [saved, setSaved] = useState<UserPreset[]>([]);
  const [label, setLabel] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const reload = useCallback(async () => {
    try {
      setSaved(await userPresets.list());
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    void reload();
  }, [reload]);

  async function save() {
    if (!label.trim()) return;
    setBusy(true);
    setError(null);
    try {
      await userPresets.save(label, params);
      setLabel("");
      await reload();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function remove(id: string) {
    setBusy(true);
    try {
      await userPresets.remove(id);
      await reload();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="space-y-2">
      <label className="label">Your presets</label>
      <div className="flex gap-2">
        <input
          value={label}
          onChange={(e) => setLabel(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") void save();
          }}
          placeholder="name these settings"
          disabled={busy}
          className="field min-w-0 flex-1"
        />
        <button
          onClick={() => void save()}
          disabled={busy || !label.trim()}
          className="btn btn-ghost px-3 py-1.5 text-xs disabled:opacity-40"
        >
          save
        </button>
      </div>

      {/* Saved settings load from a dropdown rather than as chips, matching the
          built-in presets. The stored caption is deliberately not restored: it
          is what the user was thinking about when saving, not a setting. */}
      <Select
        label="Load a saved preset"
        placeholder="none saved yet"
        value=""
        disabled={busy}
        options={[
          { value: "", label: "none saved yet" },
          ...saved.map((p) => ({ value: p.id, label: p.label })),
        ]}
        onChange={(id) => {
          const preset = saved.find((p) => p.id === id);
          if (preset) restore(preset.params);
        }}
      />

      {saved.length > 0 && (
        <div className="flex flex-wrap items-center gap-1.5">
          {saved.map((p) => (
            <span
              key={p.id}
              className="inline-flex items-center gap-1 rounded-full bg-ink-800 ring-1 ring-ink-700"
            >
              <button
                onClick={() => restore(p.params)}
                disabled={busy}
                className="px-2.5 py-1 text-[11px] text-ink-200 hover:text-ink-100 disabled:opacity-40"
              >
                {p.label}
              </button>
              <button
                onClick={() => void remove(p.id)}
                disabled={busy}
                aria-label={`Delete ${p.label}`}
                className="px-1.5 py-1 text-[11px] text-ink-500 hover:text-rose-300 disabled:opacity-40"
              >
                ×
              </button>
            </span>
          ))}
        </div>
      )}

      {error && <p className="text-[11px] text-rose-300">{error}</p>}
    </div>
  );
}

/** Voice reference.
 *
 *  ACE-Step has no gender, speaker or voice field -- `AceRequest` does not
 *  define one, and a caption adjective like "female vocal" is only a hint the
 *  planner may or may not honour. The real control is timbre conditioning: the
 *  server VAE-encodes a reference track and feeds it to the DiT as `ref_audio`,
 *  which works on plain text-to-music as well as on cover and repaint.
 */
function VoiceSection({
  tracks,
  params,
  setParams,
  imported,
  onImported,
}: {
  tracks: Track[];
  params: GenerateParams;
  setParams: React.Dispatch<React.SetStateAction<GenerateParams>>;
  imported: ImportedFile[];
  onImported: (files: ImportedFile[]) => void;
}) {
  const set = useCallback(
    <K extends keyof GenerateParams>(key: K, value: GenerateParams[K]) =>
      setParams((p) => ({ ...p, [key]: value })),
    [setParams],
  );

  // Same for a reference track: an import is not in `tracks`, so it has to be
  // checked against the import list too or it is dropped the moment it is set.
  const reference = tracks.find((t) => t.path === params.refAudio) ?? null;
  const referenceExists =
    !!reference || imported.some((f) => f.path === params.refAudio);
  useEffect(() => {
    if (params.refAudio && !referenceExists) set("refAudio", "");
  }, [params.refAudio, referenceExists, set]);

  return (
    <div className="space-y-2 rounded-xl bg-ink-900/40 p-3 ring-1 ring-ink-800">
      <SourcePicker
        value={params.refAudio}
        onChange={(p) => set("refAudio", p)}
        label="Voice reference"
        placeholder="No reference — voice follows the caption"
        describe={(f) => `timbre of ${f.name}`}
        onFiles={onImported}
      />
      <p className="text-[10px] leading-relaxed text-ink-500">
        {reference ? (
          <>
            Copying the vocal timbre of{" "}
            <span className="text-ink-300">{trackLabel(reference)}</span>.
            This conditions the model directly, so it holds far better than
            describing a voice in the caption. Use a short, clean vocal-only
            clip.
          </>
        ) : (
          <>
            There is no voice or gender control in the model. To describe one,
            put it in the caption — “sweet female vocal”, “deep male baritone” —
            and reroll if it misses.
          </>
        )}
      </p>
    </div>
  );
}

/** Male / female / no preference.
 *
 *  ACE-Step has no gender parameter — `AceRequest` does not define one and no
 *  DiT weight implements one. This appends the literal words "Male vocals" or
 *  "Female vocals" to the caption, which is exactly what the reference UI
 *  does, and the only channel the planner has to work from. The hint is
 *  applied in Rust so the caption stored as metadata stays what you typed.
 */
function VocalGenderToggle({
  params,
  setParams,
}: {
  params: GenerateParams;
  setParams: React.Dispatch<React.SetStateAction<GenerateParams>>;
}) {
  const options = [
    { id: "", label: "no preference" },
    { id: "male", label: "male" },
    { id: "female", label: "female" },
  ];
  return (
    <div className="flex items-center gap-2">
      <span className="label shrink-0">Vocal gender</span>
      <div className="flex gap-1.5">
        {options.map((o) => {
          const active = params.vocalGender === o.id;
          return (
            <button
              key={o.id || "any"}
              onClick={() => setParams((p) => ({ ...p, vocalGender: o.id }))}
              className={`rounded-lg px-3 py-1 text-[11px] transition ${
                active
                  ? "bg-glow-500/20 text-glow-200 ring-1 ring-glow-500/45"
                  : "bg-ink-850 text-ink-400 ring-1 ring-ink-700 hover:ring-ink-600"
              }`}
            >
              {o.label}
            </button>
          );
        })}
      </div>
      <p className="text-[10px] leading-relaxed text-ink-500">
        {params.vocalGender
          ? "Appended to the caption as words — the model has no gender control."
          : "Add it to the caption yourself if you want it."}
      </p>
    </div>
  );
}

/** LoRA adapter picker.
 *
 *  The registry is whatever `ace-server` found under `<models>/adapters` at
 *  startup, so an adapter added while the server runs needs a restart before
 *  it appears here.
 */
function AdapterSection({
  params,
  setParams,
  port,
}: {
  params: GenerateParams;
  setParams: React.Dispatch<React.SetStateAction<GenerateParams>>;
  port: number | null;
}) {
  const [available, setAvailable] = useState<string[]>([]);

  useEffect(() => {
    let alive = true;
    // Guarded for the same reason as everywhere else: a throw here unmounts
    // the subtree and blanks the panel.
    try {
      void adapters
        .list(port ?? undefined)
        .then((list) => {
          if (alive) setAvailable(list);
        })
        .catch(() => {
          if (alive) setAvailable([]);
        });
    } catch {
      if (alive) setAvailable([]);
    }
    return () => {
      alive = false;
    };
  }, [port]);

  return (
    <details className="space-y-2 rounded-xl bg-ink-900/40 p-3 ring-1 ring-ink-800">
      <summary className="cursor-pointer text-[11px] text-ink-300">
        LoRA
        {params.adapter && (
          <span className="ml-2 text-glow-300">{params.adapter}</span>
        )}
      </summary>

      <div className="space-y-2 pt-1">
        <Select
          label="Adapter"
          value={params.adapter}
          options={[
            { value: "", label: "No adapter" },
            ...available.map((a) => ({ value: a, label: a })),
          ]}
          onChange={(v) => setParams((p) => ({ ...p, adapter: v }))}
        />

        {available.length === 0 && (
          <p className="text-[10px] leading-relaxed text-ink-500">
            None found. Put <span className="font-mono">.safetensors</span>{" "}
            files, or subdirectories holding{" "}
            <span className="font-mono">adapter_model.safetensors</span>, into{" "}
            <span className="font-mono">&lt;models&gt;/adapters</span> and
            restart the runtime.
          </p>
        )}

        {params.adapter && (
          <div className={params.adapterScale <= 0 ? "opacity-40" : ""}>
            <div className="flex items-baseline justify-between gap-2">
              <label className="label" htmlFor="adapter-scale">
                Scale
              </label>
              <span className="font-mono text-xs text-ink-200">
                {params.adapterScale.toFixed(2)}
              </span>
            </div>
            <input
              id="adapter-scale"
              type="range"
              min={0}
              max={2}
              step={0.05}
              value={params.adapterScale}
              onChange={(e) =>
                setParams((p) => ({
                  ...p,
                  adapterScale: Number(e.target.value),
                }))
              }
              className="mt-1.5 w-full accent-glow-500"
            />
            <p className="mt-0.5 text-[10px] text-ink-500">
              How strongly the adapter pulls. 0 disables it.
            </p>
          </div>
        )}
      </div>
    </details>
  );
}

/** Planner sampling and passthrough.
 *
 *  Everything here was previously hardcoded in `to_lm_json` or never sent, so
 *  the reference UI's equivalents could not have worked.
 */
function LmSamplingPanel({
  params,
  setParams,
}: {
  params: GenerateParams;
  setParams: React.Dispatch<React.SetStateAction<GenerateParams>>;
}) {
  const [open, setOpen] = useState(false);
  const set = useCallback(
    <K extends keyof GenerateParams>(key: K, value: GenerateParams[K]) =>
      setParams((p) => ({ ...p, [key]: value })),
    [setParams],
  );

  return (
    <details
      className="space-y-3 rounded-xl bg-ink-900/40 p-3 ring-1 ring-ink-800"
      open={open}
      onToggle={(e) => setOpen((e.currentTarget as HTMLDetailsElement).open)}
    >
      <summary className="cursor-pointer text-[11px text-ink-300]">
        Planner sampling &amp; passthrough
      </summary>

      <div className="flex items-center justify-between gap-3 pt-1">
        <div>
          <p className="text-xs text-ink-200">CoT caption</p>
          <p className="text-[10px] text-ink-500">
            Let the planner rewrite your caption as chain-of-thought before
            the model sees it. Off passes it through verbatim.
          </p>
        </div>
        <button
          role="switch"
          aria-checked={params.useCotCaption}
          aria-label="CoT caption"
          onClick={() => set("useCotCaption", !params.useCotCaption)}
          className={`relative h-5 w-9 shrink-0 rounded-full transition ${
            params.useCotCaption ? "bg-glow-500" : "bg-ink-700"
          }`}
        >
          <span
            className={`absolute top-0.5 size-4 rounded-full bg-white transition-all ${
              params.useCotCaption ? "left-4.5" : "left-0.5"
            }`}
          />
        </button>
      </div>

      <div className="grid grid-cols-2 gap-4 lg:grid-cols-4">
        <EditableSlider
          label="Temperature"
          value={params.lmTemperature}
          min={0}
          max={2}
          step={0.01}
          onChange={(v) => set("lmTemperature", v)}
          display={(v) => v.toFixed(2)}
        />
        <EditableSlider
          label="CFG scale"
          value={params.lmCfgScale}
          min={0}
          max={10}
          step={0.1}
          onChange={(v) => set("lmCfgScale", v)}
          display={(v) => v.toFixed(1)}
        />
        <EditableSlider
          label="Top-p"
          value={params.lmTopP}
          min={0}
          max={1}
          step={0.01}
          onChange={(v) => set("lmTopP", v)}
          display={(v) => v.toFixed(2)}
        />
        <EditableSlider
          label="Top-k"
          value={params.lmTopK}
          min={0}
          max={200}
          onChange={(v) => set("lmTopK", Math.round(v))}
          display={(v) => (v === 0 ? "auto" : String(v))}
        />
      </div>

      <div>
        <label className="label" htmlFor="lm-negative">
          Negative prompt
        </label>
        <textarea
          id="lm-negative"
          rows={2}
          value={params.lmNegativePrompt}
          onChange={(e) => set("lmNegativePrompt", e.target.value)}
          placeholder="Things to avoid…"
          className="field mt-1.5 resize-y text-xs"
        />
      </div>

      <div>
        <label className="label" htmlFor="audio-codes">
          Audio codes
        </label>
        <textarea
          id="audio-codes"
          rows={2}
          value={params.audioCodes}
          onChange={(e) => set("audioCodes", e.target.value)}
          placeholder="Optional — comma-separated codes to condition on"
          spellCheck={false}
          className="field mt-1.5 resize-y font-mono text-[10px]"
        />
        <p className="mt-1 text-[10px] leading-relaxed text-ink-500">
          Bypasses the planner's arrangement and puts the model into cover mode.
          Leave empty unless you are iterating on a specific arrangement.
        </p>
      </div>
    </details>
  );
}

/** Load a parameter bundle from a JSON file.
 *
 *  Accepts anything that looks like `GenerateParams`, in either casing, and
 *  takes only the keys it recognises. Presets written by the reference UI or
 *  exported by the backend therefore load, and an unknown key is ignored
 *  rather than throwing the whole file away.
 */
function LoadParamsButton({
  setParams,
}: {
  setParams: React.Dispatch<React.SetStateAction<GenerateParams>>;
}) {
  const inputRef = useRef<HTMLInputElement>(null);
  const [error, setError] = useState<string | null>(null);

  async function onFile(e: React.ChangeEvent<HTMLInputElement>) {
    const file = e.target.files?.[0];
    if (!file) return;
    setError(null);
    try {
      const text = await file.text();
      const parsed: unknown = JSON.parse(text);
      if (typeof parsed !== "object" || parsed === null) {
        throw new Error("not a JSON object");
      }
      // Presets may wrap the params under `params`.
      const raw = (parsed as Record<string, unknown>).params ?? parsed;
      const snakeToCamel: Record<string, string> = {
        caption: "caption",
        lyrics: "lyrics",
        duration: "duration",
        bpm: "bpm",
        keyscale: "keyscale",
        timesignature: "timesignature",
        vocal_language: "vocalLanguage",
        inference_steps: "inferenceSteps",
        guidance_scale: "guidanceScale",
        shift: "shift",
        seed: "seed",
        output_format: "outputFormat",
        synth_batch_size: "synthBatchSize",
        lm_model: "lmModel",
        synth_model: "synthModel",
        solver: "solver",
        task_type: "taskType",
        lm_temperature: "lmTemperature",
        lm_cfg_scale: "lmCfgScale",
        lm_top_p: "lmTopP",
        lm_top_k: "lmTopK",
        lm_negative_prompt: "lmNegativePrompt",
        use_cot_caption: "useCotCaption",
        audio_codes: "audioCodes",
        adapter: "adapter",
        adapter_scale: "adapterScale",
        track: "track",
        vocal_gender: "vocalGender",
        repainting_start: "repaintingStart",
        repainting_end: "repaintingEnd",
        audio_cover_strength: "audioCoverStrength",
        title: "title",
      };
      setParams((current) => {
        const next = { ...current };
        let applied = 0;
        for (const [key, value] of Object.entries(raw as Record<string, unknown>)) {
          const camel = snakeToCamel[key] ?? key;
          // Only keys this build knows about are applied. An unknown key is
          // ignored rather than failing the whole file, so a preset saved by
          // a newer or older version still loads.
          if (camel in next) {
            (next as unknown as Record<string, unknown>)[camel] = value;
            applied += 1;
          }
        }
        if (applied === 0) {
          setError("No recognised parameters in that file.");
        }
        return next;
      });
    } catch (err) {
      setError(String(err));
    } finally {
      // Allow re-picking the same file after fixing it.
      if (inputRef.current) inputRef.current.value = "";
    }
  }

  return (
    <div className="space-y-1">
      <input
        ref={inputRef}
        type="file"
        accept="application/json,.json"
        onChange={(e) => void onFile(e)}
        className="hidden"
      />
      <button
        onClick={() => inputRef.current?.click()}
        className="btn btn-ghost w-full px-3 py-1.5 text-xs"
      >
        Load parameters (JSON)
      </button>
      {error && <p className="text-[11px] text-rose-300">{error}</p>}
    </div>
  );
}
