#!/usr/bin/env bash
# Restore the vendored engine binaries that .gitignore excludes.
#
# They are deliberately not in git: libggml-cuda.so is 113 MB and the two
# ace-server binaries are 6 MB each, past GitHub's hard 100 MB per-file limit,
# and all of it is build output rather than source. `npm run tauri build` needs
# them present, so a fresh clone cannot bundle until this has run.
#
# Both come from local source trees rather than a download, so adjust the paths
# if yours differ.

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
resources="$here/src-tauri/resources"

acestep_src="${ACESTEP_SRC:-/media/ghost/Pride/acestep.cpp}"
acestep_build="${ACESTEP_BUILD:-$acestep_src/build}"
# The HOT-Step binary and libraries, and -- separately -- its plugin tree.
#
# Two paths, not one, because they come from different places and mixing them up
# is silent. The installed runtime under /opt carries the *engine-built-in*
# plugins, which still include euler; the repository's top-level plugins/ is the
# newer community set that replaced it, and euler is gone from that one. Ship the
# wrong pair and the engine either segfaults mid-render (no euler, and nothing
# falls back) or quietly runs a solver set nobody has tested.
hotstep_src="${HOTSTEP_SRC:-/opt/hot-step-cpp/engine}"
hotstep_plugins="${HOTSTEP_PLUGINS:-/home/ghost/Documents/Default Project/HOT-Step-CPP/plugins}"

die() { echo "error: $*" >&2; exit 1; }

# The ACE-Step engine is copied from a local build tree, so a clone gets whatever
# that tree was built from. Synced lyrics need the LRC alignment pass, which is
# not in upstream acestep.cpp -- it was ported in as a patch. Apply it before
# building, or the engine will accept get_lrc and quietly return no timings.
if [ -d "$acestep_src" ] && [ -d "$acestep_src/.git" ]; then
  if ! grep -rq "get_lrc" "$acestep_src/src/request.h" 2>/dev/null; then
    echo "note: the ACE-Step source has no LRC support; synced lyrics will not work."
    echo "      apply with: git -C \"$acestep_src\" am $here/scripts/acestep-lrc-alignment.patch"
  fi
fi

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

fetch_hotstep() {
  local src="$hotstep_src"
  [ -d "$src" ] || die "HOT-Step runtime not found at $src
  install it first, or set HOTSTEP_SRC"

  local out="$resources/hotstep-runtime"
  mkdir -p "$out"
  cp -v "$src/ace-server" "$out/"

  # Both engines ship a file called ace-server; these are the libraries that go
  # with this one. Copying the set wholesale is deliberate -- the CPU backend is
  # a set of per-microarchitecture variants and a hand-listed subset is how one
  # goes missing.
  for f in "$src"/libggml*.so*; do
    # The Vulkan backend is left behind: this is a CUDA machine, and shipping it
    # adds bulk for a code path that cannot run here.
    case "$f" in *vulkan*) continue ;; esac
    [ -e "$f" ] && cp -v "$f" "$out/"
  done

  # The plugin tree, with its subdirectories intact. The engine scans four
  # hardcoded directories and is not recursive, so a flattened copy loads no
  # solvers at all -- and its default solver has no plugin, which is a hard
  # crash rather than a fallback.
  if [ -d "$hotstep_plugins" ]; then
    rm -rf "$out/plugins"
    cp -rv "$hotstep_plugins" "$out/"
  else
    die "HOT-Step plugin tree not found at $hotstep_plugins
  the engine loads no solvers without it and cannot render"
  fi
}

fetch_runtime
fetch_hotstep

echo
echo "resources restored under $resources"
echo "now run: npm install && npx tauri build --bundles deb"