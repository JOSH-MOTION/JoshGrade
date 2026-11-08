//! Video commands: PluralEyes-style audio sync and offline auto captions.

use media_core::captions::{self, BurnStyle, Cue, CueStyle, TranscribeOptions, Transcript, Word};
use media_core::export::{self, RenderOptions};
use media_core::ffmpeg::{self, MediaInfo, Tools};
use media_core::sync::{self, Match, SyncReport};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager, State};

#[derive(Default)]
pub struct VideoState {
    cancel: Arc<AtomicBool>,
    /// Loaded caption engine (model id, context) — kept so a second video starts instantly.
    whisper: Mutex<Option<(String, Arc<whisper_rs::WhisperContext>)>>,
}

pub type VSt<'a> = State<'a, Arc<VideoState>>;

fn tools() -> Result<Tools, String> {
    Tools::locate().map_err(|e| e.to_string())
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| e.to_string())?
}

fn models_dir(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app.path().app_data_dir().map_err(|e| e.to_string())?.join("caption-models"))
}

#[derive(Serialize)]
pub struct MediaItem {
    path: String,
    kind: &'static str, // "video" | "audio"
    info: Option<MediaInfo>,
    error: Option<String>,
}

#[derive(Serialize)]
pub struct Collected {
    items: Vec<MediaItem>,
    skipped: Vec<String>,
}

fn walk(p: &Path, depth: usize, out: &mut Vec<PathBuf>, skipped: &mut Vec<String>) {
    if p.is_dir() {
        if depth > 3 {
            return;
        }
        if let Ok(rd) = std::fs::read_dir(p) {
            let mut v: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
            v.sort();
            for e in v {
                let name = e.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if !name.starts_with('.') && name != "Synced" && name != "Captioned" {
                    walk(&e, depth + 1, out, skipped);
                }
            }
        }
    } else if media_core::is_video(p) || media_core::is_audio(p) {
        out.push(p.to_path_buf());
    } else if p.is_file() && depth == 0 {
        skipped.push(p.to_string_lossy().into_owned());
    }
}

/// Expand dropped files/folders into video and audio files, with their details.
#[tauri::command]
pub async fn media_collect(paths: Vec<String>) -> Result<Collected, String> {
    blocking(move || {
        let t = tools()?;
        let mut found = Vec::new();
        let mut skipped = Vec::new();
        for p in &paths {
            walk(Path::new(p), 0, &mut found, &mut skipped);
        }
        let items = found
            .into_iter()
            .map(|p| {
                let kind = if media_core::is_video(&p) { "video" } else { "audio" };
                match ffmpeg::probe(&t, &p) {
                    // A "video" with no picture (e.g. some .mp4 voice memos) counts as audio.
                    Ok(info) => MediaItem { kind: if info.has_video { kind } else { "audio" }, path: p.to_string_lossy().into_owned(), info: Some(info), error: None },
                    Err(e) => MediaItem { kind, path: p.to_string_lossy().into_owned(), info: None, error: Some(e.to_string()) },
                }
            })
            .collect();
        Ok(Collected { items, skipped })
    })
    .await
}

#[tauri::command]
pub fn media_cancel(state: VSt<'_>) {
    state.cancel.store(true, Ordering::Relaxed);
}

// ------------------------------------------------------------------ sync

#[tauri::command]
pub async fn sync_run(app: AppHandle, state: VSt<'_>, clips: Vec<String>, references: Vec<String>) -> Result<SyncReport, String> {
    state.cancel.store(false, Ordering::Relaxed);
    let cancel = state.cancel.clone();
    blocking(move || {
        let t = tools()?;
        let clips: Vec<PathBuf> = clips.into_iter().map(PathBuf::from).collect();
        let refs: Vec<PathBuf> = references.into_iter().map(PathBuf::from).collect();
        sync::sync(&t, &clips, &refs, &cancel, &mut |p| {
            let _ = app.emit("sync-progress", p);
        })
        .map_err(|e| e.to_string())
    })
    .await
}

#[derive(Serialize, Clone)]
struct FileProgress {
    index: usize,
    total: usize,
    file: String,
    output: Option<String>,
    error: Option<String>,
}

#[tauri::command]
pub async fn sync_render(app: AppHandle, state: VSt<'_>, matches: Vec<Match>, out_dir: String, options: RenderOptions) -> Result<usize, String> {
    state.cancel.store(false, Ordering::Relaxed);
    let cancel = state.cancel.clone();
    blocking(move || {
        let t = tools()?;
        let total = matches.len();
        let mut ok = 0;
        for (i, m) in matches.iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let r = export::render_synced(&t, m, Path::new(&out_dir), &options);
            if r.is_ok() {
                ok += 1;
            }
            let _ = app.emit(
                "file-progress",
                FileProgress {
                    index: i,
                    total,
                    file: m.clip.clone(),
                    output: r.as_ref().ok().map(|p| p.to_string_lossy().into_owned()),
                    error: r.err().map(|e| e.to_string()),
                },
            );
        }
        Ok(ok)
    })
    .await
}

#[tauri::command]
pub async fn sync_timeline(report: SyncReport, dest: String) -> Result<String, String> {
    blocking(move || {
        std::fs::write(&dest, export::timeline_xml(&report.matches, &report.references)).map_err(|e| e.to_string())?;
        Ok(dest)
    })
    .await
}

// ------------------------------------------------------------------ captions

#[derive(Serialize)]
pub struct ModelState {
    id: &'static str,
    label: &'static str,
    size_mb: u32,
    description: &'static str,
    downloaded: bool,
}

#[tauri::command]
pub fn caption_models(app: AppHandle) -> Result<Vec<ModelState>, String> {
    let dir = models_dir(&app)?;
    Ok(captions::MODELS
        .iter()
        .map(|m| ModelState { id: m.id, label: m.label, size_mb: m.size_mb, description: m.description, downloaded: dir.join(m.file).exists() })
        .collect())
}

#[derive(Serialize, Clone)]
struct DownloadProgress {
    id: String,
    got: u64,
    total: u64,
}

#[tauri::command]
pub async fn caption_download(app: AppHandle, state: VSt<'_>, id: String) -> Result<(), String> {
    state.cancel.store(false, Ordering::Relaxed);
    let cancel = state.cancel.clone();
    let dir = models_dir(&app)?;
    blocking(move || {
        let mut last = 0u64;
        captions::download_model(&dir, &id, &cancel, |got, total| {
            if got - last > 1_000_000 || got == total {
                last = got;
                let _ = app.emit("model-progress", DownloadProgress { id: id.clone(), got, total });
            }
        })
        .map(|_| ())
        .map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command]
pub async fn caption_transcribe(app: AppHandle, state: VSt<'_>, path: String, model: String, language: String, translate: bool) -> Result<Transcript, String> {
    state.cancel.store(false, Ordering::Relaxed);
    let cancel = state.cancel.clone();
    let st = Arc::clone(&state);
    let dir = models_dir(&app)?;
    blocking(move || {
        let t = tools()?;
        let mp = captions::model_path(&dir, &model).ok_or("unknown caption engine")?;
        if !mp.exists() {
            return Err("Download the caption engine first.".into());
        }
        let ctx = {
            let mut w = st.whisper.lock().unwrap();
            match &*w {
                Some((id, c)) if *id == model => c.clone(),
                _ => {
                    let _ = app.emit("caption-progress", captions::TranscribeProgress { percent: 0.0, latest: "Loading the caption engine…".into() });
                    let c = Arc::new(captions::load_model(&mp).map_err(|e| e.to_string())?);
                    *w = Some((model.clone(), c.clone()));
                    c
                }
            }
        };
        captions::transcribe(&t, &ctx, Path::new(&path), &TranscribeOptions { language, translate }, &cancel, &mut |p| {
            let _ = app.emit("caption-progress", p);
        })
        .map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command]
pub fn caption_cues(words: Vec<Word>, style: CueStyle) -> Vec<Cue> {
    captions::build_cues(&words, &style)
}

/// Save captions as a file: "srt", "vtt" or "txt".
#[tauri::command]
pub fn caption_save(cues: Vec<Cue>, words: Vec<Word>, format: String, dest: String) -> Result<String, String> {
    let text = match format.as_str() {
        "vtt" => captions::to_vtt(&cues),
        "txt" => {
            if words.is_empty() {
                cues.iter().map(|c| c.text.replace('\n', " ")).collect::<Vec<_>>().join("\n")
            } else {
                captions::to_txt(&words)
            }
        }
        _ => captions::to_srt(&cues),
    };
    std::fs::write(&dest, text).map_err(|e| e.to_string())?;
    Ok(dest)
}

#[derive(Deserialize)]
pub struct IntoVideo {
    path: String,
    cues: Vec<Cue>,
    language: String,
    /// "track" (switchable, instant) or "burn" (into the picture).
    mode: String,
    style: BurnStyle,
    out_dir: String,
}

#[tauri::command]
pub async fn caption_into_video(req: IntoVideo) -> Result<String, String> {
    blocking(move || {
        let t = tools()?;
        let out = if req.mode == "burn" {
            if !t.can_burn_subtitles() {
                return Err("This copy of ffmpeg can't burn in captions. Use 'caption track' instead.".into());
            }
            captions::burn_captions(&t, Path::new(&req.path), &req.cues, &req.style, Path::new(&req.out_dir))
        } else {
            captions::add_caption_track(&t, Path::new(&req.path), &req.cues, &req.language, Path::new(&req.out_dir))
        };
        out.map(|p| p.to_string_lossy().into_owned()).map_err(|e| e.to_string())
    })
    .await
}
