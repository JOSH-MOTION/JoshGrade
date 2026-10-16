// Hide the console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use base64::Engine;
use grade_core::scopes::{self, Scopes};
use grade_core::report::{self, Line, Measure};
use grade_core::{decode, encode, lut, Adjust, Analysis, Grade, OutputFormat, Photo, Settings};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tauri::{Emitter, State};

/// Decoded preview-size photo + analysis + "before" data, so sliders re-render instantly.
struct Entry {
    photo: Photo,
    analysis: Analysis,
    before_url: String,
    before_scopes: Scopes,
    before_measure: Measure,
}

#[derive(Default)]
struct AppState {
    cache: Mutex<HashMap<String, Arc<Entry>>>,
    cancel: Mutex<bool>,
}

type St<'a> = State<'a, Arc<AppState>>;

#[derive(Serialize)]
struct LookInfo {
    id: String,
    name: String,
    description: String,
}

#[derive(Serialize)]
struct Collected {
    photos: Vec<String>,
    skipped: Vec<String>,
}

#[derive(Serialize)]
struct PreviewResult {
    before: Option<String>,
    after: String,
    overlay: Option<String>,
    width: u32,
    height: u32,
    /// Measured before → after report for the current settings.
    report: Vec<Line>,
    /// The automatic correction in slider units (what the sliders start at).
    auto_adjust: Adjust,
    scopes_before: Scopes,
    scopes_after: Scopes,
}

#[derive(Deserialize)]
struct BatchItem {
    path: String,
    settings: Settings,
}

#[derive(Deserialize)]
struct BatchRequest {
    items: Vec<BatchItem>,
    format: OutputFormat,
    write_lut: bool,
    out_dir: String,
}

#[derive(Serialize, Clone)]
struct Progress {
    index: usize,
    total: usize,
    file: String,
    ok: bool,
    output: Option<String>,
    error: Option<String>,
}

fn jpeg_url(photo: &Photo, quality: u8) -> Result<String, String> {
    let bytes = encode::jpeg_bytes(photo, quality).map_err(|e| e.to_string())?;
    Ok(format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)))
}

fn png_url(img: &image::RgbaImage) -> Result<String, String> {
    let mut buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Png).map_err(|e| e.to_string())?;
    Ok(format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(buf.into_inner())))
}

fn cached(state: &AppState, path: &str) -> Result<Arc<Entry>, String> {
    if let Some(v) = state.cache.lock().unwrap().get(path) {
        return Ok(v.clone());
    }
    let photo = decode::load(Path::new(path), Some(grade_core::ANALYSIS_SIZE), true).map_err(|e| e.to_string())?;
    let analysis = grade_core::analyze(&photo);
    let before_url = jpeg_url(&photo, 88)?;
    let before_scopes = scopes::compute(&photo);
    let before_measure = report::measure(&photo);
    let entry = Arc::new(Entry { photo, analysis, before_url, before_scopes, before_measure });
    let mut c = state.cache.lock().unwrap();
    if c.len() > 16 {
        c.clear(); // keep memory bounded
    }
    c.insert(path.to_string(), entry.clone());
    Ok(entry)
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| e.to_string())?
}

#[tauri::command]
fn list_looks() -> Vec<LookInfo> {
    grade_core::all_looks()
        .into_iter()
        .map(|l| LookInfo { id: l.id.into(), name: l.name.into(), description: l.description.into() })
        .collect()
}

/// Expand dropped/selected files and folders (incl. subfolders) into supported photo paths.
#[tauri::command]
async fn collect_photos(paths: Vec<String>) -> Result<Collected, String> {
    blocking(move || {
        let p: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
        let (photos, skipped) = grade_core::collect_inputs(&p);
        let s = |v: Vec<PathBuf>| v.into_iter().map(|p| p.to_string_lossy().into_owned()).collect();
        Ok(Collected { photos: s(photos), skipped: s(skipped) })
    })
    .await
}

#[tauri::command]
async fn thumbnail(path: String) -> Result<String, String> {
    blocking(move || {
        let t = decode::thumbnail(Path::new(&path), 240).map_err(|e| e.to_string())?;
        jpeg_url(&t, 80)
    })
    .await
}

#[tauri::command]
async fn preview(state: St<'_>, path: String, settings: Settings, with_before: bool, clipping: bool) -> Result<PreviewResult, String> {
    let st = Arc::clone(&state);
    blocking(move || {
        let e = cached(&st, &path)?;
        let grade = grade_core::grade_with(&e.analysis, &settings).map_err(|e| e.to_string())?;
        let after = grade.apply(&e.photo);
        let overlay = if clipping { Some(png_url(&scopes::clipping_overlay(&after))?) } else { None };
        Ok(PreviewResult {
            before: with_before.then(|| e.before_url.clone()),
            after: jpeg_url(&after, 88)?,
            overlay,
            width: e.photo.width,
            height: e.photo.height,
            report: report::compare(&e.before_measure, &report::measure(&after)),
            auto_adjust: if settings.auto { grade_core::auto_sliders(&e.analysis.params) } else { Adjust::default() },
            scopes_before: e.before_scopes.clone(),
            scopes_after: scopes::compute(&after),
        })
    })
    .await
}

#[derive(Serialize)]
struct LookThumb {
    id: String,
    image: String,
}

/// The current photo rendered small in every look (with your adjustments), for the look picker.
#[tauri::command]
async fn look_thumbs(state: St<'_>, path: String, settings: Settings) -> Result<Vec<LookThumb>, String> {
    let st = Arc::clone(&state);
    blocking(move || {
        let e = cached(&st, &path)?;
        let small = shrink(&e.photo, 260);
        grade_core::all_looks()
            .into_iter()
            .map(|l| {
                let auto = if settings.auto { e.analysis.params } else { grade_core::AutoParams::identity() };
                let g = Grade::with_adjust(&auto, &l.params, &settings.adjust, settings.strength);
                Ok(LookThumb { id: l.id.into(), image: jpeg_url(&g.apply(&small), 80)? })
            })
            .collect()
    })
    .await
}

/// Box-filter downscale (fast, good enough for thumbnails).
fn shrink(p: &Photo, max: u32) -> Photo {
    let f = ((p.width.max(p.height) as f32) / max as f32).ceil().max(1.0) as u32;
    let (w, h) = ((p.width / f).max(1), (p.height / f).max(1));
    let mut data = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        for x in 0..w {
            let mut acc = [0f32; 3];
            for dy in 0..f {
                for dx in 0..f {
                    let px = p.data[((y * f + dy) * p.width + x * f + dx) as usize];
                    for c in 0..3 {
                        acc[c] += px[c];
                    }
                }
            }
            let n = (f * f) as f32;
            data.push(acc.map(|v| v / n));
        }
    }
    Photo { width: w, height: h, data, from_raw: p.from_raw }
}

/// Save the photo's grade as a .cube LUT (for Photoshop's Color Lookup layer).
#[tauri::command]
async fn export_lut(state: St<'_>, path: String, settings: Settings, dest: String) -> Result<String, String> {
    let st = Arc::clone(&state);
    blocking(move || {
        let e = cached(&st, &path)?;
        let grade = grade_core::grade_with(&e.analysis, &settings).map_err(|e| e.to_string())?;
        let stem = Path::new(&path).file_stem().and_then(|s| s.to_str()).unwrap_or("photo").to_string();
        std::fs::write(&dest, lut::cube_string(&grade, 33, &format!("{stem} - {}", settings.look))).map_err(|e| e.to_string())?;
        Ok(dest)
    })
    .await
}

fn format_for(dest: &Path, fallback: OutputFormat) -> OutputFormat {
    match dest.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("jpg") | Some("jpeg") => OutputFormat::Jpeg,
        Some("tif") | Some("tiff") => OutputFormat::Tiff,
        Some("png") => OutputFormat::Png,
        _ => fallback,
    }
}

/// Save one photo, full resolution, exactly where you chose.
#[tauri::command]
async fn save_photo(path: String, settings: Settings, dest: String, format: OutputFormat, write_lut: bool) -> Result<String, String> {
    blocking(move || {
        let dest = PathBuf::from(dest);
        let fmt = format_for(&dest, format);
        let r = grade_core::save_graded(Path::new(&path), &dest, &settings, fmt, 95, write_lut).map_err(|e| e.to_string())?;
        Ok(r.output.to_string_lossy().into_owned())
    })
    .await
}

#[tauri::command]
fn cancel_batch(state: St<'_>) {
    *state.cancel.lock().unwrap() = true;
}

/// Save many photos (each with its own settings) into one folder, reporting "save-progress" events.
#[tauri::command]
async fn save_batch(app: tauri::AppHandle, state: St<'_>, req: BatchRequest) -> Result<usize, String> {
    *state.cancel.lock().unwrap() = false;
    let st = Arc::clone(&state);
    blocking(move || {
        let total = req.items.len();
        let dir = PathBuf::from(&req.out_dir);
        let mut done = 0;
        for (i, item) in req.items.iter().enumerate() {
            if *st.cancel.lock().unwrap() {
                break;
            }
            let input = PathBuf::from(&item.path);
            let dest = grade_core::unique_output(&input, &dir, req.format);
            let ev = match grade_core::save_graded(&input, &dest, &item.settings, req.format, 95, req.write_lut) {
                Ok(r) => {
                    done += 1;
                    Progress { index: i, total, file: item.path.clone(), ok: true, output: Some(r.output.to_string_lossy().into_owned()), error: None }
                }
                Err(e) => Progress { index: i, total, file: item.path.clone(), ok: false, output: None, error: Some(e.to_string()) },
            };
            let _ = app.emit("save-progress", ev);
        }
        Ok(done)
    })
    .await
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(Arc::new(AppState::default()))
        .invoke_handler(tauri::generate_handler![
            list_looks,
            collect_photos,
            thumbnail,
            preview,
            look_thumbs,
            export_lut,
            save_photo,
            save_batch,
            cancel_batch
        ])
        .run(tauri::generate_context!())
        .expect("error while running JoshGrade");
}
