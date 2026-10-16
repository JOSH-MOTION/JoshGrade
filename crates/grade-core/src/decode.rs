//! Loading JPEG/PNG/TIFF/WebP and camera RAW files into a float sRGB buffer.

use crate::color::{to_display, Rgb};
use crate::{Error, Photo};
use image::imageops::FilterType;
use image::metadata::Orientation;
use image::{DynamicImage, ImageDecoder, ImageReader, Rgb32FImage};
use std::path::Path;

/// Camera RAW extensions we hand to the RAW decoder (rawler supports Canon CR2/CR3,
/// Nikon, Sony, Fuji, Panasonic, Olympus, Pentax, Leica, DNG and more).
pub const RAW_EXTENSIONS: &[&str] = &[
    "cr2", "cr3", "crw", "nef", "nrw", "arw", "srf", "sr2", "raf", "rw2", "rwl", "orf", "pef",
    "dng", "srw", "3fr", "fff", "iiq", "mos", "erf", "kdc", "dcr", "mrw", "x3f",
];
pub const RASTER_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "tif", "tiff", "webp"];

fn ext_of(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

pub fn is_raw(path: &Path) -> bool {
    RAW_EXTENSIONS.contains(&ext_of(path).as_str())
}

pub fn is_jpeg(path: &Path) -> bool {
    matches!(ext_of(path).as_str(), "jpg" | "jpeg")
}

pub fn is_supported(path: &Path) -> bool {
    let e = ext_of(path);
    RAW_EXTENSIONS.contains(&e.as_str()) || RASTER_EXTENSIONS.contains(&e.as_str())
}

/// Load a photo.
/// * `max_dim` – downscale so the longest side is at most this (for fast previews).
/// * `apply_orientation` – rotate pixels per EXIF. Leave false when the original EXIF
///   (which still carries the orientation flag) will be copied into the output.
pub fn load(path: &Path, max_dim: Option<u32>, apply_orientation: bool) -> Result<Photo, Error> {
    let (mut img, orientation) = load_dynamic(path)?;
    if apply_orientation {
        img.apply_orientation(orientation);
    }
    Ok(to_photo(img, max_dim, is_raw(path)))
}

/// Decode to an `image::DynamicImage` plus its EXIF orientation (not yet applied).
pub fn load_dynamic(path: &Path) -> Result<(DynamicImage, Orientation), Error> {
    if is_raw(path) {
        load_raw(path)
    } else {
        load_raster(path)
    }
}

/// Convert (and optionally downscale) a decoded image into a float `Photo`.
pub fn to_photo(img: DynamicImage, max_dim: Option<u32>, from_raw: bool) -> Photo {
    let mut img = img;
    if let Some(m) = max_dim {
        if img.width().max(img.height()) > m {
            img = img.resize(m, m, FilterType::Triangle);
        }
    }
    let buf = match img {
        DynamicImage::ImageRgb32F(b) => b,
        other => other.to_rgb32f(),
    };
    let (w, h) = buf.dimensions();
    let data: Vec<Rgb> = buf
        .into_raw()
        .chunks_exact(3)
        .map(|c| [c[0].clamp(0.0, 1.0), c[1].clamp(0.0, 1.0), c[2].clamp(0.0, 1.0)])
        .collect();
    Photo { width: w, height: h, data, from_raw }
}

/// Fast small preview for file lists. RAW files use the camera's embedded JPEG preview
/// when there is one (much faster than developing the RAW).
pub fn thumbnail(path: &Path, size: u32) -> Result<Photo, Error> {
    if is_raw(path) {
        if let Some(img) = raw_embedded_preview(path) {
            return Ok(to_photo(img, Some(size), true));
        }
    }
    load(path, Some(size), true)
}

fn raw_embedded_preview(path: &Path) -> Option<DynamicImage> {
    use rawler::decoders::RawDecodeParams;
    use rawler::rawsource::RawSource;
    let src = RawSource::new(path).ok()?;
    let decoder = rawler::get_decoder(&src).ok()?;
    let params = RawDecodeParams::default();
    let img = decoder
        .thumbnail_image(&src, &params)
        .ok()
        .flatten()
        .or_else(|| decoder.preview_image(&src, &params).ok().flatten())?;
    let orientation = decoder
        .raw_metadata(&src, &params)
        .ok()
        .and_then(|md| md.exif.orientation)
        .and_then(|o| Orientation::from_exif(o as u8))
        .unwrap_or(Orientation::NoTransforms);
    let mut img = img;
    img.apply_orientation(orientation);
    Some(img)
}

fn load_raster(path: &Path) -> Result<(DynamicImage, Orientation), Error> {
    let reader = ImageReader::open(path)?.with_guessed_format()?;
    let mut decoder = reader.into_decoder()?;
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let img = DynamicImage::from_decoder(decoder)?;
    Ok((img, orientation))
}

fn load_raw(path: &Path) -> Result<(DynamicImage, Orientation), Error> {
    use rawler::decoders::RawDecodeParams;
    use rawler::imgop::develop::{Intermediate, ProcessingStep, RawDevelop};
    use rawler::rawsource::RawSource;

    let raw_err = |e: rawler::RawlerError| Error::Raw(e.to_string());

    let rawimage = rawler::decode_file(path).map_err(raw_err)?;

    // Orientation lives in the metadata, not the RawImage.
    let orientation = (|| {
        let src = RawSource::new(path).ok()?;
        let decoder = rawler::get_decoder(&src).ok()?;
        let md = decoder.raw_metadata(&src, &RawDecodeParams::default()).ok()?;
        md.exif.orientation.and_then(|o| Orientation::from_exif(o as u8))
    })()
    .unwrap_or(Orientation::NoTransforms);

    // Develop to *linear* sRGB (we skip rawler's gamma step and encode ourselves,
    // keeping full float precision and highlight headroom).
    let develop = RawDevelop::new_with(&[
        ProcessingStep::Rescale,
        ProcessingStep::Demosaic,
        ProcessingStep::FujiRotate,
        ProcessingStep::CropActiveArea,
        ProcessingStep::WhiteBalance,
        ProcessingStep::Calibrate,
        ProcessingStep::CropDefault,
    ]);
    let inter = develop.develop_intermediate(&rawimage).map_err(raw_err)?;
    let (w, h, flat): (usize, usize, Vec<f32>) = match inter {
        Intermediate::ThreeColor(px) => (
            px.width,
            px.height,
            px.data.iter().flat_map(|p| p.iter().map(|v| to_display(*v))).collect(),
        ),
        Intermediate::Monochrome(px) => (
            px.width,
            px.height,
            px.data.iter().flat_map(|v| {
                let d = to_display(*v);
                [d, d, d]
            }).collect(),
        ),
        Intermediate::FourColor(px) => (
            px.width,
            px.height,
            px.data.iter().flat_map(|p| [to_display(p[0]), to_display(p[1]), to_display(p[2])]).collect(),
        ),
    };
    let buf = Rgb32FImage::from_raw(w as u32, h as u32, flat)
        .ok_or_else(|| Error::Raw("RAW buffer size mismatch".into()))?;
    Ok((DynamicImage::ImageRgb32F(buf), orientation))
}
