// Hide the console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use base64::Engine;
use grade_core::{decode, encode, lut, Analysis, Grade, Job, OutputFormat, Photo};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tauri::{Emitter, State};

/// Decoded preview-size photos + their analysis, so moving a slider re-renders instantly.
#[derive(Default)]
struct AppState {
    cache: Mutex<HashMap<String, Arc<(Photo, Analysis)>>>,
    cancel: Mutex<bool>,
}

#[derive(Serialize)]
struct LookInfo {
    id: String,
    name: String,
    description: String,
}

#[derive(Serialize)]
struct PreviewResult {
    before: String,
    after: String,
    width: u32,
    height: u32,
    notes: Vec<String>,
    params: grade_core::AutoParams,
}

#[derive(Deserialize)]
struct BatchRequest {
    paths: Vec<String>,
    look: String,
    strength: f32,
    format: OutputFormat,
    write_lut: bool,
    /// None = a "Graded" folder next to each photo.
    out_dir: Option<String>,
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

fn data_url(photo: &Photo, quality: u8) -> Result<String, String> {
    let bytes = encode::jpeg_bytes(photo, quality).map_err(|e| e.to_string())?;
    Ok(format!(
        "data:image/jpeg;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}

fn cached(state: &AppState, path: &str) -> Result<Arc<(Photo, Analysis)>, String> {
    if let Some(v) = state.cache.lock().unwrap().get(path) {
        return Ok(v.clone());
    }
    let photo = decode::load(Path::new(path), Some(grade_core::ANALYSIS_SIZE), true).map_err(|e| e.to_string())?;
    let analysis = grade_core::analyze(&photo);
    let entry = Arc::new((photo, analysis));
    let mut c = state.cache.lock().unwrap();
    if c.len() > 24 {
        c.clear(); // keep memory bounded
    }
    c.insert(path.to_string(), entry.clone());
    Ok(entry)
}

fn grade_from(analysis: &Analysis, look: &str, strength: f32) -> Result<Grade, String> {
    let look = grade_core::find_look(look).ok_or_else(|| format!("unknown look {look}"))?;
    Ok(Grade::new(&analysis.params, &look.params, strength))
}

#[tauri::command]
fn list_looks() -> Vec<LookInfo> {
    grade_core::all_looks()
        .into_iter()
        .map(|l| LookInfo { id: l.id.into(), name: l.name.into(), description: l.description.into() })
        .collect()
}

/// Expand dropped/selected files and folders into supported photo paths.
#[tauri::command]
fn collect_photos(paths: Vec<String>) -> Vec<String> {
    let p: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
    grade_core::collect_inputs(&p)
        .into_iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect()
}

#[tauri::command]
async fn thumbnail(path: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let t = decode::thumbnail(Path::new(&path), 240).map_err(|e| e.to_string())?;
        data_url(&t, 80)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn preview(state: State<'_, Arc<AppState>>, path: String, look: String, strength: f32) -> Result<PreviewResult, String> {
    // Decoding can take a few seconds for RAW files – do it off the UI thread.
    let st = Arc::clone(&state);
    let p = path.clone();
    let entry = tauri::async_runtime::spawn_blocking(move || cached(&st, &p))
        .await
        .map_err(|e| e.to_string())??;
    tauri::async_runtime::spawn_blocking(move || {
        let (photo, analysis) = (&entry.0, &entry.1);
        let grade = grade_from(analysis, &look, strength)?;
        let after = grade.apply(photo);
        Ok(PreviewResult {
            before: data_url(photo, 88)?,
            after: data_url(&after, 88)?,
            width: photo.width,
            height: photo.height,
            notes: analysis.notes.clone(),
            params: analysis.params,
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Serialize)]
struct LookThumb {
    id: String,
    image: String,
}

/// The current photo rendered small in every look, for the look picker.
#[tauri::command]
async fn look_thumbs(state: State<'_, Arc<AppState>>, path: String, strength: f32) -> Result<Vec<LookThumb>, String> {
    let st = Arc::clone(&state);
    tauri::async_runtime::spawn_blocking(move || {
        let entry = cached(&st, &path)?;
        let small = shrink(&entry.0, 260);
        grade_core::all_looks()
            .into_iter()
            .map(|l| {
                let g = Grade::new(&entry.1.params, &l.params, strength);
                Ok(LookThumb { id: l.id.into(), image: data_url(&g.apply(&small), 80)? })
            })
            .collect()
    })
    .await
    .map_err(|e| e.to_string())?
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

/// Save the current photo's grade as a .cube LUT (for Photoshop's Color Lookup layer).
#[tauri::command]
async fn export_lut(state: State<'_, Arc<AppState>>, path: String, look: String, strength: f32, dest: String) -> Result<String, String> {
    let entry = cached(&state, &path)?;
    let grade = grade_from(&entry.1, &look, strength)?;
    let stem = Path::new(&path).file_stem().and_then(|s| s.to_str()).unwrap_or("photo").to_string();
    let cube = lut::cube_string(&grade, 33, &format!("{stem} - {look}"));
    std::fs::write(&dest, cube).map_err(|e| e.to_string())?;
    Ok(dest)
}

#[tauri::command]
fn cancel_batch(state: State<'_, Arc<AppState>>) {
    *state.cancel.lock().unwrap() = true;
}

/// Grade a list of photos at full resolution, reporting progress through "grade-progress" events.
#[tauri::command]
async fn grade_batch(app: tauri::AppHandle, state: State<'_, Arc<AppState>>, req: BatchRequest) -> Result<usize, String> {
    *state.cancel.lock().unwrap() = false;
    let st = Arc::clone(&state);
    tauri::async_runtime::spawn_blocking(move || {
        let job = Job {
            look: req.look.clone(),
            strength: req.strength,
            format: req.format,
            quality: 95,
            write_lut: req.write_lut,
        };
        let total = req.paths.len();
        let mut done = 0;
        for (i, p) in req.paths.iter().enumerate() {
            if *st.cancel.lock().unwrap() {
                break;
            }
            let input = PathBuf::from(p);
            let dir = match &req.out_dir {
                Some(d) => PathBuf::from(d),
                None => input.parent().unwrap_or(Path::new(".")).join("Graded"),
            };
            let res = grade_core::process_file(&input, &dir, &job);
            let ev = match res {
                Ok(r) => {
                    done += 1;
                    Progress { index: i, total, file: p.clone(), ok: true, output: Some(r.output.to_string_lossy().into_owned()), error: None }
                }
                Err(e) => Progress { index: i, total, file: p.clone(), ok: false, output: None, error: Some(e.to_string()) },
            };
            let _ = app.emit("grade-progress", ev);
        }
        Ok(done)
    })
    .await
    .map_err(|e| e.to_string())?
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
            grade_batch,
            cancel_batch
        ])
        .run(tauri::generate_context!())
        .expect("error while running JoshGrade");
}
