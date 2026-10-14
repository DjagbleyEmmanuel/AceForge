import { readFile } from "@tauri-apps/plugin-fs";
import { audioUrl, type Track } from "../lib/api";

/** Resolve playable bytes for a track.
 *
 *  Pointing an <audio> element straight at the Tauri asset protocol is
 *  unreliable on Linux: WebKitGTK does not reliably route media-element loads
 *  through a custom URI scheme handler, so the element fails even though the
 *  same URL works over fetch.
 *
 *  Instead we pull the bytes and hand the element a Blob URL, which is an
 *  ordinary object URL the webview always understands. Two sources, in order:
 *
 *    1. fetch over the asset protocol — no copy through IPC.
 *    2. the fs plugin — the fallback when the scheme handler is unavailable.
 *
 *  The caller owns the returned URL and must revoke it.
 */
export async function loadTrackSource(
  track: Pick<Track, "path" | "format">,
): Promise<{ url: string; via: string }> {
  const url = audioUrl(track);
  const mime = track.format === "wav" ? "audio/wav" : "audio/mpeg";

  try {
    const response = await fetch(url);
    if (response.ok) {
      const blob = await response.blob();
      return { url: URL.createObjectURL(blob), via: "asset protocol" };
    }
    throw new Error(`HTTP ${response.status}`);
  } catch (protocolError) {
    // Fall through: the scheme handler may be absent entirely on this webview.
    try {
      const bytes = await readFile(track.path);
      const blob = new Blob([bytes], { type: mime });
      return { url: URL.createObjectURL(blob), via: "fs plugin" };
    } catch (fsError) {
      throw new Error(
        `could not read ${track.path}\n` +
          `asset protocol: ${String(protocolError)}\n` +
          `fs plugin: ${String(fsError)}`,
      );
    }
  }
}