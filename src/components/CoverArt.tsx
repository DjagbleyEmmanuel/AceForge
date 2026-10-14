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

async function load(path: string): Promise<string> {
  const hit = cache.get(path);
  if (hit) return hit;
  const bytes = await readFile(path);
  const url = URL.createObjectURL(
    new Blob([bytes], { type: mimeFor(path) }),
  );
  cache.set(path, url);
  return url;
}

/** One cover, or null while it loads or if it cannot be read. */
export function useCoverUrl(path: string | null | undefined): string | null {
  const [url, setUrl] = useState<string | null>(() =>
    path ? (cache.get(path) ?? null) : null,
  );

  useEffect(() => {
    if (!path) {
      setUrl(null);
      return;
    }
    let alive = true;
    const cached = cache.get(path);
    if (cached) {
      setUrl(cached);
      return;
    }
    void load(path)
      .then((u) => {
        if (alive) setUrl(u);
      })
      .catch(() => {
        // A cover that cannot be read is not worth an error banner; the track
        // still plays. Rendering nothing is honest about it.
        if (alive) setUrl(null);
      });
    return () => {
      alive = false;
    };
  }, [path]);

  return url;
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
  const url = useCoverUrl(path);

  if (!path) return null;

  if (!url) {
    return (
      <div
        className={`${className} flex items-center justify-center bg-gradient-to-br from-ink-800 to-ink-900 text-[10px] text-ink-600`}
        aria-label="cover loading"
      >
        …
      </div>
    );
  }

  return <img src={url} alt={alt} className={className} loading="lazy" />;
}