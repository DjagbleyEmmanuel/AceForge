import {
  useCallback,
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
} from "react";
import { createPortal } from "react-dom";

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
 * The list is portalled to the document body rather than positioned inside the
 * trigger's wrapper. `.panel` carries `backdrop-filter`, which establishes a
 * stacking context, so an absolutely positioned list inside it cannot rise
 * above a sibling panel that comes later in the document -- the model pickers
 * sit above the Song Builder, and its menu was painted underneath it. Portalling
 * also escapes any ancestor that clips, which is the other way a dropdown in a
 * scrollable panel gets cut off. It is positioned from the trigger's viewport
 * rectangle and re-measured while open, so it tracks scrolling.
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
  /** Viewport rectangle the portalled list is drawn at. */
  const [anchor, setAnchor] = useState<{ top: number; left: number; width: number } | null>(
    null,
  );
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
      const target = e.target as Node;
      // The list lives outside the trigger's DOM now, so containment alone
      // would treat a click on an option as an outside click and close the menu
      // before the option could register.
      if (rootRef.current?.contains(target) || listRef.current?.contains(target)) return;
      close();
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

  // Position the portalled list against the trigger, in viewport coordinates,
  // flipping upwards when there is no room below. Re-measured on scroll and
  // resize because the anchor moves while the menu is open.
  useLayoutEffect(() => {
    if (!open) return;
    const measure = () => {
      const rect = triggerRef.current?.getBoundingClientRect();
      if (!rect) return;
      const needed = Math.min(options.length, 8) * 34 + 12;
      const below = window.innerHeight - rect.bottom;
      const goUp = below < needed && rect.top > needed;
      setFlip(goUp);
      setAnchor({
        top: goUp ? rect.top - 6 : rect.bottom + 6,
        left: rect.left,
        width: rect.width,
      });
    };
    measure();
    window.addEventListener("scroll", measure, true);
    window.addEventListener("resize", measure);
    return () => {
      window.removeEventListener("scroll", measure, true);
      window.removeEventListener("resize", measure);
    };
  }, [open, options.length]);

  // Drop the measured rectangle once closed, so reopening re-measures rather
  // than reusing a stale one from a scroll position that has since changed.
  // Only on close: doing it on every change would clear the anchor the layout
  // effect has just set, and the list would never mount.
  useEffect(() => {
    if (!open) setAnchor(null);
  }, [open]);

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

      {/* Portalled: see the note at the top of the file. `fixed` plus a z-index
          high enough to clear any stacking context is what lets the menu paint
          above sibling panels. */}
      {open &&
        anchor &&
        createPortal(
          <div
            ref={listRef}
            id={listId}
            role="listbox"
            style={{
              position: "fixed",
              top: anchor.top,
              left: anchor.left,
              width: anchor.width,
              zIndex: 9999,
              // Opened upwards, the list is anchored at the trigger's top edge
              // and pulled up by its own height, which puts its bottom there.
              transform: flip ? "translateY(-100%)" : undefined,
            }}
            className="max-h-72 overflow-y-auto overflow-x-hidden rounded-xl border border-ink-700 bg-ink-850 p-1 shadow-2xl shadow-black/60"
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
          </div>,
          document.body,
        )}

      {note && !selected?.note && (
        <p className="mt-1 text-[10px] leading-relaxed text-ink-500">{note}</p>
      )}
    </div>
  );
}