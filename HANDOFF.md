# AceForge — session handoff

Written for a fresh agent picking this up cold. Read this first, then
`git log` on `main`.

- **Repo:** `/media/ghost/Pride/AceForge`
- **Branch:** `main`, remote `https://github.com/DjagbleyEmmanuel/AceForge.git`
- **Version:** `0.6.6` installed and built, HEAD `091a9e8`
- **Tests:** `207 passed` — `cargo test --offline --manifest-path src-tauri/Cargo.toml`
- **Types:** `npx tsc --noEmit` clean · **Labels:** `python3 scripts/check-duplicate-labels.py` → 0

## Hardware and hard constraints

| | |
|---|---|
| GPU | GTX 1050, **4096 MiB VRAM** |
| RAM | 24 GB |
| Disk | models on `/media/ghost/Pride` (99% full — 5.9 GB free) |

- **Never run long renders.** The user asked for this at the start and it still
  stands. Short engine probes are fine, and a 10 s probe is how the crash below
  was finally pinned.
- The build cache is at `/var/tmp/aceforge-target/target` via a symlink from
  `src-tauri/target`. Preserve it.
- **Prune the bundle directory every release.** Each `tauri build` leaves a
  ~380 MB staging dir plus an ~84 MB `.deb`. Left alone this fills `/` — it hit
  100% and failed a build with `No space left on device` mid-session.
  ```bash
  cd /var/tmp/aceforge-target/target/release/bundle/deb
  ls -1d AceForge_*_amd64 AceForge_*.deb | grep -v "$(cat /tmp/cur)" | xargs rm -rf
  ```
  Deleting frees the space eventually, not immediately — `df` lags by a few
  seconds. Check again before concluding the prune failed.
- Any change to memory pressure needs re-testing. The 4 GB card is the binding
  constraint on nearly everything.

## Layout that matters

```
/media/ghost/Pride/AceForge                 the app
/home/ghost/Documents/Default Project/HOT-Step-CPP   the newer engine (reference)
```

> The old handoff gave `/media/ghost/Documents/...` for HOT-Step-CPP. **That path
> does not exist.** It is under `/home/ghost/Documents/`. Also note the working
> directory is `/home/ghost/Documents/Default Project`, which is otherwise full
> of unrelated projects — don't assume a repo named for the task is the task.

- Two engines, both ship a binary called **`ace-server`**, linking different ggml.
  They live in separate directories (`~/.local/share/app.aceforge.desktop/engines/{acestep,hotstep}/`)
  and `start_server` takes the engine id explicitly.
- Model weights: `/media/ghost/Pride/AceStep Music Models` (YuE2 in `yue2/`,
  MiniMax in `mm3/`).

## The big one: HOT-Step could not render, and it was four faults deep

It now works — ACE-Step and HOT-Step both verified end to end. The chain is
recorded because each layer looked correct and was not.

1. **The plugins were never installed.** Tauri's `hotstep-runtime/plugins/*`
   glob installs files in `plugins/` and creates the directories it matches, but
   not what is inside them. A `**` glob is worse: Tauri v2 *flattens* it into one
   target. Neither works, because `lua-plugin-registry.h` scans four hardcoded
   subdirectories (`solvers`, `schedulers`, `guidance`, `postprocess`) and is not
   recursive. Each subdirectory is mapped explicitly in `tauri.conf.json`.
2. **The refresh could never run.** `adopt_bundled_engine` and
   `adopt_bundled_hotstep` returned early when an engine already existed, before
   reaching `adopt_from` — which is where `should_refresh` and the runtime
   fingerprint live. Both short-circuits are gone.
3. **The fingerprint never round-tripped.** The manifest was written with
   serde_json's `sourceHash` and read into a field named `source_hash` with no
   serde rename, so every read found `None` and the refresh returned `false`
   forever, silently, for every engine, on every platform.
4. **The engine reads a different key, on a different request.** This is the one
   that cost the most. `acestep.cpp` reads `solver`; HOT-Step reads
   **`infer_method`**. Each ignores the key it does not know. And
   `parse_server_fields` runs over the **`/synth` body only** — `/lm` never reads
   it and does not echo it back. So a correct solver sent with the planner is
   discarded and the render runs on the engine's default `euler`, which has no
   plugin. `dit_ggml_generate` `#include`s `hot-step-sampler.h`, whose solver
   dispatch dereferences the null it gets back. Process dies, mid-render.
   `guidance_mode` is the same trap one line later: the default is `apg`, also
   unregistered.
5. …and the stamping that fixed all that sat inside a branch
   (`if codes.is_empty() && adapter.is_empty() && …`) that a plain text-to-music
   render never entered.

Verified working, from the installed engine, through the app's two-stage flow:

```
[DiT] Solver: MD Confluence V4 (md_confluence_v4, 1 NFE/step, order 1)
[DiT] Guidance: MD STORM Guidance V2 (md_storm_guidance) [post_step]
[Server] Job done (1 tracks)
```

### The lesson, which is worth more than the fix

Five releases in a row changed something defensible and inert. Every one was
verified with hand-built `curl` requests — **which bypass the app entirely**, so
no hand-crafted test could ever exercise the branch that was broken.

Two things would have caught it on the first attempt:

- **Read the engine's own log.** It prints one line that settles everything:
  ```
  [Server] HOT-Step params: solver=euler, guidance=apg, scheduler=(default)
  ```
  Both are engine *defaults*. That line was in the user's first paste and it was
  in every paste after.
- **Test the path that runs, not the payload.** A payload proven correct by hand
  says nothing about whether the app sends it.

Also: `stderr` redirected to a file **loses its last lines when the process
dies**. The log on disk stops short of the crash and reads as though it died
earlier than it did. Under gdb on a terminal the missing lines are there. Hours
went into reading a truncated log.

`dmesg` is worth reaching for early: every crash showed `segfault at c4` with the
same faulting instruction, which says "one deterministic null deref" and rules
out a whole class of theories at once.

## YuE2

- **UI freeze during generation — fixed.** `yue2_props` was a *synchronous*
  command doing a 30 s HTTP call to an engine whose worker is saturated for the
  whole render, so the wait sat on the main thread. Polled every 8 s against a
  30 s wait, the window was blocked essentially continuously. Now async with the
  blocking call on a worker, and the poll pauses during a render.
- **False "stalled" warning — fixed.** The watchdog assumes the sidecar logs
  continuously; true of ACE-Step, false of YuE2. A measured render spent 934 s in
  NAR alone. `wait_for_job_quiet` drops the silence check for YuE2 only.
- **Lost-contact false alarm — fixed.** The poll inferred death from a *timeout*,
  which for a busy engine means nothing. It now asks the process. Budgets raised
  as a backstop only; 80 failures at a 10 s read timeout is ~14 minutes and a
  YuE2 render takes ~19.
- **Progress bar — done.** Reads YuE2's own markers (it shares none of ACE-Step's).
  Shares are from a measured render. NAR logs a chunk index with no total, so the
  count goes in the stage label and no denominator is invented.
- **Lyrics — partially done, and the limit is external.** A `.lrc` is written
  beside the audio, spread across the track, **but only for lyrics the user
  supplied**. For a composer-generated song the words never leave the engine:
  `yue2-job.h` never sets `result_lrc` (ACE-Step's `synth_worker` does, and
  returns it in `X-LRC-Text`), and the `abc` score the job does return is built
  from MIDI voices with no lyric field. Fixing that needs an engine change.
  The spread timings are a fabrication and the code says so.

## UI work completed

All seven originally-requested items are done: clear-all for logs, category
filtering, Logs moved below the Queue with each panel independently collapsible,
terminal restyle, clear-failed, backlogs removed from under the Library.

Two fixes worth remembering:

- **"Clear done" was disabled unless a *successful* render was on the list**, so a
  queue of nothing but failures had no way to be emptied at all.
- The log's `follow` mode snapped the box to the bottom, so picking a category
  showed the *last* few lines and left the rest above the scroll. A chosen
  category now reads from its first line.

## Known limits and outstanding

- **No automated test for the frontend.** `cargo test` and `tsc` only; there is
  no JS runner. Frontend correctness is tsc + reading.
- The Tauri webview cannot be driven headlessly. Anything UI-shaped is verified
  by reading the code and against the real filesystem, never by watching it.
- **YuE2 audio needs ~4.6 GB and OOMs on this card** (composer ~2.1 GB + KV cache
  ~2.4 GB). The panel says so up front, and planning works fine on its own.
  `scragnog/YuE2-GGUF` has `yue2-lm-Q4_K_M.gguf` at 2.26 GiB, the lever if it's
  ever worth pursuing.
- MiniMax weights download and classify, but there is **no generate UI**.
- **audio.cpp is gone.** The orphaned 378 MB install is deleted, `ENGINE.md` and
  `fetch-resources.sh` no longer mention it, and nothing in the running app ever
  addressed it. Do not re-add it. What remains from that work is the YuE2 and
  MiniMax coverage, which HOT-Step provides instead.

## Conventions

- Rust: `cargo test --offline` only — the machine builds offline against a
  fixed dependency set. Do not add crates casually; `cc` was added deliberately
  for minimp3.
- Build: `npm run tauri build -- --bundles deb`, install with
  `sudo -n dpkg -i .../AceForge_<ver>_amd64.deb`. Stop the app first
  (`pkill -x AceForge`) or dpkg hits a held binary.
- Bump version in `src-tauri/Cargo.toml`, `package.json` and
  `src-tauri/tauri.conf.json` together. Installing over the same version number
  muddies exactly the staleness question you need to be able to answer.
- Commit and push when a unit of work is verified.
- Report what was *not* verified. Several times here the honest answer has been
  "confirmed by reading the code and by a targeted probe", not "seen working in
  the UI".