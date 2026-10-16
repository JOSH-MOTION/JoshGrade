//! Saving graded photos. JPEG output keeps the original EXIF (camera, lens, date,
//! orientation) and colour profile when the source was a JPEG.

use crate::{Error, Photo};
use image::codecs::jpeg::JpegEncoder;
use image::{ExtendedColorType, ImageBuffer, Rgb};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum OutputFormat {
    Jpeg,
    Tiff,
    Png,
}

impl OutputFormat {
    pub fn extension(&self) -> &'static str {
        match self {
            OutputFormat::Jpeg => "jpg",
            OutputFormat::Tiff => "tif",
            OutputFormat::Png => "png",
        }
    }
}

fn to_u8(photo: &Photo) -> Vec<u8> {
    // Tiny ordered dither avoids banding in smooth skies when going float -> 8-bit.
    const BAYER: [f32; 4] = [0.0, 0.5, 0.75, 0.25];
    let w = photo.width as usize;
    photo
        .data
        .iter()
        .enumerate()
        .flat_map(|(i, p)| {
            let (x, y) = (i % w, i / w);
            let t = BAYER[(x & 1) + 2 * (y & 1)] - 0.375;
            p.map(|v| (v * 255.0 + 0.5 + t).clamp(0.0, 255.0) as u8)
        })
        .collect()
}

pub fn jpeg_bytes(photo: &Photo, quality: u8) -> Result<Vec<u8>, Error> {
    let mut buf = Vec::new();
    let enc = JpegEncoder::new_with_quality(&mut buf, quality);
    enc.write_image_compat(&to_u8(photo), photo.width, photo.height)?;
    Ok(buf)
}

trait WriteCompat {
    fn write_image_compat(self, data: &[u8], w: u32, h: u32) -> Result<(), image::ImageError>;
}
impl<W: std::io::Write> WriteCompat for JpegEncoder<W> {
    fn write_image_compat(mut self, data: &[u8], w: u32, h: u32) -> Result<(), image::ImageError> {
        self.encode(data, w, h, ExtendedColorType::Rgb8)
    }
}

/// Save `photo` to `out`. If `metadata_from` is a JPEG and the output is JPEG,
/// EXIF + ICC profile are copied over.
pub fn save(photo: &Photo, out: &Path, format: OutputFormat, quality: u8, metadata_from: Option<&Path>) -> Result<(), Error> {
    match format {
        OutputFormat::Jpeg => {
            let mut bytes = jpeg_bytes(photo, quality)?;
            if let Some(src) = metadata_from.filter(|p| crate::decode::is_jpeg(p)) {
                if let Ok(b) = copy_jpeg_metadata(src, &bytes) {
                    bytes = b;
                }
            }
            std::fs::write(out, bytes)?;
        }
        OutputFormat::Tiff | OutputFormat::Png => {
            let data: Vec<u16> = photo
                .data
                .iter()
                .flat_map(|p| p.map(|v| (v * 65535.0 + 0.5).clamp(0.0, 65535.0) as u16))
                .collect();
            let img: ImageBuffer<Rgb<u16>, Vec<u16>> = ImageBuffer::from_raw(photo.width, photo.height, data)
                .ok_or_else(|| Error::Other("buffer size mismatch".into()))?;
            img.save(out)?;
        }
    }
    Ok(())
}

fn copy_jpeg_metadata(src: &Path, encoded: &[u8]) -> Result<Vec<u8>, Error> {
    use img_parts::jpeg::Jpeg;
    use img_parts::{Bytes, ImageEXIF, ImageICC};
    let src = Jpeg::from_bytes(Bytes::from(std::fs::read(src)?)).map_err(|e| Error::Other(e.to_string()))?;
    let mut out = Jpeg::from_bytes(Bytes::copy_from_slice(encoded)).map_err(|e| Error::Other(e.to_string()))?;
    out.set_exif(src.exif());
    out.set_icc_profile(src.icc_profile());
    let mut v = Vec::new();
    out.encoder().write_to(&mut v)?;
    Ok(v)
}
