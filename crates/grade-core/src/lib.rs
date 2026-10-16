//! JoshGrade core: offline automatic photo correction + colour grading.
//!
//! ```text
//! load photo -> analyze (measure + apply editing rules) -> Grade (one per-pixel transform)
//!            -> apply / export .cube LUT
//! ```

pub mod adjust;
pub mod analyze;
pub mod color;
pub mod decode;
pub mod encode;
pub mod grade;
pub mod looks;
pub mod lut;
pub mod report;
pub mod scopes;

pub use adjust::{auto_sliders, Adjust, Settings};
pub use analyze::{analyze, Analysis, AutoParams};
pub use encode::OutputFormat;
pub use grade::Grade;
pub use looks::{all_looks, find_look, Look, LookParams};

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
    Ok(Grade::with_adjust(&auto, &look.params, &settings.adjust, settings.strength))
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
    // When the output is a JPEG made from a JPEG we copy the EXIF (incl. orientation),
    // so keep pixels un-rotated; otherwise bake the rotation into the pixels.
    let keep_exif = format == OutputFormat::Jpeg && decode::is_jpeg(input);
    let (mut img, orientation) = decode::load_dynamic(input)?;
    if !keep_exif {
        img.apply_orientation(orientation);
    }
    let raw = decode::is_raw(input);
    // Analyse the same upright, reduced copy the app previews, so the saved file matches.
    let mut upright = img.clone();
    if keep_exif {
        upright.apply_orientation(orientation);
    }
    let small = decode::to_photo(upright, Some(ANALYSIS_SIZE), raw);
    let (analysis, grade) = grade_for(&small, settings)?;
    drop(small);
    let full = decode::to_photo(img, None, raw);
    let graded = grade.apply(&full);
    drop(full);

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
