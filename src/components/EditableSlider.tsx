import { useEffect, useRef, useState } from "react";

interface Props {
  label: string;
  value: number;
  min: number;
  max: number;
  step?: number;
  onChange: (value: number) => void;
  /** Shown instead of the raw number, e.g. `8 (model default)`. */
  display?: (value: number) => string;
  help?: string;
  disabled?: boolean;
}

/**
 * A slider you can also type into.
 *
 * Dragging is far better than a spin box for the sampling controls, but a
 * slider alone cannot express "0 means let the model decide", which is exactly
 * what these fields mean. So the number stays editable and the slider is
 * disabled at the sentinel rather than snapping away from it.
 */
export function EditableSlider({
  label,
  value,
  min,
  max,
  step = 1,
  onChange,
  display,
  help,
  disabled,
}: Props) {
  const [draft, setDraft] = useState<string | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  // Ids must not contain whitespace: "slider-CFG scale" is an invalid id, so
  // `htmlFor` silently failed to associate and clicking the label did nothing.
  const inputId = `slider-${label.toLowerCase().replace(/[^a-z0-9]+/g, "-")}`;

  // While typing, keep the field showing what was typed rather than snapping
  // back to the value on every keystroke.
  useEffect(() => {
    setDraft(null);
  }, [value]);

  const shown = draft ?? (display ? display(value) : String(value));

  function commit(raw: string) {
    const parsed = Number(raw);
    setDraft(null);
    if (raw.trim() === "" || !Number.isFinite(parsed)) return;
    onChange(Math.max(min, Math.min(max, parsed)));
  }

  // Typing is pointless on a disabled control, and a stray draft would freeze
  // the displayed value.
  useEffect(() => {
    if (disabled) setDraft(null);
  }, [disabled]);

  return (
    <div className={`min-w-0 ${disabled ? "opacity-40" : ""}`}>
      {/* `min-w-0` on the label lets it truncate instead of forcing the row
          wider than its column. Without it the fixed-width value box below
          pushed past the column and overlapped the next slider's value. */}
      <div className="flex min-w-0 items-baseline justify-between gap-2">
        <label className="label truncate" htmlFor={inputId}>
          {label}
        </label>
        <input
          ref={inputRef}
          type="text"
          inputMode="decimal"
          value={shown}
          disabled={disabled}
          onChange={(e) => setDraft(e.target.value)}
          onBlur={(e) => commit(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              commit((e.target as HTMLInputElement).value);
              (e.target as HTMLInputElement).blur();
            } else if (e.key === "Escape") {
              setDraft(null);
              (e.target as HTMLInputElement).blur();
            }
          }}
          className="w-16 shrink-0 rounded border border-transparent bg-transparent px-1 py-0.5 text-right font-mono text-xs text-ink-200 outline-none transition hover:border-ink-700 focus:border-glow-500/60 focus:bg-ink-900 disabled:cursor-not-allowed"
        />
      </div>
      <input
        id={inputId}
        type="range"
        min={min}
        max={max}
        step={step}
        value={value}
        disabled={disabled}
        onChange={(e) => onChange(Number(e.target.value))}
        className="mt-1.5 w-full accent-glow-500 disabled:cursor-not-allowed"
        aria-label={label}
      />
      {help && <p className="mt-0.5 text-[10px] leading-relaxed text-ink-500">{help}</p>}
    </div>
  );
}
