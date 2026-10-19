#!/usr/bin/env bash
# Build the bundled MP3 encoder.
#
# Why this exists rather than a Rust crate: every pure-Rust MP3 encoder is
# LGPL, and Rust links statically, so pulling one in would put LGPL code inside
# an MIT binary and oblige us to make the app relinkable. Shipping `lame` as its
# own executable is mere aggregation instead -- the user can replace or delete
# it, and this app stays MIT.
#
# The result is fully static on purpose. A first attempt linked libtinfo (the
# terminal library, pulled in for lame's progress bar) and would have failed on
# any machine without ncurses, which is the exact problem this script exists to
# solve. `ldd` on the output must print "not a dynamic executable".
#
# LGPL notice: the binary in resources/encoder/ is LAME 3.100, and its licence
# travels with it as COPYING.LAME.txt.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
OUT="$HERE/../src-tauri/resources/encoder"
VERSION=3.100
WORK="${TMPDIR:-/tmp}/aceforge-lame-$$"
TARBALL="https://downloads.sourceforge.net/project/lame/lame/${VERSION}/lame-${VERSION}.tar.gz"

cleanup() { rm -rf "$WORK"; }
trap cleanup EXIT

mkdir -p "$WORK" "$OUT"
cd "$WORK"

if [ ! -f lame.tar.gz ]; then
  echo "==> fetching LAME ${VERSION}"
  curl -fsSL -o lame.tar.gz "$TARBALL"
fi
tar xzf lame.tar.gz
cd "lame-${VERSION}"

# --disable-shared --enable-static plus the libtool override below: the
# configure-level LDFLAGS alone leaves a dynamically linked binary behind,
# because libtool adds its own flags to the frontend link line.
echo "==> configuring"
CFLAGS="-O2 -static" LDFLAGS="-static" ./configure \
  --disable-shared \
  --enable-static \
  --enable-frontend \
  --disable-gtktest \
  --disable-analyzer-hooks \
  >configure.log 2>&1

echo "==> building"
make -j"$(nproc)" >make.log 2>&1
make -C frontend LDFLAGS="-static -all-static" >frontend.log 2>&1

BIN="$(find "$WORK" -name lame -type f -perm -u+x | head -1)"
if [ -z "$BIN" ]; then
  echo "!! no lame binary produced; see $WORK/make.log" >&2
  exit 1
fi

if ldd "$BIN" 2>&1 | grep -qv "not a dynamic executable"; then
  echo "!! the binary is not fully static:" >&2
  ldd "$BIN" >&2
  exit 1
fi

install -m 0755 "$BIN" "$OUT/lame"
install -m 0644 "COPYING" "$OUT/COPYING.LAME.txt"

echo "==> $OUT/lame ($(du -h "$OUT/lame" | cut -f1), static)"
"$OUT/lame" --version 2>&1 | head -1