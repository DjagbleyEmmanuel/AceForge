import { useEffect, useState } from "react";
import { Select } from "./Select";

export interface ThemeDef {
  id: string;
  label: string;
  /** The two accent colours, used for the swatch gradient. */
  swatch: [string, string];
}

export const THEMES: ThemeDef[] = [
  { id: "aurora", label: "Aurora", swatch: ["#8b5cf6", "#34d399"] },
  { id: "ember", label: "Ember", swatch: ["#f97316", "#fbbf24"] },
  { id: "ocean", label: "Ocean", swatch: ["#0ea5e9", "#2dd4bf"] },
  { id: "neon", label: "Neon", swatch: ["#22c55e", "#a3e635"] },
  { id: "orchid", label: "Orchid", swatch: ["#d946ef", "#fb7185"] },
  { id: "solar", label: "Solar", swatch: ["#eab308", "#fb923c"] },
  { id: "spectrum", label: "Spectrum", swatch: ["#06b6d4", "#e879f9"] },
];

const KEY = "aceforge.theme";
const DEFAULT = "aurora";

function readStored(): string {
  try {
    const v = window.localStorage.getItem(KEY);
    // Guard against a stale or hand-edited value naming a theme that no longer
    // exists, which would otherwise leave the app with no accent colour.
    return v && THEMES.some((t) => t.id === v) ? v : DEFAULT;
  } catch {
    return DEFAULT;
  }
}

/**
 * Write the theme onto the document root.
 *
 * The attribute rather than a class because the CSS overrides live in
 * `[data-theme]` rules with higher specificity than the `:root` defaults
 * Tailwind emits.
 */
export function applyTheme(id: string): void {
  document.documentElement.setAttribute("data-theme", id);
}

export function useTheme(): [string, (id: string) => void] {
  const [theme, setThemeState] = useState<string>(DEFAULT);

  useEffect(() => {
    const stored = readStored();
    applyTheme(stored);
    setThemeState(stored);
  }, []);

  function setTheme(id: string) {
    if (!THEMES.some((t) => t.id === id)) return;
    applyTheme(id);
    setThemeState(id);
    try {
      window.localStorage.setItem(KEY, id);
    } catch {
      // Storage unavailable: the theme still applies for this session.
    }
  }

  return [theme, setTheme];
}

export function ThemeChooser({
  theme,
  onChange,
}: {
  theme: string;
  onChange: (id: string) => void;
}) {
  const current = THEMES.find((t) => t.id === theme) ?? THEMES[0];

  // The swatches ride along as leading marks inside the same listbox the rest
  // of the app uses, so the theme picker is not the one control in the sidebar
  // that opens a different kind of menu.
  return (
    <div className="panel p-3">
      <Select
        size="sm"
        label="Theme"
        value={theme}
        options={THEMES.map((t) => ({
          value: t.id,
          label: t.label,
          swatch: t.swatch,
        }))}
        onChange={onChange}
      />
      <div
        className="mt-2 h-1 w-full rounded-full"
        style={{
          background: `linear-gradient(135deg, ${current.swatch[0]}, ${current.swatch[1]})`,
        }}
      />
    </div>
  );
}
