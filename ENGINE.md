# Engines

AceForge drives one runtime at a time. Engines live in
`~/.local/share/app.aceforge.desktop/engines/<id>/`, each with a `manifest.json`.

The bundled ACE-Step runtime is adopted into the registry on first launch, so
there is always one working engine.

## Why the two runtimes are not the same shape

| | acestep | audiocpp |
|---|---|---|
| binary | `ace-server` | `audiocpp_server` |
| API | async job queue | one blocking request |
| progress | pollable | none |
| cancel | yes | **no** |
| families | ACE-Step 1.5 (turbo/sft/base/XL) | YuE2, HeartMuLa, Stable Audio 3, MiDashengLM-Gen |
| licence | MIT | Apache-2.0 |

audio.cpp's `POST /v1/tasks/run` blocks for the whole render and cannot be
interrupted: upstream states a wedged GPU call cannot be cancelled from
userspace. Music generation is documented as taking minutes, so its
`busy_timeout_ms` needs raising well past the 5 minute default.

The UI reflects this rather than hiding it — a blocking engine gets no progress
bar and no cancel button.

## Installing an engine from source

Build the runtime yourself, then place the files in a directory:

```
engines/<id>/
  manifest.json
  audiocpp_server          (or ace-server)
  ...shared libraries...
```

`manifest.json`:

```json
{
  "id": "audiocpp",
  "label": "audio.cpp",
  "binary": "audiocpp_server",
  "files": ["libggml-cuda.so", "model_specs/heartmula.json"]
}
```

`files` lists every other file needed at runtime — shared libraries, and
`model_specs/` if the engine should read specs from disk. The binary must be
executable.

The installer refuses the bundle and lists the reason if the id is unknown, a
declared file is missing, or the binary lost its executable bit. Installation
stages to a temporary directory and swaps atomically, so a failure leaves the
previous engine in place.

## Building audio.cpp

```bash
git clone https://github.com/0xShug0/audio.cpp
cd audio.cpp
cmake -B build -DENGINE_ENABLE_CUDA=ON -DCMAKE_CUDA_ARCHITECTURES=61 \
  -DCUDAToolkit_ROOT=/usr/local/cuda-12.6
cmake --build build --target audiocpp_server -j4
```

`CMAKE_CUDA_ARCHITECTURES=61` is required for Pascal (GTX 1050). CUDA 13 dropped
`sm_61`; use 12.6.

Do not link eSpeak statically unless you need the TTS families — it is
GPL-3.0-or-later and changes the licensing of the whole bundle.

## Models

Models are not interchangeable between engines. acestep.cpp reads four separate
GGUF files (LM / text encoder / DiT / VAE); audio.cpp reads one fused GGUF per
family, plus sidecar config files. The same weights cannot serve both.
