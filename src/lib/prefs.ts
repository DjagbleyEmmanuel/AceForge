/**
 * Small UI preferences that should survive a restart.
 *
 * Deliberately separate from generation settings. Those are decisions made per
 * render; these are how the window was left -- how the library was sorted,
 * whether the log was following, whether the lyrics were open. Losing them
 * means the app rearranges itself under the user, which is the same complaint
 * as a control that does not remember.
 *
 * Backed by localStorage, and every read is guarded: a webview with storage
 * disabled should fall back to the defaults rather than fail to start.
 */

const PREFIX = "aceforge.ui.";

function read<T>(key: string, fallback: T): T {
  try {
    const raw = window.localStorage.getItem(PREFIX + key);
    return raw === null ? fallback : (JSON.parse(raw) as T);
  } catch {
    return fallback;
  }
}

export function readFlag(key: string, fallback: boolean): boolean {
  return read<boolean>(key, fallback);
}

export function writeFlag(key: string, value: boolean): void {
  try {
    window.localStorage.setItem(PREFIX + key, JSON.stringify(value));
  } catch {
    // Storage unavailable; the preference lasts for this session only.
  }
}

export function readText(key: string, fallback: string): string {
  return read<string>(key, fallback);
}

export function writeText(key: string, value: string): void {
  try {
    window.localStorage.setItem(PREFIX + key, JSON.stringify(value));
  } catch {
    // As above.
  }
}
