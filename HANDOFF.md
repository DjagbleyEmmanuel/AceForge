# AceForge — session handoff

Written for a fresh agent picking this up cold. Read this first, then the
`git log` on `main`.

- **Repo:** `/media/ghost/Pride/AceForge`
- **Branch:** `main`, remote `https://github.com/DjagbleyEmmanuel/AceForge.git`
- **Version at time of writing:** `0.5.5` installed, last commit `54d7eaf`
- **Tests:** `169 passed` — `cargo test --offline --manifest-path src-tauri/Cargo.toml`
- **Types:** `npx tsc --noEmit` clean · **Labels:** `python3 scripts/check-duplicate-labels.py` → 0

## Hardware and hard constraints

| | |
|---|---|
| GPU | GTX 1050, **4096 MiB VRAM** |
| RAM | 24 GB |
| Disk | models on `/media/ghost/Pride` |

- **Never run long audio.cpp / YuE2 renders.** The user asked for this at the
  start and it still stands. Short engine probes are fine.
- The disk holding the models has been full before; the build cache lives at
  `/var/tmp/aceforge-target/target` via symlink. Preserve that.
- Any change to memory pressure needs re-testing. The 4 GB card is the binding
  constraint on nearly everything.

## Layout that matters

```
/media/ghost/Pride/AceForge            the app
/media/ghost/Pride/acestep.cpp         bundled engine, branch feat/lrc-alignment
/media/ghost/Documents/Default Project/HOT-Step-CPP   the newer engine (read-only reference)
```

- Two engines, both ship a binary called **`ace-server`**, and they link
  different ggml (0.25.x vs 0.26.x). They are kept in separate directories
  (`~/.local/share/app.aceforge.desktop/engines/{acestep,hotstep}/`) and
  `start_server` takes the engine id explicitly. Do not let one load the other's
  libraries.
- Model weights: `/media/ghost/Pride/AceStep Music Models`
  (YuE2 in its `yue2/` subdir, MiniMax in `mm3/`). HOT-Step's app is configured
  to the same root.

## Things already fixed this session

Each of these had a root cause that was *not* the obvious one. Don't regress them.

1. **Audio clipping.** `audio_normalize` in acestep.cpp peak-normalised to 1.0
   then hard-clamped. Renders hit 0 dBFS with a ~14.5 dB crest, which buries
   vocals. Now -1 dBFS with a rational soft clip. Engine commit `0d4a426`.
2. **Crop / normalise were dead on MP3**, which is the *default* output format.
   Vendored minimp3 (CC0) into `src-tauri/src/vendor/`, wired via `cc` in
   `build.rs`. Three FFI traps: `mp3dec_t` is 6668 bytes; the frame-info struct
   has six fields and calls the rate `hz`; `mp3dec_decode_frame` returns samples
   *per channel*. All pinned by const assertions and tests.
3. **Log viewer stole scroll.** `scrollIntoView` walks every scrollable
   ancestor, so following the tail dragged the page. Now scrolls its own box.
4. **Imported sources were deleted on pick.** `list_tracks` is non-recursive by
   design and imports live in `library/imports`, so the "drop a missing source"
   effect dropped every import. Fixed by consulting both lists.
5. **Cover art was invisible.** `.panel` has `backdrop-filter`, which creates a
   stacking context, so `grant_covers` — which only widened the *asset protocol*
   scope — never touched the fs-plugin scope the frontend actually reads
   through. Both are granted now, at save time as well as startup. Also 320px
   thumbnailing (that folder is 495 MB of PNGs) and `!` instead of an endless
   `…` on failure.
6. **Retry froze the window.** `retry()` called `next_id()` inside
   `Inner::update`, and `next_id()` locks the same non-reentrant Mutex.
   Thread parked forever holding the lock.
7. **base64 padding.** `decode_base64` rejected `=` as a bad character, so any
   LRC not a multiple of three bytes failed — and it runs *before* the audio is
   read, so a successful render got thrown away. ~2 jobs in 3.
8. **Select dropdowns painted under later panels.** Portalled to `document.body`.
9. **queue tests wrote into the user's real queue** — `state()` is a
   process-wide `OnceLock` over `queue.json`. Now isolated via
   `ACEFORGE_DATA_DIR`, with a test asserting on the file actually written.
10. **A reset status poll killed live renders.** `wait_for_job` treated a poll
    as a point of failure; the engine's keep-alive timeout expires between
    400 ms polls and the next request hits a closed socket. Now tolerated, with
    the stall watchdog deciding death.

## Two things I got wrong, and what they cost

- I reported "progress tests pass" while filtering output that hid the fact that
  **the tests had been lost in an earlier edit and never ran.** When restored
  they immediately caught a real bug: `parse_dit_step` read the step total from
  the token *after* `39/50`, which is `t=0.240`, so **no** progress line ever
  parsed and the bar would have sat on its indeterminate sweep forever.
  Lesson: when adding tests, confirm they actually ran (`test result: ok. 169`,
  not a filtered grep).
- I diagnosed HOT-Step's crash as a missing `PluginRegistry::init` in
  `tools/ace-server.cpp` and patched that file. **Wrong file.** CMake builds the
  binary named `ace-server` from `tools/hot-step-server.cpp`; the upstream
  `ace-server.cpp` is dead reference code and is never compiled. Reverted. The
  real cause was the missing plugins directory (see below) — but a *second*
  fault remains, below.

## OPEN: HOT-Step cannot render. Two faults.

### Fault 1 — FIXED but not yet verified end to end

`[Plugins] Loaded 0 solvers` because Tauri's resource glob `hotstep-runtime/*`
matched files but not directories, so the Lua plugins were never installed.
Fixed by also bundling `resources/hotstep-runtime/plugins/*`. Verified by hand:
0 solvers → 13.

The solver is now **opt-in** (`GenerateParams::solver` defaults to empty).
Sending a name the running engine lacks is fatal on HOT-Step: it logs
`unknown solver 'euler'`, the fallback lookup misses too, and it dereferences
the null result. The bundled engine wants `euler`; HOT-Step has
`md_confluence_v4`, `md_storm_V4`, `storm`, `md_vortex_v1`, … and no `euler`.

### Fault 2 — OPEN. HOT-Step segfaults on this GPU regardless.

Reproduced directly, outside the app:

```bash
cp -a /usr/lib/AceForge/hotstep-runtime /tmp/hstest   # + plugins/ from resources
cd /tmp/hstest && ./ace-server --models "/media/ghost/Pride/AceStep Music Models" \
    --host 127.0.0.1 --port 8831 --vae-chunk 256 --max-seq 4096
# POST /lm  -> done
# POST /synth -> Segmentation fault (core dumped)
```

It dies **immediately after** `[DiT] Graph exec: direct (static, no scheduler)`
and never reaches `[DiT] CFG enabled`. The bundled acestep.cpp passes that same
line with the same DiT weights. Reproduced three ways: no `solver` field,
`solver: "euler"`, and `solver: "storm"` (a name that *is* registered). So it
is not the solver lookup and not something this app sends.

Candidate causes, in the order I would test them:
1. **CUDA arch.** HOT-Step ships `libggml-cuda.so` built for a newer arch; the
   card is sm_61. A kernel fault would surface exactly like this. Check
   `cuobjdump --list-elf` on the shipped `.so`, and try `GGML_CUDA_FORCE_MMQ=1`
   or a CPU-backend run (`CUDA_VISIBLE_DEVICES=`) to see whether it survives
   without CUDA.
2. A genuine upstream null-deref in `hot-step-sampler.h` on a code path the
   bundled engine does not take. `hot-step-sampler.h:709` is the solver lookup;
   `hot-step-sampler-trt.h:196` is the TensorRT twin.
3. Rebuilding the engine is currently blocked: `ggml-cuda/fattn-train.cu` fails
   with `__float22bfloat162_rn is undefined` under the local CUDA 12.0. The
   packaged binary was evidently built with different flags. The target is
   `add_executable(ace-server tools/hot-step-server.cpp ...)` at `CMakeLists.txt:484`.
   Note `tools/public/index.html.gz` → `index.html.gz.hpp` is a generated header
   needed to compile; regenerate with
   `cmake -DINPUT=$PWD/tools/public/index.html.gz -DOUTPUT=$PWD/build/index.html.gz.hpp -P tools/xxd.cmake`.

**Until this is resolved, HOT-Step is unusable for generation.** The bundled
`acestep` engine works and is the default.

## OPEN: requested but not started

The user asked for this last and it has **not** been touched:

1. Clear-all button for the model logs.
2. Log filtering by category — runtime generation logs, errors, etc. — so a
   user can see and select which kind they want.
3. Move the Logs UI **below** the Queue, each with its own collapse/expand.
4. Restyle the Logs UI to look like a terminal.
5. A button to clear **failed** generations from the queue (there is none today;
   `clear_done` only removes finished rows and the user has been unable to clear
   the junk).
6. Remove the backlogs section from under the library.
7. All of it must not lag the app. Follow the ProgressPanel pattern: one 1 Hz
   timer, CSS-driven animation on `transform`, no `requestAnimationFrame` loop,
   state changes confined to the component that shows them.

Relevant files: `src/components/LogViewer.tsx`, `src/components/QueuePanel.tsx`,
`src/App.tsx` (sidebar is the `<aside>`, `ThemeChooser`/`Runtime` card at ~line 60),
`src-tauri/src/server.rs` (`stderr_since`, `stderr_tail`, `stderr_mark`),
`src-tauri/src/queue.rs` (`clear_done`, `discard`).

## Also outstanding

- **26 junk rows** ("retry me" / "boom") from my test pollution are still in
  `~/.local/share/app.aceforge.desktop/queue.json`. Needs AceForge **closed**,
  then `rm` the file. The test no longer pollutes; verified by diffing the real
  file across a run (byte-identical).
- Orphaned `~/.local/share/app.aceforge.desktop/audiocpp/` (150 MB, the removed
  audio.cpp runtime) and `audiocpp/server.json`. Nothing references them.
- YuE2 works for **planning** on this card (a real 2358-byte ABC lead sheet) but
  the audio stage needs ~4.6 GB and OOMs: composer ~2.1 GB + KV cache ~2.4 GB.
  The panel says so up front. `scragnog/YuE2-GGUF` has `yue2-lm-Q4_K_M.gguf` at
  2.26 GiB, which is the lever if that is ever worth pursuing.
- MiniMax weights download and classify, but there is **no generate UI** for it.

## Conventions

- Rust: `cargo test --offline` only — the machine builds offline against a
  fixed dependency set. Do not add crates casually; `cc` was added deliberately
  for minimp3.
- Build: `npm run tauri build -- --bundles deb`, install with
  `sudo dpkg -i .../AceForge_<ver>_amd64.deb`.
- Bump version in `src-tauri/Cargo.toml`, `package.json` and
  `src-tauri/tauri.conf.json` together.
- Commit and push when a unit of work is verified.
- Report what was *not* verified. Several times here the honest answer has been
  "confirmed by reading the code and by a targeted probe", not "seen working in
  the UI" — the Tauri webview cannot be driven headlessly.