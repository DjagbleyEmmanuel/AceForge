import {
  useCallback,
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
} from "react";

export interface SelectOption<T extends string> {
  value: T;
  label: string;
  /** Small print under the label: what this option actually does. */
  note?: string;
  /** Right-aligned metadata, e.g. an installed-model count. */
  meta?: string;
  /** Two colours rendered as a small gradient square before the label. */
  swatch?: [string, string];
  disabled?: boolean;
}

interface Props<T extends string> {
  value: T;
  options: SelectOption<T>[];
  onChange: (value: T) => void;
  label?: string;
  /** Hint under the trigger; the model pickers use this for their caption. */
  note?: string;
  placeholder?: string;
  disabled?: boolean;
  size?: "sm" | "md";
  /** How wide the control takes. Preset pickers are narrow by nature and a
   *  full-panel-wide box for a one-word choice looks empty. */
  width?: "auto" | "sm" | "md" | "lg" | "full";
}

/**
 * Themed dropdown.
 *
 * Replaces the native `<select>` throughout the app because WebKitGTK draws
 * those with the platform widget style: a flat grey box with a system arrow and
 * a light popup list that ignores every colour in the design. None of that is
 * reachable from CSS -- `appearance: none` hides the arrow and nothing else --
 * so the control is built from a button and a listbox instead.
 *
 * The list is portalled-free but flipped when there is no room below, because a
 * clipped menu in a scrollable panel is worse than an awkward one.
 */
export function Select<T extends string>({
  value,
  options,
  onChange,
  label,
  note,
  placeholder = "select…",
  disabled,
  size = "md",
  width = "full",
}: Props<T>) {
  const [open, setOpen] = useState(false);
  const [activeIndex, setActiveIndex] = useState(0);
  const [flip, setFlip] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const listId = useId();

  const selected = options.find((o) => o.value === value);

  const close = useCallback(() => {
    setOpen(false);
  }, []);

  // Clicking anywhere else closes the menu, including on the trigger itself so
  // a second click does not immediately reopen it.
  useEffect(() => {
    if (!open) return;
    const onPointer = (e: PointerEvent) => {
      if (!rootRef.current?.contains(e.target as Node)) close();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        close();
        triggerRef.current?.focus();
      }
    };
    document.addEventListener("pointerdown", onPointer);
    document.addEventListener("keydown", onKey, true);
    return () => {
      document.removeEventListener("pointerdown", onPointer);
      document.removeEventListener("keydown", onKey, true);
    };
  }, [open, close]);

  // Keep the highlighted option scrolled into view while arrowing through.
  useEffect(() => {
    if (!open) return;
    const list = listRef.current;
    const item = list?.querySelector<HTMLElement>(`[data-index="${activeIndex}"]`);
    item?.scrollIntoView({ block: "nearest" });
  }, [open, activeIndex]);

  // Measure before opening: if the menu would run past the bottom of the
  // viewport, open it upwards instead.
  useLayoutEffect(() => {
    if (!open) return;
    const rect = rootRef.current?.getBoundingClientRect();
    if (!rect) return;
    const needed = Math.min(options.length, 8) * 34 + 12;
    setFlip(window.innerHeight - rect.bottom < needed && rect.top > needed);
  }, [open, options.length]);

  function openMenu() {
    const i = options.findIndex((o) => o.value === value);
    setActiveIndex(i >= 0 ? i : 0);
    setOpen(true);
  }

  function commit(i: number) {
    const option = options[i];
    if (!option || option.disabled) return;
    onChange(option.value);
    close();
    triggerRef.current?.focus();
  }

  function onKeyDown(e: React.KeyboardEvent) {
    if (disabled) return;
    if (!open) {
      if (["ArrowDown", "ArrowUp", "Enter", " "].includes(e.key)) {
        e.preventDefault();
        openMenu();
      }
      return;
    }
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setActiveIndex((i) => Math.min(options.length - 1, i + 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setActiveIndex((i) => Math.max(0, i - 1));
    } else if (e.key === "Home") {
      e.preventDefault();
      setActiveIndex(0);
    } else if (e.key === "End") {
      e.preventDefault();
      setActiveIndex(options.length - 1);
    } else if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      commit(activeIndex);
    }
  }

  const pad = size === "sm" ? "px-2 py-1 text-[11px]" : "px-3 py-2 text-[13px]";
  const widthClass = {
    auto: "w-auto",
    sm: "w-28",
    md: "w-44",
    lg: "w-64",
    full: "w-full",
  }[width];

  return (
    <div ref={rootRef} className={`relative ${widthClass}`}>
      {label && (
        <span className="label mb-1.5 block" id={`${listId}-label`}>
          {label}
        </span>
      )}
      <button
        ref={triggerRef}
        type="button"
        role="combobox"
        aria-expanded={open}
        aria-controls={open ? listId : undefined}
        aria-haspopup="listbox"
        aria-labelledby={label ? `${listId}-label` : undefined}
        disabled={disabled}
        onClick={() => (open ? close() : openMenu())}
        onKeyDown={onKeyDown}
        className={`flex w-full items-center justify-between gap-2 rounded-xl border border-ink-700 bg-ink-900 text-left transition outline-none ${
          pad
        } ${
          disabled
            ? "cursor-not-allowed opacity-45"
            : "hover:border-ink-600 focus:border-glow-500 focus:shadow-[0_0_0_3px_color-mix(in_oklab,var(--color-glow-500)_22%,transparent)]"
        }`}
      >
        <span className="min-w-0 flex-1">
          <span className="flex items-center gap-2">
            {selected?.swatch && (
              <span
                className="size-3.5 shrink-0 rounded"
                style={{
                  background: `linear-gradient(135deg, ${selected.swatch[0]}, ${selected.swatch[1]})`,
                }}
              />
            )}
            <span
              className={`block truncate ${selected ? "text-ink-100" : "text-ink-500"}`}
            >
              {selected?.label ?? placeholder}
            </span>
          </span>
          {selected?.note && (
            <span className="block truncate text-[10px] text-ink-500">
              {selected.note}
            </span>
          )}
        </span>
        <svg
          width="11"
          height="11"
          viewBox="0 0 12 12"
          fill="none"
          stroke="currentColor"
          strokeWidth="1.8"
          strokeLinecap="round"
          strokeLinejoin="round"
          className={`shrink-0 text-ink-500 transition-transform ${open ? "rotate-180" : ""}`}
        >
          <path d="M2.5 4.5L6 8l3.5-3.5" />
        </svg>
      </button>

      {open && (
        <div
          ref={listRef}
          id={listId}
          role="listbox"
          className={`absolute left-0 right-0 z-50 max-h-72 overflow-y-auto overflow-x-hidden rounded-xl border border-ink-700 bg-ink-850 p-1 shadow-2xl shadow-black/60 ${
            flip ? "bottom-full mb-1.5" : "top-full mt-1.5"
          }`}
        >
          {options.map((option, i) => {
            const active = option.value === value;
            return (
              <button
                key={option.value}
                type="button"
                role="option"
                aria-selected={active}
                data-index={i}
                disabled={option.disabled}
                onPointerEnter={() => setActiveIndex(i)}
                onClick={() => commit(i)}
                className={`block w-full min-w-0 overflow-hidden rounded-lg px-2.5 py-1.5 text-left transition ${
                  option.disabled
                    ? "cursor-not-allowed opacity-40"
                    : i === activeIndex
                      ? "bg-glow-500/15 text-ink-100"
                      : "text-ink-300 hover:bg-ink-800"
                }`}
              >
                <span className="flex items-center justify-between gap-3">
                  <span className="flex min-w-0 flex-1 items-center gap-2">
                    {option.swatch && (
                      <span
                        className="size-3.5 shrink-0 rounded"
                        style={{
                          background: `linear-gradient(135deg, ${option.swatch[0]}, ${option.swatch[1]})`,
                        }}
                      />
                    )}
                    <span className="truncate text-[12px]">{option.label}</span>
                  </span>
                  {option.meta && (
                    <span className="shrink-0 font-mono text-[10px] text-ink-500">
                      {option.meta}
                    </span>
                  )}
                  {active && (
                    <svg
                      width="11"
                      height="11"
                      viewBox="0 0 12 12"
                      fill="none"
                      stroke="currentColor"
                      strokeWidth="2"
                      strokeLinecap="round"
                      strokeLinejoin="round"
                      className="shrink-0 text-glow-400"
                    >
                      <path d="M2.5 6.5l2.5 2.5 4.5-5" />
                    </svg>
                  )}
                </span>
                {option.note && (
                  <span className="mt-0.5 block line-clamp-2 text-[10px] leading-snug text-ink-500">
                    {option.note}
                  </span>
                )}
              </button>
            );
          })}
          {options.length === 0 && (
            <p className="px-2.5 py-2 text-[11px] text-ink-500">nothing here yet</p>
          )}
        </div>
      )}

      {note && !selected?.note && (
        <p className="mt-1 text-[10px] leading-relaxed text-ink-500">{note}</p>
      )}
    </div>
  );
}