/**
 * Remembers which files a session is working with.
 *
 * Generation settings are deliberately not persisted -- nobody wants steps and
 * guidance silently reverting to a value they forgot choosing. The file
 * selections are different: they point at things that outlive the process, and
 * losing one means the Generate button goes back to demanding a source track
 * that is already sitting in the imports folder. That reads as the app having
 * forgotten the file, which is exactly the confusion this avoids.
 *
 * Only paths are stored, and only after the file has been confirmed to exist,
 * so a library move cannot leave a dangling pointer.
 */

const KEY = "aceforge.session";

interface Remembered {
  sourceAudio?: string;
  refAudio?: string;
  track?: string;
  title?: string;
}

function read(): Remembered {
  try {
    const raw = window.localStorage.getItem(KEY);
    if (!raw) return {};
    const parsed: unknown = JSON.parse(raw);
    return parsed && typeof parsed === "object" ? (parsed as Remembered) : {};
  } catch {
    return {};
  }
}

export function remember(p: Remembered): void {
  try {
    // Empty values are dropped rather than stored, so clearing a picker does
    // not leave a stale key behind to be restored later.
    const next: Remembered = {};
    for (const [k, v] of Object.entries(p)) {
      if (typeof v === "string" && v.trim()) {
        (next as Record<string, string>)[k] = v;
      }
    }
    window.localStorage.setItem(KEY, JSON.stringify(next));
  } catch {
    // Storage unavailable: the session simply starts fresh next time.
  }
}

export function recall(): Remembered {
  return read();
}

/** Forget everything, used when the remembered file no longer exists. */
export function forget(): void {
  try {
    window.localStorage.removeItem(KEY);
  } catch {
    // Nothing to do.
  }
}
