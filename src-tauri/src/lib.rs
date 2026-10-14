pub mod audiocpp;
pub mod downloads;
pub mod engines;
pub mod generate;
pub mod music;
pub mod paths;
pub mod presets;
pub mod server;
pub mod settings;
pub mod user_presets;
pub mod store;

use serde::Serialize;
use std::path::PathBuf;
use tauri::Manager;

#[derive(Serialize)]
struct StoreView {
    repo: String,
    models_dir: String,
    /// True when `models_dir` came from a saved setting or the env override
    /// rather than the built-in default. Drives the "custom" badge in the UI.
    models_dir_is_custom: bool,
    files: Vec<store::WeightFile>,
    presets: Vec<store::Preset>,
    validation: store::Validation,
}

#[derive(Serialize)]
struct SettingsView {
    audiocpp_models_dir: String,
    audiocpp_backend: String,
    audiocpp_device: u32,
    audiocpp_threads: u32,
    vae_chunk: i32,
    max_seq: i32,
    models_dir: String,
    resolved_models_dir: String,
    default_models_dir: String,
    autostart_server: bool,
    env_override: Option<String>,
    library_dir: String,
    resolved_library_dir: String,
}

#[tauri::command]
fn get_settings() -> SettingsView {
    let saved = settings::load();
    let env = std::env::var("ACEFORGE_MODELS_DIR")
        .ok()
        .filter(|v| !v.trim().is_empty());

    SettingsView {
        audiocpp_models_dir: saved.audiocpp_models_dir,
        audiocpp_backend: saved.audiocpp_backend,
        audiocpp_device: saved.audiocpp_device,
        audiocpp_threads: saved.audiocpp_threads,
        vae_chunk: saved.vae_chunk,
        max_seq: saved.max_seq,
        resolved_models_dir: settings::resolve_models_dir(None).display().to_string(),
        default_models_dir: store::default_models_dir().display().to_string(),
        models_dir: saved.models_dir,
        library_dir: saved.library_dir,
        resolved_library_dir: paths::library_dir().display().to_string(),
        autostart_server: saved.autostart_server,
        env_override: env,
    }
}

/// Point the app at a different weights directory.
///
/// Passing an empty string clears the override and restores the default, which
/// is why this takes the raw string rather than a path.
#[tauri::command]
fn set_models_dir(path: String) -> Result<SettingsView, String> {
    let trimmed = path.trim();

    let resolved = if trimmed.is_empty() {
        store::default_models_dir()
    } else {
        settings::normalize(trimmed)?
    };

    // Create it if missing: the user is telling us this is where weights go.
    if !resolved.exists() {
        std::fs::create_dir_all(&resolved).map_err(|e| {
            format!("cannot create {}: {e}", resolved.display())
        })?;
    } else if !resolved.is_dir() {
        return Err(format!("{} exists but is not a directory", resolved.display()));
    }

    let mut current = settings::load();
    // Store the normalized absolute path so later reads need no re-resolution.
    current.models_dir = if trimmed.is_empty() {
        String::new()
    } else {
        resolved.display().to_string()
    };
    settings::save(&current)?;

    Ok(get_settings())
}

/// What the backend actually loaded, per `/props`. This is the ground truth
/// for "will this file work": the registry is built from each GGUF's
/// `general.architecture` metadata at scan time, not from the filename.
fn backend_registry(port: Option<u16>) -> Option<Vec<String>> {
    let port = port.or_else(|| server::running_port())?;
    let props = server::props(port).ok()?;
    let models = props.get("models")?.as_object()?;
    let mut names = Vec::new();
    for bucket in models.values() {
        if let Some(list) = bucket.as_array() {
            for item in list {
                if let Some(s) = item.as_str() {
                    names.push(s.to_string());
                }
            }
        }
    }
    Some(names)
}

/// LoRA adapters the running server has registered.
///
/// `/props` reports these as a top-level `adapters` array, not inside `models`,
/// so the registry walk above cannot see them.
#[tauri::command]
fn list_adapters(port: Option<u16>) -> Vec<String> {
    let Some(port) = port.or_else(|| server::running_port()) else {
        return Vec::new();
    };
    let Ok(props) = server::props(port) else {
        return Vec::new();
    };
    props
        .get("adapters")
        .and_then(|a| a.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

#[tauri::command]
fn store_snapshot() -> Result<StoreView, String> {
    let saved = settings::load();
    let env = std::env::var("ACEFORGE_MODELS_DIR")
        .ok()
        .filter(|v| !v.trim().is_empty());
    let dir = settings::resolve_models_dir(None);

    // Offline-tolerant: a failed Hub call still yields local validation so the
    // app stays usable without network.
    let supported = backend_registry(None);
    let files = store::scan_local_with(&dir, supported.as_deref());
    let presets = store::presets(&files);
    let validation = store::validate(&dir);

    Ok(StoreView {
        repo: store::REPO.to_string(),
        models_dir: dir.display().to_string(),
        models_dir_is_custom: env.is_some() || !saved.models_dir.trim().is_empty(),
        files,
        presets,
        validation,
    })
}

/// Installed and available runtimes, with what each one can actually produce.
#[tauri::command]
fn list_engines() -> Vec<engines::Engine> {
    engines::list()
}

/// Adopt the runtime shipped inside the app bundle as the ace-step engine, so
/// a fresh install has one working engine before the user does anything.
#[tauri::command]
fn adopt_bundled_engine(app: tauri::AppHandle) -> Result<bool, String> {
    use tauri::Manager;
    let Some(resource_dir) = app.path().resource_dir().ok() else {
        return Ok(false);
    };
    let bundled = resource_dir.join("aceforge-runtime");
    if engines::binary_path(engines::EngineId::AceStep).is_some() {
        return Ok(false);
    }
    if !bundled.join("ace-server").is_file() {
        return Ok(false);
    }
    engines::adopt_from(&bundled).map(|_| true)
}

/// Adopt the audio.cpp server shipped inside the app bundle. Unlike the
/// ace-step runtime this one is large and optional, so an engine the user
/// already installed is never overwritten.
#[tauri::command]
fn adopt_bundled_audiocpp(app: tauri::AppHandle) -> Result<bool, String> {
    use tauri::Manager;
    let Some(resource_dir) = app.path().resource_dir().ok() else {
        return Ok(false);
    };
    if engines::binary_path(engines::EngineId::AudioCpp).is_some() {
        return Ok(false);
    }
    let bundled = resource_dir.join("audiocpp");
    if !bundled.join("audiocpp_server").is_file() {
        return Ok(false);
    }
    engines::adopt_from(&bundled).map(|_| true)
}

/// Validate an engine directory the user has extracted somewhere.
#[tauri::command]
fn inspect_engine(path: String) -> engines::Inspection {
    engines::inspect(std::path::Path::new(&path))
}

/// Install a validated engine directory.
#[tauri::command]
fn install_engine(path: String) -> Result<String, String> {
    engines::install(std::path::Path::new(&path))
}

#[tauri::command]
fn remove_engine(id: String) -> Result<(), String> {
    let parsed = engines::EngineId::parse(&id).ok_or_else(|| format!("unknown engine: {id}"))?;
    engines::remove(parsed)
}

/// Where engines are installed, so the user knows where to extract to.
#[tauri::command]
fn engines_dir() -> String {
    engines::install_dir().display().to_string()
}

/// Start audio.cpp against one or more model directories.
/// Start audio.cpp. Must be async: `start` waits on `/health` for up to 90s,
/// and a synchronous command blocks the main thread, which freezes the webview
/// instead of showing a spinner.
#[tauri::command]
async fn audiocpp_start(
    models: Vec<audiocpp::ModelEntry>,
) -> Result<audiocpp::RuntimeStatus, String> {
    tokio::task::spawn_blocking(move || audiocpp::start(&models))
        .await
        .map_err(|e| format!("start task failed: {e}"))?
}

/// What audio.cpp can currently see. Proves a directory was picked up.
#[tauri::command]
fn audiocpp_models() -> Result<Vec<audiocpp::ModelEntry>, String> {
    audiocpp::list_models()
}

#[tauri::command]
fn audiocpp_stop() {
    audiocpp::stop();
}

/// Render on audio.cpp. Blocks for as long as the model takes, which on CPU is
/// measured in minutes for a 3B model.
#[tauri::command]
async fn audiocpp_render(
    app: tauri::AppHandle,
    model: String,
    params: audiocpp::YuE2Params,
) -> Result<audiocpp::Rendered, String> {
    use tauri::Emitter;
    audiocpp::reset_cancel();
    let progress = app.clone();
    tokio::task::spawn_blocking(move || {
        let _ = progress.emit("ac://stage", ("rendering", "this can take minutes on CPU"));
        audiocpp::render(&model, &params)
    })
    .await
    .map_err(|e| format!("render task failed: {e}"))?
}

/// The only way to stop a run: audio.cpp cannot be interrupted mid-render.
#[tauri::command]
fn audiocpp_cancel() {
    audiocpp::cancel();
}

/// Identify a directory as an audio.cpp model, if it looks like one.
#[tauri::command]
fn audiocpp_probe(path: String) -> Vec<audiocpp::ModelEntry> {
    audiocpp::probe_dir(std::path::Path::new(&path))
}

/// Everything a directory offers, without starting the runtime. This is what
/// the picker renders from, so assigning a folder is enough to populate it.
#[tauri::command]
fn audiocpp_scan(path: String) -> audiocpp::ScanResult {
    audiocpp::scan(std::path::Path::new(&path))
}

/// Backend, device and CPU thread count for the audio.cpp runtime.
#[tauri::command]
fn set_audiocpp_runtime(
    backend: String,
    device: u32,
    threads: u32,
) -> Result<SettingsView, String> {
    let allowed = ["cpu", "cuda", "vulkan", "metal", "hip"];
    if !allowed.contains(&backend.as_str()) {
        return Err(format!("unknown backend {backend}"));
    }
    let mut current = settings::load();
    current.audiocpp_backend = backend;
    current.audiocpp_device = device;
    current.audiocpp_threads = threads;
    settings::save(&current)?;
    Ok(get_settings())
}

/// Keys, tempos, languages and sampling presets for the Studio pickers.
#[tauri::command]
fn music_reference() -> music::MusicReference {
    music::reference()
}

/// Merge a sampling preset into the live request. Returning the whole object
/// keeps the frontend from having to know which fields a preset owns.
#[tauri::command]
fn apply_sampling_preset(
    preset_id: String,
    current: generate::GenerateParams,
) -> Result<generate::GenerateParams, String> {
    let preset = music::find_sampling(&preset_id)
        .ok_or_else(|| format!("unknown sampling preset: {preset_id}"))?;
    Ok(music::apply_sampling(&current, &preset.params))
}

#[tauri::command]
fn generation_presets() -> Vec<presets::Preset> {
    presets::all()
}

/// User-saved parameter bundles, newest first.
#[tauri::command]
fn user_presets_list() -> Vec<user_presets::UserPreset> {
    user_presets::list()
}

#[tauri::command]
fn user_preset_save(
    label: String,
    current: generate::GenerateParams,
) -> Result<user_presets::UserPreset, String> {
    user_presets::save_preset(&label, current)
}

#[tauri::command]
fn user_preset_delete(id: String) -> Result<(), String> {
    user_presets::delete(&id)
}

/// Pull the published list from the Hub and cache it.
///
/// Runs on a blocking task and is never awaited on the render path: the UI
/// shows locally installed weights immediately, then folds this in. Returns the
/// cached list when offline instead of an error, so a failed refresh is a
/// no-op rather than a regression.
#[tauri::command]
async fn refresh_weights() -> Result<Vec<store::WeightFile>, String> {
    let fetched = tokio::task::spawn_blocking(store::refresh_catalogue).await;
    match fetched {
        Ok(Ok(files)) => Ok(files),
        // Offline: fall back to whatever was cached last time.
        Ok(Err(_)) => Ok(store::load_catalogue()),
        Err(e) => Err(format!("refresh task failed: {e}")),
    }
}

/// True when the cached catalogue is old enough to be worth refreshing.
#[tauri::command]
fn catalogue_is_stale() -> bool {
    store::catalogue_is_stale()
}

/// Free space at the models dir, shown before a download starts.
#[tauri::command]
fn models_free_space() -> u64 {
    downloads::free_bytes(&settings::resolve_models_dir(None))
}

/// Start (or resume) a download. Re-running with the same filename picks up
/// from whatever segments already exist on disk.
#[tauri::command]
fn start_download(
    app: tauri::AppHandle,
    filename: String,
    url: String,
    total: u64,
) -> Result<String, String> {
    let dir = settings::resolve_models_dir(None);
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;

    // Only ever fetch from the repo this store knows about.
    let allowed = format!("https://huggingface.co/{}/resolve/main/", store::REPO);
    if !url.starts_with(&allowed) {
        return Err(format!("refusing to fetch outside {allowed}"));
    }

    let id = filename.clone();
    downloads::start(id.clone(), filename, url, total, dir)?;

    let handle = app.clone();
    let watch = id.clone();
    // Progress is pushed rather than polled so the UI can show a live bar.
    std::thread::spawn(move || {
        use tauri::Emitter;
        loop {
            std::thread::sleep(std::time::Duration::from_millis(400));
            let statuses = downloads::statuses();
            let Some(current) = statuses.iter().find(|s| s.id == watch) else {
                // Gone from the registry: either finished and pruned, or
                // never started. Either way there is nothing left to report.
                break;
            };
            let _ = handle.emit("dl://progress", current);
            if matches!(
                current.state,
                downloads::DownloadState::Done
                    | downloads::DownloadState::Failed
                    | downloads::DownloadState::Cancelled
            ) {
                // Emit once more so the terminal state is not lost to the
                // cleanup that removes the row.
                let _ = handle.emit("dl://progress", current);
                break;
            }
        }
    });
    Ok(id)
}

#[tauri::command]
fn cancel_download(id: String) {
    downloads::cancel(&id);
}

#[tauri::command]
fn download_statuses() -> Vec<downloads::DownloadStatus> {
    downloads::statuses()
}

#[tauri::command]
fn clear_finished_downloads() {
    downloads::clear_finished();
}

#[tauri::command]
fn preset_download_script(preset_id: String) -> Result<String, String> {
    let dir = settings::resolve_models_dir(None);
    let files = store::scan_local(&dir);
    let preset = store::presets(&files)
        .into_iter()
        .find(|p| p.id == preset_id)
        .ok_or_else(|| format!("unknown preset: {preset_id}"))?;

    Ok(format!(
        "# {label} ({gib:.2} GiB)\n#\n# AceForge never downloads weights. Run this in your terminal:\n\
         mkdir -p \"{dir}\"\ncd \"{dir}\"\n\n{curl}\n",
        label = preset.label,
        gib = preset.total_bytes as f64 / 1024f64.powi(3),
        dir = dir.display(),
        curl = preset
            .files
            .iter()
            .map(|f| store::curl_command(f))
            .collect::<Vec<_>>()
            .join("\n\n")
    ))
}

#[tauri::command]
fn start_server(app: tauri::AppHandle) -> Result<server::ServerStatus, String> {
    let resource_dir = app.path().resource_dir().ok();
    server::start(&settings::resolve_models_dir(None), resource_dir.as_deref())
}

/// Tile size the sidecar is launched with. Changing it needs a restart to
/// take effect, which the UI states plainly rather than silently ignoring.
#[tauri::command]
fn set_vae_chunk(chunk: i32) -> Result<SettingsView, String> {
    let mut current = settings::load();
    current.vae_chunk = chunk.clamp(64, 1024);
    settings::save(&current)?;
    Ok(get_settings())
}

/// LM KV cache size the sidecar launches with.
/// Point audio.cpp at its own model directory.
#[tauri::command]
fn set_audiocpp_models_dir(path: String) -> Result<SettingsView, String> {
    let mut current = settings::load();
    let trimmed = path.trim();
    if trimmed.is_empty() {
        current.audiocpp_models_dir = String::new();
    } else {
        let resolved = settings::normalize(trimmed)?;
        std::fs::create_dir_all(&resolved)
            .map_err(|e| format!("cannot create {}: {e}", resolved.display()))?;
        current.audiocpp_models_dir = resolved.display().to_string();
    }
    settings::save(&current)?;
    Ok(get_settings())
}

#[tauri::command]
fn set_max_seq(value: i32) -> Result<SettingsView, String> {
    let mut current = settings::load();
    current.max_seq = value.clamp(1024, 32768);
    settings::save(&current)?;
    Ok(get_settings())
}

#[tauri::command]
fn stop_server() -> Result<usize, String> {
    server::stop()
}

/// Tail of the sidecar's stderr.
///
/// This is the only place the backend explains itself: a failed job carries no
/// error field, so "GGUF is truncated" exists nowhere else. Surfacing it in the
/// app removes the terminal from the diagnosis loop.
#[tauri::command]
fn server_logs(limit: Option<usize>) -> Vec<String> {
    let dir = settings::resolve_models_dir(None);
    server::stderr_tail(&dir, limit.unwrap_or(2000).min(20_000))
}

/// Byte length of the log, so the viewer can resume without re-sending the
/// whole thing every second.
#[tauri::command]
fn server_log_cursor() -> u64 {
    let dir = settings::resolve_models_dir(None);
    server::stderr_len(&dir)
}

/// Everything written since `cursor`. Empty when nothing changed, which is the
/// common case and must not clear the view.
#[tauri::command]
fn server_logs_since(cursor: u64, limit: Option<usize>) -> Vec<String> {
    let dir = settings::resolve_models_dir(None);
    server::stderr_since(&dir, cursor, limit.unwrap_or(2000).min(20_000))
}

/// The port the sidecar came up on. The UI needs it for every /job call, and
/// it is only known once the handshake completes.
#[tauri::command]
fn server_port() -> Option<u16> {
    server::running_port()
}

/// Run one generation. Progress is emitted as `gen://stage` so the UI can
/// show LM planning and DiT rendering separately instead of one opaque bar.
#[tauri::command]
async fn generate_track(
    app: tauri::AppHandle,
    port: u16,
    params: generate::GenerateParams,
) -> Result<generate::GenerateOutcome, String> {
    // The pipeline is blocking and CPU/GPU bound, so keep it off the async
    // runtime's worker threads.
    tokio::task::spawn_blocking(move || {
        use tauri::Emitter;
        let emitter = app.clone();
        let mut stage = move |stage: &str, detail: &str| {
            let _ = emitter.emit("gen://stage", (stage, detail));
        };
        // Publish the live job id so the UI can offer a real cancel.
        let job_emitter = app.clone();
        let mut job = move |id: &str| {
            let _ = job_emitter.emit("gen://job", id);
        };
        generate::generate(port, &params, &mut stage, &mut job)
    })
    .await
    .map_err(|e| format!("generation task panicked: {e}"))?
}

#[tauri::command]
fn cancel_track(port: u16, job_id: String) -> Result<(), String> {
    generate::cancel(port, &job_id)
}

#[tauri::command]
fn library() -> Vec<generate::Track> {
    generate::list_tracks()
}

/// An imported source file, as the pickers need it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedFile {
    /// Filename in the library. The copy keeps the original name.
    pub name: String,
    /// Absolute path inside the library.
    pub path: String,
    pub bytes: u64,
}

/// Copy a file chosen in the system dialog into the library.
///
/// Only WAV and MP3 are accepted, and not as a guess: `audio_read_buf` in
/// `acestep.cpp` sniffs for a RIFF header and otherwise falls through to the
/// MP3 decoder, so a FLAC or an M4A would be accepted silently and then fail
/// much later with "Failed to decode audio". Rejecting it here names the
/// problem instead.
///
/// Copied rather than referenced so the library stays self-contained and the
/// containment check on `source_audio`/`ref_audio` keeps holding: a path the
/// frontend supplies must never point outside the library.
#[tauri::command]
fn import_audio(sources: Vec<String>) -> Result<Vec<ImportedFile>, String> {
    if sources.is_empty() {
        return Err("nothing selected".to_string());
    }
    let dir = paths::imports_dir();
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;

    let mut out = Vec::with_capacity(sources.len());
    for src in sources {
        let from = std::path::PathBuf::from(src.trim());
        let name = from
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| "no filename".to_string())?
            .to_string();
        let ext = from
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !importable_extension(&ext) {
            return Err(format!(
                "{name}: the backend reads only .wav and .mp3. \
                 FLAC, OGG and M4A are not decoded."
            ));
        }
        if !from.is_file() {
            return Err(format!("{name}: not a file"));
        }

        let stem = from.file_stem().and_then(|s| s.to_str()).unwrap_or("import");
        let target = deduped_path(&dir, stem, &ext);
        std::fs::copy(&from, &target).map_err(|e| format!("cannot import {name}: {e}"))?;
        let bytes = std::fs::metadata(&target).map(|m| m.len()).unwrap_or(0);
        out.push(ImportedFile {
            name: target
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(&name)
                .to_string(),
            path: target.display().to_string(),
            bytes,
        });
    }
    Ok(out)
}

/// First free name in `dir`: `stem.ext`, then `stem-2.ext`, and so on.
///
/// Importing two different files both called `mix.mp3` must not silently
/// overwrite the first one, which is how a source track changes underneath a
/// saved preset.
fn deduped_path(dir: &std::path::Path, stem: &str, ext: &str) -> std::path::PathBuf {
    let first = dir.join(format!("{stem}.{ext}"));
    if !first.exists() {
        return first;
    }
    for n in 2..10_000u32 {
        let candidate = dir.join(format!("{stem}-{n}.{ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    dir.join(format!("{stem}-{}.{ext}", std::process::id()))
}

/// Formats the backend can actually decode. See `import_audio`.
fn importable_extension(ext: &str) -> bool {
    let e = ext.to_ascii_lowercase();
    e == "wav" || e == "mp3"
}

/// Previously imported files, newest first.
///
/// Deliberately separate from the generated library: the source and voice
/// pickers want the handful of files you brought in, not every song the app
/// has ever rendered.
#[tauri::command]
fn list_imports() -> Vec<ImportedFile> {
    let dir = paths::imports_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out: Vec<ImportedFile> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let path = e.path();
            let ext = path.extension().and_then(|x| x.to_str()).unwrap_or("");
            if !importable_extension(ext) {
                return None;
            }
            let meta = e.metadata().ok()?;
            Some(ImportedFile {
                name: path.file_name()?.to_str()?.to_string(),
                path: path.display().to_string(),
                bytes: meta.len(),
            })
        })
        .collect();
    // By modification time, so the one just imported is at the top.
    out.sort_by(|a, b| {
        let ta = std::fs::metadata(&a.path).and_then(|m| m.modified()).ok();
        let tb = std::fs::metadata(&b.path).and_then(|m| m.modified()).ok();
        tb.cmp(&ta).then_with(|| a.name.cmp(&b.name))
    });
    out
}

#[tauri::command]
fn remove_import(path: String) -> Result<(), String> {
    let target = std::path::PathBuf::from(path);
    let dir = paths::imports_dir();
    let real = target
        .canonicalize()
        .map_err(|e| format!("cannot resolve: {e}"))?;
    let real_dir = dir
        .canonicalize()
        .map_err(|e| format!("imports directory missing: {e}"))?;
    if !real.starts_with(&real_dir) {
        return Err("refusing to delete outside the imports directory".to_string());
    }
    std::fs::remove_file(&real).map_err(|e| format!("cannot delete: {e}"))
}

/// Point the library at a directory of the user's choosing.
#[tauri::command]
fn set_library_dir(app: tauri::AppHandle, path: String) -> Result<SettingsView, String> {
    let trimmed = path.trim();
    if !trimmed.is_empty() {
        let dir = std::path::PathBuf::from(trimmed);
        std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        // Playback goes through the asset protocol, whose scope is fixed at
        // build time. Widen it now or songs land here and never play.
        use tauri::Manager;
        app.asset_protocol_scope()
            .allow_directory(&dir, true)
            .map_err(|e| format!("cannot grant read access to {}: {e}", dir.display()))?;
    }

    let mut current = settings::load();
    current.library_dir = trimmed.to_string();
    settings::save(&current)?;
    // Rebuilded rather than patched: `resolved_library_dir` has to reflect the
    // directory that was just written, and duplicating that logic here is how
    // the two drift apart.
    Ok(get_settings())
}

#[tauri::command]
fn delete_track(path: String) -> Result<(), String> {
    // Only ever delete inside our own library directory.
    let lib = paths::library_dir();
    let target = PathBuf::from(&path);
    if !target.starts_with(&lib) {
        return Err("refusing to delete outside the library".to_string());
    }
    std::fs::remove_file(&target)
        .map_err(|e| format!("cannot delete {}: {e}", target.display()))
}

#[tauri::command]
fn reveal_track(app: tauri::AppHandle, path: String) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    let target = PathBuf::from(&path);
    if !target.is_file() {
        return Err(format!("{} is not a file", target.display()));
    }
    let lib = paths::library_dir();
    if !target.starts_with(&lib) {
        return Err("refusing to open a path outside the library".to_string());
    }
    app.opener()
        .reveal_item_in_dir(&target)
        .map_err(|e| format!("cannot reveal: {e}"))
}

#[tauri::command]
fn server_props(port: u16) -> Result<serde_json::Value, String> {
    server::props(port)
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            get_settings,
            set_models_dir,
            set_vae_chunk,
            set_max_seq,
            set_audiocpp_models_dir,
            store_snapshot,
            music_reference,
            apply_sampling_preset,
            audiocpp_start,
            audiocpp_models,
            audiocpp_stop,
            audiocpp_render,
            audiocpp_cancel,
            audiocpp_probe,
            audiocpp_scan,
            set_audiocpp_runtime,
            list_engines,
            list_adapters,
            adopt_bundled_engine,
            adopt_bundled_audiocpp,
            inspect_engine,
            install_engine,
            remove_engine,
            engines_dir,
            generation_presets,
            user_presets_list,
            user_preset_save,
            user_preset_delete,
            refresh_weights,
            catalogue_is_stale,
            models_free_space,
            start_download,
            cancel_download,
            download_statuses,
            clear_finished_downloads,
            preset_download_script,
            start_server,
            stop_server,
            server_logs,
            server_log_cursor,
            server_logs_since,
            server_port,
            server_props,
            generate_track,
            cancel_track,
            library,
            delete_track,
            reveal_track,
            import_audio,
            list_imports,
            remove_import,
            set_library_dir,
        ])
        .setup(|app| {
            // The asset-protocol scope ships as `$APPDATA/**`. A library
            // anywhere else would be written to and listed fine and then fail
            // to play, because the webview fetches audio through that protocol.
            // Widen the scope at runtime for wherever the library actually is.
            use tauri::Manager;
            let dir = paths::library_dir();
            if let Err(e) = app.asset_protocol_scope().allow_directory(&dir, true) {
                eprintln!("[library] asset scope for {} failed: {e}", dir.display());
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running AceForge");
}
#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch(std::path::PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "aceforge-import-{tag}-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("scratch");
            Self(dir)
        }

    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// The backend sniffs RIFF and otherwise hands the bytes to the MP3
    /// decoder. Importing a FLAC would be accepted here and then die much later
    /// with "Failed to decode audio", so it is refused up front by name.
    #[test]
    fn only_wav_and_mp3_may_be_imported() {
        assert!(importable_extension("wav"));
        assert!(importable_extension("mp3"));
        assert!(importable_extension("WAV"), "case must not matter");
        for bad in ["flac", "ogg", "m4a", "aac", "opus", "wma"] {
            assert!(
                !importable_extension(bad),
                "{bad} cannot be decoded and must be refused"
            );
        }
        assert!(!importable_extension(""), "no extension is not importable");
    }

    /// Importing two different files both named `mix.mp3` must not overwrite
    /// the first: a source track silently changing under a saved preset is
    /// exactly the kind of thing that is impossible to diagnose later.
    #[test]
    fn clashing_names_are_disambiguated_instead_of_overwriting() {
        let s = Scratch::new("dedup");
        let first = deduped_path(&s.0, "mix", "mp3");
        std::fs::write(&first, b"first").expect("write");
        assert_eq!(first.file_name().unwrap(), "mix.mp3");

        let second = deduped_path(&s.0, "mix", "mp3");
        std::fs::write(&second, b"second").expect("write");
        assert_eq!(second.file_name().unwrap(), "mix-2.mp3");

        let third = deduped_path(&s.0, "mix", "mp3");
        assert_eq!(third.file_name().unwrap(), "mix-3.mp3");

        assert_eq!(std::fs::read(&first).unwrap(), b"first", "not overwritten");
    }
}
