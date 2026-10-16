//! JoshGrade core: offline automatic photo correction + colour grading.
//!
//! ```text
//! load photo -> analyze (measure + apply editing rules) -> Grade (one per-pixel transform)
//!            -> apply / export .cube LUT
//! ```

pub mod analyze;
pub mod color;
pub mod decode;
pub mod encode;
pub mod grade;
pub mod looks;
pub mod lut;

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

#[derive(Debug, Clone)]
pub struct Job {
    pub look: String,
    pub strength: f32,
    pub format: OutputFormat,
    pub quality: u8,
    /// Also write a .cube LUT next to each graded photo.
    pub write_lut: bool,
}

impl Default for Job {
    fn default() -> Self {
        Job { look: "natural".into(), strength: 1.0, format: OutputFormat::Jpeg, quality: 95, write_lut: false }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct JobResult {
    pub input: PathBuf,
    pub output: PathBuf,
    pub lut: Option<PathBuf>,
    pub notes: Vec<String>,
}

/// Analyse a photo (from a reduced copy) and build its grade.
pub fn grade_for(photo_small: &Photo, look: &str, strength: f32) -> Result<(Analysis, Grade), Error> {
    let look = find_look(look).ok_or_else(|| Error::UnknownLook(look.into()))?;
    let analysis = analyze(photo_small);
    let grade = Grade::new(&analysis.params, &look.params, strength);
    Ok((analysis, grade))
}

/// Grade one file at full resolution and write it into `out_dir`.
pub fn process_file(input: &Path, out_dir: &Path, job: &Job) -> Result<JobResult, Error> {
    // When the output is a JPEG made from a JPEG we copy the EXIF (incl. orientation),
    // so keep pixels un-rotated; otherwise bake the rotation into the pixels.
    let keep_exif = job.format == OutputFormat::Jpeg && decode::is_jpeg(input);
    let (mut img, orientation) = decode::load_dynamic(input)?;
    if !keep_exif {
        img.apply_orientation(orientation);
    }
    let raw = decode::is_raw(input);
    let small = decode::to_photo(img.clone(), Some(ANALYSIS_SIZE), raw);
    let (analysis, grade) = grade_for(&small, &job.look, job.strength)?;
    drop(small);
    let full = decode::to_photo(img, None, raw);
    let graded = grade.apply(&full);
    drop(full);

    std::fs::create_dir_all(out_dir)?;
    let stem = input.file_stem().and_then(|s| s.to_str()).unwrap_or("photo");
    let mut output = out_dir.join(format!("{}_graded.{}", stem, job.format.extension()));
    let mut n = 2;
    while output.exists() {
        output = out_dir.join(format!("{}_graded_{}.{}", stem, n, job.format.extension()));
        n += 1;
    }
    encode::save(&graded, &output, job.format, job.quality, Some(input))?;

    let lut = if job.write_lut {
        let p = output.with_extension("cube");
        std::fs::write(&p, lut::cube_string(&grade, 33, &format!("{} - {}", stem, job.look)))?;
        Some(p)
    } else {
        None
    };
    Ok(JobResult { input: input.to_path_buf(), output, lut, notes: analysis.notes })
}

/// Collect supported photos from files and folders (folders are scanned one level deep).
pub fn collect_inputs(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for p in paths {
        if p.is_dir() {
            if let Ok(rd) = std::fs::read_dir(p) {
                let mut v: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path()))
                    .filter(|f| f.is_file() && decode::is_supported(f) && !is_hidden(f))
                    .collect();
                v.sort();
                out.extend(v);
            }
        } else if decode::is_supported(p) {
            out.push(p.clone());
        }
    }
    out
}

fn is_hidden(p: &Path) -> bool {
    p.file_name().and_then(|n| n.to_str()).map(|n| n.starts_with('.')).unwrap_or(false)
}
