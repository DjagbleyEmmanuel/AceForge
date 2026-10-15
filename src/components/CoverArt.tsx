import { useEffect, useState } from "react";
import { readFile } from "@tauri-apps/plugin-fs";

/**
 * Cover artwork, loaded through the fs plugin rather than the asset protocol.
 *
 * Audio already reads its bytes this way, because WebKitGTK will not reliably
 * route media-element loads through a custom URI scheme. The same webview also
 * refuses to render an `<img>` pointed at `asset://` for a cover outside the
 * library folder, which showed as a broken image on every track. Going through
 * the fs plugin sidesteps the protocol entirely and matches what already works.
 *
 * Covers are small and reused across every render of the library, so the object
 * URLs are cached by path: without that, scrolling the list would re-read every
 * cover on every frame.
 */

const cache = new Map<string, string>();

/** Longest edge of a cached thumbnail.
 *
 *  A folder of artwork is easily half a gigabyte -- individual PNGs run to tens
 *  of megabytes. Handing every one of those to the webview as an object URL
 *  costs the full file per thumbnail, so a library grid takes the process out
 *  of memory and stops painting. Covers are displayed at 80px, so anything
 *  past this is spent bytes.
 */
const THUMB_EDGE = 320;

/** Below this there is nothing worth shrinking. */
const SMALL_ENOUGH = 192 * 1024;

function mimeFor(path: string): string {
  const ext = path.split(".").pop()?.toLowerCase() ?? "";
  switch (ext) {
    case "svg":
      return "image/svg+xml";
    case "jpg":
    case "jpeg":
      return "image/jpeg";
    case "gif":
      return "image/gif";
    case "webp":
      return "image/webp";
    case "avif":
      return "image/avif";
    case "bmp":
      return "image/bmp";
    case "ico":
      return "image/x-icon";
    case "tif":
    case "tiff":
      return "image/tiff";
    default:
      return "application/octet-stream";
  }
}

/** Decode, shrink and cache one cover.
 *
 * The intermediate object URL for the full-size file is revoked immediately:
 * holding both the original and the thumbnail alive for every cover in a grid
 * is what this scaling exists to avoid.
 */
async function load(path: string): Promise<string> {
  const hit = cache.get(path);
  if (hit) return hit;
  const bytes = await readFile(path);
  const blob = new Blob([bytes], { type: mimeFor(path) });
  const source = URL.createObjectURL(blob);
  try {
    // Vector art is already tiny and `createImageBitmap` will not decode SVG in
    // this webview, so it goes through untouched. The same goes for anything
    // already small enough that shrinking it would only cost time.
    if (blob.type === "image/svg+xml" || bytes.byteLength <= SMALL_ENOUGH) {
      cache.set(path, source);
      return source;
    }
    const bitmap = await createImageBitmap(blob);
    const scale = Math.min(1, THUMB_EDGE / Math.max(bitmap.width, bitmap.height));
    const w = Math.max(1, Math.round(bitmap.width * scale));
    const h = Math.max(1, Math.round(bitmap.height * scale));
    const canvas = document.createElement("canvas");
    canvas.width = w;
    canvas.height = h;
    const ctx = canvas.getContext("2d");
    if (!ctx) throw new Error("no 2d context");
    ctx.drawImage(bitmap, 0, 0, w, h);
    bitmap.close();
    const small = await new Promise<Blob | null>((resolve) =>
      canvas.toBlob(resolve, "image/webp", 0.82),
    );
    if (!small) throw new Error("encode failed");
    const url = URL.createObjectURL(small);
    // The full-size object URL is dead once the thumbnail exists; leaving it
    // alive would pin the original file's bytes for the life of the cache.
    URL.revokeObjectURL(source);
    cache.set(path, url);
    return url;
  } catch (e) {
    // A format the webview will not rasterise is still better shown as-is than
    // not shown at all, which is indistinguishable from a missing file.
    cache.set(path, source);
    return source;
  }
}

export type CoverState = "loading" | "ready" | "failed";

/** The URL, plus whether a cover that failed should say so.
 *
 *  A failure and a slow load used to render the same "…", so a whole grid of
 *  refused covers looked like it was still working. Those are different
 *  problems and the difference is worth showing.
 */
export function useCoverUrl(
  path: string | null | undefined,
): { url: string | null; state: CoverState } {
  const [state, setState] = useState<CoverState>(() =>
    path ? (cache.get(path) ? "ready" : "loading") : "ready",
  );
  const [url, setUrl] = useState<string | null>(() =>
    path ? (cache.get(path) ?? null) : null,
  );

  useEffect(() => {
    if (!path) {
      setUrl(null);
      setState("ready");
      return;
    }
    let alive = true;
    const cached = cache.get(path);
    if (cached) {
      setUrl(cached);
      setState("ready");
      return;
    }
    setState("loading");
    void load(path)
      .then((u) => {
        if (!alive) return;
        setUrl(u);
        setState("ready");
      })
      .catch(() => {
        // The track still plays without artwork, so this is not worth an error
        // banner -- but it is worth not pretending the cover is still loading.
        if (alive) {
          setUrl(null);
          setState("failed");
        }
      });
    return () => {
      alive = false;
    };
  }, [path]);

  return { url, state };
}

/** Cover image, or a neutral placeholder when there is none to show. */
export function CoverArt({
  path,
  className = "size-20 rounded-lg object-cover ring-1 ring-ink-800",
  alt = "",
}: {
  path: string | null | undefined;
  className?: string;
  alt?: string;
}) {
  const { url, state } = useCoverUrl(path);

  if (!path) return null;

  if (!url) {
    return (
      <div
        className={`${className} flex items-center justify-center bg-gradient-to-br from-ink-800 to-ink-900 text-[10px] text-ink-600`}
        aria-label={state === "loading" ? "cover loading" : "cover unreadable"}
        title={
          state === "failed"
            ? "This cover could not be read. Check that the app has permission to the folder."
            : undefined
        }
      >
        {state === "failed" ? "!" : "…"}
      </div>
    );
  }

  return <img src={url} alt={alt} className={className} loading="lazy" />;
}