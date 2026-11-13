//! JoshGrade core: offline automatic photo correction + colour grading.
//!
//! ```text
//! load photo -> analyze (measure + apply editing rules) -> Grade (one per-pixel transform)
//!            -> apply / export .cube LUT
//! ```

pub mod adjust;
pub mod analyze;
pub mod camera;
pub mod color;
pub mod curves;
pub mod decode;
pub mod detail;
pub mod encode;
pub mod geometry;
pub mod grade;
pub mod learn;
pub mod looks;
pub mod lens;
pub mod lut;
pub mod output;
pub mod spots;
pub mod masks;
pub mod report;
pub mod retouch;
pub mod scopes;

pub use adjust::{auto_sliders, Adjust, Settings};
pub use analyze::{analyze, Analysis, AutoParams};
pub use encode::OutputFormat;
pub use grade::Grade;
pub use looks::{all_looks, find_look, Look, LookParams};
pub use retouch::RetouchSettings;

use std::path::{Path, PathBuf};

/// A photo as sRGB-encoded floats in 0..1, row-major.
#[derive(Debug, Clone)]
pub struct Photo {
    pub width: u32,
    pub height: u32,
    pub data: Vec<color::Rgb>,
    pub from_raw: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not read image: {0}")]
    Image(#[from] image::ImageError),
    #[error("file error: {0}")]
    Io(#[from] std::io::Error),
    #[error("could not read RAW file: {0}")]
    Raw(String),
    #[error("{0}")]
    Retouch(String),
    #[error("unknown look '{0}'")]
    UnknownLook(String),
    #[error("{0}")]
    Other(String),
}

/// Size used for analysis. Analysing a reduced copy is fast and makes the result
/// identical between the on-screen preview and the full-resolution export.
pub const ANALYSIS_SIZE: u32 = 1600;

#[derive(Debug, Clone, serde::Serialize)]
pub struct SaveResult {
    pub input: PathBuf,
    pub output: PathBuf,
    pub lut: Option<PathBuf>,
    pub notes: Vec<String>,
}

/// Build the grade for a photo from its analysis and settings.
pub fn grade_with(analysis: &Analysis, settings: &Settings) -> Result<Grade, Error> {
    let look = find_look(&settings.look).ok_or_else(|| Error::UnknownLook(settings.look.clone()))?;
    let auto = if settings.auto { analysis.params } else { AutoParams::identity() };
    Ok(Grade::with_adjust(&auto, &look.params, &settings.adjust, settings.strength).with_curves(&settings.curves))
}

/// White balance picker: the Temperature and Tint slider values that make the spot at
/// (x, y) (fractions of the upright photo) neutral grey, like clicking with Lightroom's
/// white balance eyedropper. Other settings stay as they are.
pub fn white_balance_at(analysis: &Analysis, settings: &Settings, photo: &Photo, x: f32, y: f32) -> Option<(f32, f32)> {
    let look = find_look(&settings.look)?;
    let auto = if settings.auto { analysis.params } else { AutoParams::identity() };
    let (w, h) = (photo.width as i64, photo.height as i64);
    let (cx, cy) = ((x * w as f32) as i64, (y * h as f32) as i64);
    let r = (w.max(h) / 300).max(2);
    let mut acc = [0f64; 3];
    let mut n = 0.0;
    for yy in (cy - r).max(0)..(cy + r + 1).min(h) {
        for xx in (cx - r).max(0)..(cx + r + 1).min(w) {
            let p = photo.data[(yy * w + xx) as usize];
            for k in 0..3 {
                acc[k] += color::to_linear(p[k]) as f64 * auto.wb_gains[k] as f64;
            }
            n += 1.0;
        }
    }
    if n == 0.0 {
        return None;
    }
    let [r_, g_, b_] = acc.map(|v| (v / n) as f32);
    if g_ <= 1e-5 || r_ + b_ <= 1e-5 || r_.max(g_).max(b_) > 0.98 {
        return None; // too dark or clipped to judge
    }
    // r(1+T) = b(1-T) = g(1-Tn)
    let t = (b_ - r_) / (b_ + r_);
    let tn = 1.0 - r_ * (1.0 + t) / g_;
    let temp = ((t - look.params.temp) * 330.0).clamp(-100.0, 100.0);
    let tint = ((tn - look.params.tint) * 250.0).clamp(-100.0, 100.0);
    Some(((temp * 10.0).round() / 10.0, (tint * 10.0).round() / 10.0))
}

/// Analyse a photo (from a reduced copy) and build its grade.
pub fn grade_for(photo_small: &Photo, settings: &Settings) -> Result<(Analysis, Grade), Error> {
    let analysis = analyze(photo_small);
    let grade = grade_with(&analysis, settings)?;
    Ok((analysis, grade))
}

/// `name_graded.ext` in `dir`, adding _2, _3… so nothing is ever overwritten.
pub fn unique_output(input: &Path, dir: &Path, format: OutputFormat) -> PathBuf {
    let stem = input.file_stem().and_then(|s| s.to_str()).unwrap_or("photo");
    let mut output = dir.join(format!("{}_graded.{}", stem, format.extension()));
    let mut n = 2;
    while output.exists() {
        output = dir.join(format!("{}_graded_{}.{}", stem, n, format.extension()));
        n += 1;
    }
    output
}

/// Grade one file at full resolution and save it exactly at `output`.
pub fn save_graded(input: &Path, output: &Path, settings: &Settings, format: OutputFormat, quality: u8, write_lut: bool) -> Result<SaveResult, Error> {
    let o = output::ExportOptions { quality, ..Default::default() };
    save_graded_with(input, output, settings, format, &o, write_lut)
}

/// Like [`save_graded`], with export options (resize, output sharpening, watermark, quality).
pub fn save_graded_with(input: &Path, output: &Path, settings: &Settings, format: OutputFormat, opts: &output::ExportOptions, write_lut: bool) -> Result<SaveResult, Error> {
    let quality = opts.quality.clamp(1, 100);
    // Pixels are always saved upright (EXIF is copied with its orientation reset).
    let (mut img, orientation) = decode::load_dynamic(input)?;
    img.apply_orientation(orientation);
    let raw = decode::is_raw(input);
    // Analyse the same reduced copy the app previews, so the saved file matches.
    let small = decode::to_photo(img.clone(), Some(ANALYSIS_SIZE), raw);
    let (analysis, grade) = grade_for(&small, settings)?;
    let mut full = decode::to_photo(img, None, raw);
    if settings.retouch.active() {
        let faces = retouch::find_faces(&small).map_err(|e| Error::Retouch(e.to_string()))?;
        if !faces.is_empty() {
            full = retouch::apply(&full, &faces, &settings.retouch);
        }
    }
    // lens corrections / transform (before grading, like a RAW developer)
    if settings.lens.active() {
        full = lens::apply(&full, &settings.lens);
    }
    // spot removal / red eye (positions are on the corrected photo)
    if !settings.spots.is_empty() {
        full = spots::apply_whole(&full, &settings.spots);
    }
    let seg = if masks::needs_ai(&settings.masks) && masks::active(&settings.masks) {
        let s = if settings.lens.geometric() { lens::apply(&small, &settings.lens) } else { small.clone() };
        Some(masks::segment(&s).map_err(Error::Retouch)?)
    } else {
        None
    };
    drop(small);
    let graded = grade.apply_exact(&full);
    let graded = masks::apply(&graded, &settings.masks, seg.as_ref(), &masks::View::whole(&graded));
    let graded = finish(&graded, settings);
    drop(full);
    let graded = if opts.changes_pixels() { output::apply(graded, opts) } else { graded };

    if let Some(dir) = output.parent() {
        std::fs::create_dir_all(dir)?;
    }
    encode::save(&graded, output, format, quality, Some(input))?;

    let lut = if write_lut {
        let p = output.with_extension("cube");
        let stem = input.file_stem().and_then(|s| s.to_str()).unwrap_or("photo");
        std::fs::write(&p, lut::cube_string(&grade, 33, &format!("{} - {}", stem, settings.look)))?;
        Some(p)
    } else {
        None
    };
    Ok(SaveResult { input: input.to_path_buf(), output: output.to_path_buf(), lut, notes: analysis.notes })
}

/// Everything after the colour grade, for a full-resolution save: detail (sharpening,
/// noise reduction, clarity…), then the crop, then vignette and grain on the final frame.
pub fn finish(p: &Photo, settings: &Settings) -> Photo {
    let d = &settings.detail;
    let p = detail::apply(p, d, 1.0);
    let p = geometry::apply(&p, &settings.crop);
    detail::finish_frame(&p, d, &detail::Frame::whole(), 1.0)
}

/// Collect supported photos from files and folders (folders are scanned up to 3 levels deep).
/// Returns (photos, unsupported files that were skipped).
pub fn collect_inputs(paths: &[PathBuf]) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut out = Vec::new();
    let mut skipped = Vec::new();
    for p in paths {
        if p.is_dir() {
            walk(p, 0, &mut out, &mut skipped);
        } else if decode::is_supported(p) {
            out.push(p.clone());
        } else if p.is_file() {
            skipped.push(p.clone());
        }
    }
    (out, skipped)
}

fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>, skipped: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| !is_hidden(p)).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            // Skip our own output folders so re-adding a folder doesn't import graded copies.
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if depth < 3 && name != "Graded" && !name.ends_with(".app") {
                walk(&p, depth + 1, out, skipped);
            }
        } else if decode::is_supported(&p) {
            out.push(p);
        } else if !is_sidecar(&p) {
            skipped.push(p);
        }
    }
}

fn is_hidden(p: &Path) -> bool {
    p.file_name().and_then(|n| n.to_str()).map(|n| n.starts_with('.')).unwrap_or(false)
}

/// Files that normally sit next to photos and aren't worth warning about.
fn is_sidecar(p: &Path) -> bool {
    let e = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    matches!(e.as_str(), "xmp" | "thm" | "db" | "ini" | "txt" | "json" | "cube" | "lrv" | "aae")
}

#[cfg(test)]
mod wb_tests {
    use super::*;
    #[test]
    fn picker_makes_the_spot_grey() {
        // a warm-tinted grey card
        let c = [0.55f32, 0.5, 0.46];
        let photo = Photo { width: 64, height: 64, data: vec![c; 64 * 64], from_raw: false };
        let analysis = analyze(&photo);
        let mut s = Settings { auto: false, ..Default::default() };
        let (t, n) = white_balance_at(&analysis, &s, &photo, 0.5, 0.5).unwrap();
        s.adjust.temp = t;
        s.adjust.tint = n;
        let g = grade_with(&analysis, &s).unwrap();
        let o = g.apply_px(c);
        assert!((o[0] - o[1]).abs() < 0.02 && (o[2] - o[1]).abs() < 0.02, "{o:?} temp {t} tint {n}");
    }
}
