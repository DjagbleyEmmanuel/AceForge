#!/usr/bin/env bash
# Restore the vendored engine binaries that .gitignore excludes.
#
# They are deliberately not in git: audiocpp_server is 144 MB and
# libggml-cuda.so.0 is 111 MB, both past GitHub's hard 100 MB per-file limit,
# and both are build output rather than source. `npm run tauri build` needs them
# present, so a fresh clone cannot bundle until this has run.
#
# Both come from local source trees rather than a download, so adjust the paths
# if yours differ.

set -euo pipefail

# The ACE-Step engine is copied from a local build tree, so a clone gets whatever
# that tree was built from. Synced lyrics need the LRC alignment pass, which is
# not in upstream acestep.cpp -- it was ported in as a patch. Apply it before
# building, or the engine will accept get_lrc and quietly return no timings.
if [ -d "$acestep_src" ] && [ -d "$acestep_src/.git" ]; then
  if git -C "$acestep_src" log --oneline -1 | grep -q . ; then
    if ! grep -rq "get_lrc" "$acestep_src/src/request.h" 2>/dev/null; then
      echo "note: the ACE-Step source has no LRC support; synced lyrics will not work."
      echo "      apply with: git -C \"$acestep_src\" am $here/scripts/acestep-lrc-alignment.patch"
    fi
  fi
fi

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
resources="$here/src-tauri/resources"

acestep_src="${ACESTEP_SRC:-/media/ghost/Pride/acestep.cpp}"
acestep_build="${ACESTEP_BUILD:-$acestep_src/build}"
audiocpp_build="${AUDIOCPP_BUILD:-/media/ghost/Pride/audio.cpp/build}"

die() { echo "error: $*" >&2; exit 1; }

fetch_runtime() {
  local src="$acestep_build"
  [ -d "$src" ] || die "acestep.cpp build not found at $src
  build it first, or set ACESTEP_SRC"

  mkdir -p "$resources/runtime"
  cp -v "$src/ace-server" "$resources/runtime/"
  # The server dlopens these; the bundle ships them beside it.
  for lib in libggml.so.0 libggml-base.so.0 libggml-cpu.so.0 libggml-cuda.so.0; do
    [ -e "$src/$lib" ] && cp -v "$src/$lib" "$resources/runtime/"
  done
}

fetch_audiocpp() {
  [ -d "$audiocpp_build" ] || die "audio.cpp build not found at $audiocpp_build
  build it first, or set AUDIOCPP_BUILD"

  mkdir -p "$resources/audiocpp"
  cp -v "$audiocpp_build/bin/audiocpp_server" "$resources/audiocpp/"

  # espeak-ng-data.bin is not produced by the CMake build; take it from the
  # installed engine directory rather than trying to rebuild it.
  local espeak
  espeak="$(find "$HOME/.local/share" -name espeak-ng-data.bin 2>/dev/null | head -1 || true)"
  [ -n "$espeak" ] && cp -v "$espeak" "$resources/audiocpp/"

  cat > "$resources/audiocpp/manifest.json" <<'JSON'
{
  "id": "audiocpp",
  "label": "audio.cpp",
  "binary": "audiocpp_server",
  "files": ["espeak-ng-data.bin"]
}
JSON
}

fetch_runtime
fetch_audiocpp

echo
echo "resources restored under $resources"
echo "now run: npm install && npx tauri build --bundles deb"