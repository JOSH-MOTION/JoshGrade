// Hide the console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use base64::Engine;
use grade_core::scopes::{self, Scopes};
use grade_core::report::{self, Line, Measure};
use grade_core::retouch::{self, Face, RetouchSettings};
use grade_core::{decode, encode, geometry, lut, masks, Adjust, Analysis, Grade, OutputFormat, Photo, Settings};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tauri::{Emitter, State};

mod jobs;
mod video;

/// Decoded preview-size photo + analysis + "before" data, so sliders re-render instantly.
struct Entry {
    photo: Photo,
    analysis: Analysis,
    before_url: String,
    before_scopes: Scopes,
    before_measure: Measure,
    /// Faces found for retouching (found once, the first time retouch is used).
    faces: Mutex<Option<Arc<Vec<Face>>>>,
    /// Full-resolution size (upright), for 100% zoom.
    full_dims: (u32, u32),
    /// The last retouched copy, so moving a grade slider doesn't redo the retouch.
    retouched: Mutex<Option<(RetouchSettings, Arc<Photo>)>>,
    /// Where the person and the sky are (found the first time an AI mask is used).
    seg: Mutex<Option<Arc<masks::Segments>>>,
}

impl Entry {
    /// Person / sky maps, when a mask needs them.
    fn segments(&self, m: &[masks::Mask]) -> Result<Option<Arc<masks::Segments>>, String> {
        if !masks::needs_ai(m) {
            return Ok(None);
        }
        let mut g = self.seg.lock().unwrap();
        if let Some(s) = &*g {
            return Ok(Some(s.clone()));
        }
        let s = Arc::new(masks::segment(&self.photo).map_err(|e| if e == "model-missing" { "mask-model-missing".to_string() } else { e })?);
        *g = Some(s.clone());
        Ok(Some(s))
    }

    /// The photo to grade: the original, or the retouched copy when retouch is on.
    fn base(&self, r: &RetouchSettings) -> Result<(Option<Arc<Photo>>, Option<RetouchInfo>), String> {
        if !r.active() {
            return Ok((None, None));
        }
        if let Some((rs, p)) = &*self.retouched.lock().unwrap() {
            if rs == r {
                let n = self.faces.lock().unwrap().as_ref().map(|f| f.len()).unwrap_or(0);
                return Ok((Some(p.clone()), Some(RetouchInfo { faces: n })));
            }
        }
        let faces = {
            let mut g = self.faces.lock().unwrap();
            match &*g {
                Some(f) => f.clone(),
                None => {
                    let f = Arc::new(retouch::find_faces(&self.photo).map_err(|e| match e {
                        retouch::RetouchError::ModelMissing => "retouch-model-missing".to_string(),
                        e => e.to_string(),
                    })?);
                    *g = Some(f.clone());
                    f
                }
            }
        };
        let p = Arc::new(retouch::apply(&self.photo, &faces, r));
        *self.retouched.lock().unwrap() = Some((*r, p.clone()));
        Ok((Some(p), Some(RetouchInfo { faces: faces.len() })))
    }
}

#[derive(Serialize, Clone)]
struct RetouchInfo {
    faces: usize,
}

#[derive(Default)]
struct AppState {
    cache: Mutex<HashMap<String, Arc<Entry>>>,
    cancel: Mutex<bool>,
    /// Full-resolution copy of the photo being zoomed into (path + retouch settings).
    full: Mutex<Option<(String, RetouchSettings, Arc<Photo>)>>,
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
    /// Set when retouch is on: how many faces were retouched.
    retouch: Option<RetouchInfo>,
    /// Full-resolution size of the (turned) photo, for 100% zoom.
    full_width: u32,
    full_height: u32,
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
    // decoded straight to screen size (RAW: shrunk before the slow colour steps)
    let (photo, full_dims) = decode::load_preview(Path::new(path), grade_core::ANALYSIS_SIZE).map_err(|e| e.to_string())?;
    let analysis = grade_core::analyze(&photo);
    let before_url = jpeg_url(&photo, 88)?;
    let before_scopes = scopes::compute(&photo);
    let before_measure = report::measure(&photo);
    let entry = Arc::new(Entry { photo, analysis, before_url, before_scopes, before_measure, faces: Mutex::new(None), retouched: Mutex::new(None), seg: Mutex::new(None), full_dims });
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

/// Get the next photos ready in the background (like Lightroom's preview cache), so moving
/// to them is instant.
#[tauri::command]
async fn prefetch(state: St<'_>, paths: Vec<String>) -> Result<(), String> {
    let st = Arc::clone(&state);
    blocking(move || {
        for p in paths.iter().take(2) {
            let _ = cached(&st, p);
        }
        Ok(())
    })
    .await
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
        let (base, retouch) = e.base(&settings.retouch)?;
        let cr = settings.crop;
        let turned = cr.rot90 % 4 != 0 || cr.flip_h;
        let after = grade.apply(base.as_deref().unwrap_or(&e.photo));
        let seg = if masks::active(&settings.masks) { e.segments(&settings.masks)? } else { None };
        let after = masks::apply(&after, &settings.masks, seg.as_deref(), &masks::View::whole(&after));
        // detail at preview scale (sharpening only shows at 100% zoom, like Lightroom)
        let k = e.photo.width.max(e.photo.height) as f32 / e.full_dims.0.max(e.full_dims.1).max(1) as f32;
        let after = grade_core::detail::apply(&after, &settings.detail, k);
        // Quarter turns are applied here; straighten + crop are shown by the viewer.
        let after = geometry::turn(&after, cr.rot90, cr.flip_h);
        let frame = grade_core::detail::Frame::of_crop(&cr, after.width as f32, after.height as f32);
        let after = grade_core::detail::finish_frame(&after, &settings.detail, &frame, k);
        let overlay = if clipping { Some(png_url(&scopes::clipping_overlay(&after))?) } else { None };
        let (fw, fh) = if cr.rot90 % 2 == 1 { (e.full_dims.1, e.full_dims.0) } else { e.full_dims };
        Ok(PreviewResult {
            full_width: fw,
            full_height: fh,
            before: if !with_before { None } else if turned { Some(jpeg_url(&geometry::turn(&e.photo, cr.rot90, cr.flip_h), 88)?) } else { Some(e.before_url.clone()) },
            after: jpeg_url(&after, 88)?,
            overlay,
            width: after.width,
            height: after.height,
            report: report::compare(&e.before_measure, &report::measure(&after)),
            auto_adjust: if settings.auto { grade_core::auto_sliders(&e.analysis.params) } else { Adjust::default() },
            scopes_before: e.before_scopes.clone(),
            scopes_after: scopes::compute(&after),
            retouch,
        })
    })
    .await
}

/// A sharp, full-resolution piece of the photo for zooming in. `region` = x, y, w, h as
/// fractions of the (turned) photo; the result is at most `max_px` on its long side.
#[tauri::command]
async fn preview_region(state: St<'_>, path: String, settings: Settings, region: [f32; 4], max_px: u32, before: bool) -> Result<String, String> {
    let st = Arc::clone(&state);
    blocking(move || {
        let e = cached(&st, &path)?;
        let r = if before { RetouchSettings::default() } else { settings.retouch };
        // full-resolution (retouched) base, cached for the photo being zoomed
        let base = {
            let hit = st.full.lock().unwrap().as_ref().filter(|(p, rs, _)| *p == path && (*rs == r || !r.active() && !rs.active())).map(|x| x.2.clone());
            match hit {
                Some(b) => b,
                None => {
                    *st.full.lock().unwrap() = None; // free the old one first
                    let mut full = decode::load(Path::new(&path), None, true).map_err(|e| e.to_string())?;
                    if r.active() {
                        let faces = retouch::find_faces(&e.photo).map_err(|e| e.to_string())?;
                        full = retouch::apply(&full, &faces, &r);
                    }
                    let b = Arc::new(full);
                    *st.full.lock().unwrap() = Some((path.clone(), r, b.clone()));
                    b
                }
            }
        };
        // region in the turned frame -> the stored (upright) frame
        let cr = settings.crop;
        let map = |x: f32, y: f32| -> (f32, f32) {
            let x = if cr.flip_h { 1.0 - x } else { x };
            match cr.rot90 % 4 {
                0 => (x, y),
                1 => (y, 1.0 - x),
                2 => (1.0 - x, 1.0 - y),
                _ => (1.0 - y, x),
            }
        };
        let [rx, ry, rw, rh] = region;
        let (a, b) = (map(rx, ry), map(rx + rw, ry + rh));
        let (x0, x1) = (a.0.min(b.0).clamp(0.0, 1.0), a.0.max(b.0).clamp(0.0, 1.0));
        let (y0, y1) = (a.1.min(b.1).clamp(0.0, 1.0), a.1.max(b.1).clamp(0.0, 1.0));
        let (bw, bh) = (base.width as f32, base.height as f32);
        let (px0, py0) = ((x0 * bw) as u32, (y0 * bh) as u32);
        let pw = (((x1 - x0) * bw).ceil() as u32).clamp(1, base.width - px0.min(base.width - 1));
        let ph = (((y1 - y0) * bh).ceil() as u32).clamp(1, base.height - py0.min(base.height - 1));
        let mut piece = grade_core::Photo { width: pw, height: ph, data: Vec::with_capacity((pw * ph) as usize), from_raw: base.from_raw };
        for y in py0..py0 + ph {
            let row = (y * base.width + px0) as usize;
            piece.data.extend_from_slice(&base.data[row..row + pw as usize]);
        }
        if !before {
            let grade = grade_core::grade_with(&e.analysis, &settings).map_err(|e| e.to_string())?;
            let seg = if masks::active(&settings.masks) { e.segments(&settings.masks)? } else { None };
            let view = masks::View { full_w: bw, full_h: bh, x0: px0 as f32, y0: py0 as f32, step: 1.0 };
            let graded = masks::apply(&grade.apply(&piece), &settings.masks, seg.as_deref(), &view);
            piece = grade_core::detail::apply(&graded, &settings.detail, 1.0);
        }
        let mut piece = geometry::turn(&piece, cr.rot90, cr.flip_h);
        if !before && settings.detail.framed() {
            // vignette / grain measured on the whole crop frame, shifted to this piece
            let (tw, th) = if cr.rot90 % 2 == 1 { (base.height as f32, base.width as f32) } else { (base.width as f32, base.height as f32) };
            let mut f = grade_core::detail::Frame::of_crop(&cr, tw, th);
            let (ox, oy) = (rx.clamp(0.0, 1.0) * tw, ry.clamp(0.0, 1.0) * th);
            f.cx -= ox;
            f.cy -= oy;
            f.px -= ox;
            f.py -= oy;
            piece = grade_core::detail::finish_frame(&piece, &settings.detail, &f, 1.0);
        }
        if piece.width.max(piece.height) > max_px {
            piece = shrink(&piece, max_px);
        }
        jpeg_url(&piece, 90)
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

/// Save the graded photo as a 16-bit TIFF next to the original and open it in Photoshop.
#[tauri::command]
async fn open_in_photoshop(path: String, settings: Settings) -> Result<String, String> {
    blocking(move || {
        let input = PathBuf::from(&path);
        let dir = input.parent().map(|d| d.join("JoshGrade for Photoshop")).ok_or("bad path")?;
        let dest = grade_core::unique_output(&input, &dir, OutputFormat::Tiff);
        let r = grade_core::save_graded(&input, &dest, &settings, OutputFormat::Tiff, 95, false).map_err(|e| e.to_string())?;
        let out = r.output.to_string_lossy().into_owned();
        #[cfg(target_os = "macos")]
        {
            let by_id = std::process::Command::new("open").args(["-b", "com.adobe.Photoshop"]).arg(&out).status().map(|s| s.success()).unwrap_or(false);
            let ok = by_id
                || ["Adobe Photoshop 2026", "Adobe Photoshop 2025", "Adobe Photoshop 2024", "Adobe Photoshop"]
                    .iter()
                    .any(|app| std::process::Command::new("open").args(["-a", app]).arg(&out).status().map(|s| s.success()).unwrap_or(false));
            if !ok {
                return Err(format!("Saved {} but couldn't find Photoshop on this Mac.", r.output.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()));
            }
        }
        #[cfg(target_os = "windows")]
        {
            let _ = std::process::Command::new("cmd").args(["/C", "start", "", "photoshop.exe"]).arg(&out).status();
        }
        Ok(out)
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

/// A red see-through picture of where one mask is, the same size as the preview.
#[tauri::command]
async fn mask_overlay(state: St<'_>, path: String, settings: Settings, index: usize) -> Result<String, String> {
    let st = Arc::clone(&state);
    blocking(move || {
        let e = cached(&st, &path)?;
        let m = settings.masks.get(index).ok_or("no such mask")?;
        let seg = e.segments(std::slice::from_ref(m))?;
        let v = masks::values(&e.photo, m, seg.as_deref(), &masks::View::whole(&e.photo));
        let p = Photo { width: e.photo.width, height: e.photo.height, data: v.iter().map(|&a| [a, a, a]).collect(), from_raw: false };
        let p = geometry::turn(&p, settings.crop.rot90, settings.crop.flip_h);
        let img = image::RgbaImage::from_fn(p.width, p.height, |x, y| {
            let a = p.data[(y * p.width + x) as usize][0];
            image::Rgba([255, 45, 70, (a * 140.0) as u8])
        });
        png_url(&img)
    })
    .await
}

#[derive(Serialize)]
struct RetouchModel {
    ready: bool,
    size_mb: u32,
}

#[tauri::command]
fn retouch_model() -> RetouchModel {
    RetouchModel { ready: retouch::model_ready(), size_mb: retouch::PARSER_MB }
}

/// Are the person / sky models here? (about 41 MB, downloaded once)
#[tauri::command]
fn mask_model() -> RetouchModel {
    RetouchModel { ready: masks::models_ready(), size_mb: 41 }
}

#[derive(Serialize, Clone)]
struct DlProgress {
    got: u64,
    total: u64,
}

fn download(app: &tauri::AppHandle, url: &str, dest: &Path, mb: u32, base: u64, all: u64) -> Result<(), String> {
    use std::io::{Read, Write};
    if dest.exists() {
        return Ok(());
    }
    let dir = dest.parent().unwrap().to_path_buf();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let resp = ureq::get(url).call().map_err(|e| format!("Couldn't download ({e}). Check your internet connection and try again."))?;
    let total: u64 = resp.header("Content-Length").and_then(|v| v.parse().ok()).unwrap_or(mb as u64 * 1_000_000);
    let part = dest.with_extension("part");
    let mut f = std::fs::File::create(&part).map_err(|e| e.to_string())?;
    let mut r = resp.into_reader();
    let mut buf = vec![0u8; 1 << 16];
    let (mut got, mut last) = (0u64, 0u64);
    loop {
        let n = r.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        f.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        got += n as u64;
        if got - last > 500_000 {
            last = got;
            let _ = app.emit("retouch-download", DlProgress { got: base + got, total: all });
        }
    }
    drop(f);
    if got < total * 9 / 10 {
        let _ = std::fs::remove_file(&part);
        return Err("The download was cut off. Try again.".into());
    }
    std::fs::rename(&part, dest).map_err(|e| e.to_string())?;
    let _ = app.emit("retouch-download", DlProgress { got: base + got, total: all });
    Ok(())
}

/// Download the face map model once (about 53 MB). Retouch works offline after that.
#[tauri::command]
async fn retouch_download(app: tauri::AppHandle) -> Result<(), String> {
    blocking(move || download(&app, retouch::PARSER_URL, &retouch::parser_path(), retouch::PARSER_MB, 0, retouch::PARSER_MB as u64 * 1_000_000)).await
}

/// Download the person + sky models once (about 41 MB). Masks work offline after that.
#[tauri::command]
async fn mask_download(app: tauri::AppHandle) -> Result<(), String> {
    blocking(move || {
        let d = retouch::model_dir();
        download(&app, masks::SUBJECT_URL, &d.join(masks::SUBJECT_FILE), 26, 0, 41_000_000)?;
        download(&app, masks::SKY_URL, &d.join(masks::SKY_FILE), 15, 26_000_000, 41_000_000)
    })
    .await
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            use tauri::Manager;
            if let Ok(d) = app.path().app_data_dir() {
                retouch::set_model_dir(d.join("face-models"));
            }
            Ok(())
        })
        .manage(Arc::new(AppState::default()))
        .manage(Arc::new(video::VideoState::default()))
        .manage(Arc::new(jobs::Jobs::default()))
        .invoke_handler(tauri::generate_handler![
            list_looks,
            collect_photos,
            thumbnail,
            preview,
            look_thumbs,
            export_lut,
            save_photo,
            save_batch,
            cancel_batch,
            retouch_model,
            preview_region,
            open_in_photoshop,
            retouch_download,
            mask_model,
            mask_download,
            mask_overlay,
            prefetch,
            jobs::jobs_list,
            jobs::job_cancel,
            jobs::jobs_clear,
            jobs::job_sync_render,
            jobs::job_caption_video,
            jobs::job_edit_export,
            jobs::edit_find_pauses,
            jobs::edit_graded_frame,
            video::media_collect,
            video::media_cancel,
            video::sync_run,
            video::sync_render,
            video::sync_timeline,
            video::caption_models,
            video::caption_download,
            video::caption_transcribe,
            video::caption_templates,
            video::caption_cues,
            video::caption_save,
            video::caption_into_video
        ])
        .run(tauri::generate_context!())
        .expect("error while running JoshGrade");
}
