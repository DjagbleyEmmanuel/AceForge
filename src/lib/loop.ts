/**
 * Per-track loop state.
 *
 * A shared repeat mode rendered under every card reads as though toggling one
 * file sets them all, which is not what anyone means by "loop this one". Loop is
 * therefore a property of the file, kept here so every player card renders the
 * same switch for its own track and it survives a remount.
 *
 * Backed by localStorage: the library outlives a session often enough that
 * forgetting which take you left on repeat would be its own annoyance. Every
 * access is guarded because a webview with storage disabled should degrade to
 * "off", not to a blank panel.
 */

const KEY = "aceforge.loopTracks";

function readAll(): Set<string> {
  try {
    const raw = window.localStorage.getItem(KEY);
    if (!raw) return new Set();
    const parsed: unknown = JSON.parse(raw);
    return Array.isArray(parsed)
      ? new Set(parsed.filter((v): v is string => typeof v === "string"))
      : new Set();
  } catch {
    return new Set();
  }
}

let cache: Set<string> | null = null;

function all(): Set<string> {
  if (cache === null) cache = readAll();
  return cache;
}

function persist(): void {
  try {
    window.localStorage.setItem(KEY, JSON.stringify([...all()]));
  } catch {
    // Storage full or unavailable. The loop still works for this session; it
    // just will not be remembered next time.
  }
}

export function isLooping(path: string): boolean {
  return all().has(path);
}

export function setLooping(path: string, on: boolean): void {
  const set = all();
  if (on) {
    set.add(path);
  } else {
    set.delete(path);
  }
  persist();
}

/** Flip and return the new state, for a single-button toggle. */
export function toggleLooping(path: string): boolean {
  const next = !isLooping(path);
  setLooping(path, next);
  return next;
}