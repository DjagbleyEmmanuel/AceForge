/**
 * Single-playback coordination for the library.
 *
 * Tracks live in a keep-alive tree, so several `<audio>` elements exist at
 * once. Without a registry, starting one leaves the others playing and the
 * output stacks.
 *
 * Module scope on purpose: it has to outlive individual players, which are
 * unmounted and remounted as the library changes.
 */

let currentId: string | null = null;
let stopCurrent: (() => void) | null = null;

/** Take over playback, stopping whatever was playing first.
 *
 *  The new owner is registered before the previous one is stopped: stopping
 *  fires its own pause event, and the guard in `releasePlayback` must already
 *  see the new id so it does not clear the fresh registration.
 */
export function claimPlayback(id: string, stop: () => void): void {
  if (currentId === id) return;

  const previous = stopCurrent;
  currentId = id;
  stopCurrent = stop;

  if (previous) previous();
}

/** Give up the claim, but only if this player still holds it. A player that
 *  was stopped by someone else must not clear the new owner's registration. */
export function releasePlayback(id: string): void {
  if (currentId !== id) return;
  currentId = null;
  stopCurrent = null;
}

export function isPlayingId(): string | null {
  return currentId;
}