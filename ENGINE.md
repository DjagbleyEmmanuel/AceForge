# Engines

AceForge drives one runtime at a time. Engines live in
`~/.local/share/app.aceforge.desktop/engines/<id>/`, each with a `manifest.json`.

The bundled ACE-Step runtime is adopted into the registry on first launch, so
there is always one working engine.

## The two engines

| | acestep | hotstep |
|---|---|---|
| binary | `ace-server` | `ace-server` |
| libraries | `libggml-{base,cpu,cuda}.so.0` | `libggml-{base,cpu,cuda}.so.0` + CPU variant plugins |
| solvers | built in | **Lua plugins** |
| pipelines | ACE-Step 1.5 (turbo/sft/base/XL) | ACE-Step 1.5, YuE2, MiniMax-Music3 |
| licence | MIT | see the upstream repository |

Both ship a file called `ace-server`, and their ggml sonames are unversioned
(`libggml.so.0`) on both sides — so nothing about the filename distinguishes
them, and linking one engine's libraries against the other's binaries loads
cleanly and fails later. Hence the separate directories, and the explicit engine
id on every start: `start_server` takes the engine rather than resolving
whichever is found first.

## HOT-Step needs its plugins, and they must keep their directories

HOT-Step resolves its sampler out of a Lua plugin registry, not from compiled-in
solvers. It scans four hardcoded subdirectories:

```
plugins/solvers/  plugins/schedulers/  plugins/guidance/  plugins/postprocess/
```

The scan is **not recursive**, and the engine ships no solver named `euler` --
its own default. A missing or flattened plugin tree does not degrade, it kills
the render: the sampler logs `unknown solver 'euler', falling back to euler`, the
fallback re-asks for the same missing name, and the next line dereferences the
null it got back.

Two things follow, and both have bitten:

- `tauri.conf.json` maps each plugin subdirectory **explicitly**. A `plugins/*`
  glob installs the files and creates the directories but not their contents; a
  `**` glob is worse, because Tauri flattens it into one target.
- HOT-Step reads its solver from the `infer_method` key of the **`/synth` body**.
  It is not `solver`, and `/lm` neither reads it nor echoes it back.

## Installing an engine from source

Build the runtime yourself, then place the files in a directory:

```
engines/<id>/
  manifest.json
  ace-server
  ...shared libraries...
```

`manifest.json`:

```json
{
  "id": "hotstep",
  "label": "HOT-Step CPP",
  "binary": "ace-server",
  "files": ["libggml-base.so", "libggml-cuda.so"],
  "sourceHash": "9aaf31f784623819"
}
```

- `files` lists every other file needed at runtime — shared libraries, and
  `model_specs/` if the engine reads specs from disk. **Regular files only**:
  listing a directory makes the install report itself broken, because the
  dependency check asks whether each entry is a file.
- `sourceHash` is a fingerprint of the whole runtime directory, used to decide
  whether a bundled update should replace what is installed. Its **key name has
  to match on both sides** — it is written by serde_json as `sourceHash` and read
  into a field that must carry `#[serde(rename = "sourceHash")]`. When that was
  missing the refresh silently never ran, for any engine, forever.
- The binary must be executable.

The installer refuses the bundle and lists the reason if the id is unknown, a
declared file is missing, or the binary lost its executable bit. Installation
stages to a temporary directory and swaps atomically, so a failure leaves the
previous engine in place.

A directory with no manifest is **not** an install and is cleared and re-adopted.
Otherwise a half-finished copy reports "not installed" while holding a full
payload, and nothing repairs it.

## Building

The bundled runtimes are fetched rather than committed: `scripts/fetch-resources.sh`
puts them back after a fresh clone. See that script for where they come from.

`CMAKE_CUDA_ARCHITECTURES=61` is required for Pascal (GTX 1050). CUDA 13 dropped
`sm_61`; use 12.6.

## Models

Models are not interchangeable between engines. ACE-Step reads four separate GGUF
files (LM / text encoder / DiT / VAE); YuE2 reads its own pair under `yue2/` and
MiniMax under `mm3/`. The same weights cannot serve both.