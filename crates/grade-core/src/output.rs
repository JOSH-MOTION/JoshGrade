//! Export options, like Lightroom's Export dialog: resize, output sharpening, file naming
//! and a watermark. Applied to the finished picture just before it is written.

use crate::color::luma;
use crate::retouch::{blur, Plane};
use crate::Photo;
use ab_glyph::{Font, FontRef, PxScale, ScaleFont};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

static FONT: &[u8] = include_bytes!("../fonts/Inter-SemiBold.otf");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Sharpen {
    #[default]
    None,
    Screen,
    Matte,
    Glossy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SharpenAmount {
    Low,
    #[default]
    Standard,
    High,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Watermark {
    pub text: String,
    /// height of the text as a percentage of the picture's shorter side
    pub size: f32,
    /// 0..100
    pub opacity: f32,
    /// "tl" "tc" "tr" "ml" "mc" "mr" "bl" "bc" "br"
    pub position: String,
    /// true = dark text (for light photos)
    pub dark: bool,
}

impl Default for Watermark {
    fn default() -> Self {
        Watermark { text: String::new(), size: 3.0, opacity: 70.0, position: "br".into(), dark: false }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ExportOptions {
    /// Longest side in pixels; 0 keeps the full size. Never enlarges.
    pub long_edge: u32,
    /// JPEG quality 1..100
    pub quality: u8,
    pub sharpen: Sharpen,
    pub sharpen_amount: SharpenAmount,
    /// File name template: {name} original name, {n} number in the batch, {date} today
    /// (YYYY-MM-DD), {look}. Empty = "{name}_graded".
    pub rename: String,
    pub watermark: Watermark,
}

impl Default for ExportOptions {
    fn default() -> Self {
        ExportOptions { long_edge: 0, quality: 92, sharpen: Sharpen::None, sharpen_amount: SharpenAmount::Standard, rename: String::new(), watermark: Watermark::default() }
    }
}

impl ExportOptions {
    pub fn changes_pixels(&self) -> bool {
        self.long_edge > 0 || self.sharpen != Sharpen::None || !self.watermark.text.trim().is_empty()
    }
}

/// Apply resize, output sharpening and the watermark, in that order (like Lightroom).
pub fn apply(p: Photo, o: &ExportOptions) -> Photo {
    let p = if o.long_edge > 0 { resize_long(&p, o.long_edge) } else { p };
    let p = if o.sharpen != Sharpen::None { output_sharpen(&p, o.sharpen, o.sharpen_amount) } else { p };
    if o.watermark.text.trim().is_empty() {
        p
    } else {
        watermark(p, &o.watermark)
    }
}

/// High-quality downsizing (Lanczos in linear light would be nicer, but sRGB Lanczos matches
/// what Lightroom and Photoshop produce for export).
pub fn resize_long(p: &Photo, long: u32) -> Photo {
    let cur = p.width.max(p.height);
    if long == 0 || long >= cur {
        return p.clone();
    }
    let k = long as f64 / cur as f64;
    let (w, h) = (((p.width as f64 * k).round() as u32).max(1), ((p.height as f64 * k).round() as u32).max(1));
    let flat: Vec<f32> = p.data.iter().flat_map(|c| c.iter().copied()).collect();
    let img = image::Rgb32FImage::from_raw(p.width, p.height, flat).expect("size");
    let out = image::imageops::resize(&img, w, h, image::imageops::FilterType::Lanczos3);
    let data = out.into_raw().chunks_exact(3).map(|c| [c[0].clamp(0.0, 1.0), c[1].clamp(0.0, 1.0), c[2].clamp(0.0, 1.0)]).collect();
    Photo { width: w, height: h, data, from_raw: p.from_raw }
}

/// Output sharpening for where the picture is going: screens want a small crisp radius,
/// matte paper a stronger, wider one, glossy paper in between.
pub fn output_sharpen(p: &Photo, kind: Sharpen, amount: SharpenAmount) -> Photo {
    let (sigma, base) = match kind {
        Sharpen::None => return p.clone(),
        Sharpen::Screen => (0.6, 0.55),
        Sharpen::Glossy => (0.9, 0.75),
        Sharpen::Matte => (1.1, 1.0),
    };
    let amt = base
        * match amount {
            SharpenAmount::Low => 0.6,
            SharpenAmount::Standard => 1.0,
            SharpenAmount::High => 1.5,
        };
    let (w, h) = (p.width as usize, p.height as usize);
    let y = Plane { w, h, d: p.data.par_iter().map(|c| luma(*c)).collect() };
    let soft = blur(&y, sigma);
    let data = p
        .data
        .par_iter()
        .enumerate()
        .map(|(i, c)| {
            // sharpen brightness only (no colour fringes), and go easy in flat areas (noise)
            let d = y.d[i] - soft.d[i];
            let d = if d.abs() < 0.004 { d * (d.abs() / 0.004) } else { d };
            let add = (amt * d).clamp(-0.15, 0.15);
            [(c[0] + add).clamp(0.0, 1.0), (c[1] + add).clamp(0.0, 1.0), (c[2] + add).clamp(0.0, 1.0)]
        })
        .collect();
    Photo { width: p.width, height: p.height, data, from_raw: p.from_raw }
}

/// Draw a text watermark.
pub fn watermark(mut p: Photo, wm: &Watermark) -> Photo {
    let text = wm.text.trim();
    let Ok(font) = FontRef::try_from_slice(FONT) else { return p };
    let short = p.width.min(p.height) as f32;
    let px = (short * wm.size.clamp(0.5, 20.0) / 100.0).max(8.0);
    let sf = font.as_scaled(PxScale::from(px));
    // lay out one line
    let mut x = 0.0f32;
    let mut glyphs = Vec::new();
    let mut prev = None;
    for ch in text.chars() {
        let id = font.glyph_id(ch);
        if let Some(pv) = prev {
            x += sf.kern(pv, id);
        }
        glyphs.push(id.with_scale_and_position(px, ab_glyph::point(x, sf.ascent())));
        x += sf.h_advance(id);
        prev = Some(id);
    }
    let (tw, th) = (x, sf.ascent() - sf.descent());
    let margin = short * 0.03;
    let pos = wm.position.as_bytes();
    let (vy, hx) = (pos.first().copied().unwrap_or(b'b'), pos.get(1).copied().unwrap_or(b'r'));
    let ox = match hx {
        b'l' => margin,
        b'c' => (p.width as f32 - tw) / 2.0,
        _ => p.width as f32 - tw - margin,
    };
    let oy = match vy {
        b't' => margin,
        b'm' => (p.height as f32 - th) / 2.0,
        _ => p.height as f32 - th - margin,
    };
    let ink = if wm.dark { [0.05f32, 0.05, 0.05] } else { [1.0f32, 1.0, 1.0] };
    let shadow = if wm.dark { [1.0f32, 1.0, 1.0] } else { [0.0f32, 0.0, 0.0] };
    let op = (wm.opacity / 100.0).clamp(0.0, 1.0);
    let (w, h) = (p.width as i64, p.height as i64);
    let mut cov = vec![0f32; (w * h) as usize];
    for g in glyphs {
        if let Some(o) = font.outline_glyph(g) {
            let b = o.px_bounds();
            o.draw(|gx, gy, c| {
                let (xx, yy) = ((ox + b.min.x) as i64 + gx as i64, (oy + b.min.y) as i64 + gy as i64);
                if xx >= 0 && yy >= 0 && xx < w && yy < h {
                    let k = (yy * w + xx) as usize;
                    cov[k] = (cov[k] + c).min(1.0);
                }
            });
        }
    }
    // a soft shadow keeps it readable on any background
    let sh = blur(&Plane { w: w as usize, h: h as usize, d: cov.clone() }, (px * 0.06).max(1.0));
    let off = ((px * 0.04).round() as i64).max(1);
    for yy in 0..h {
        for xx in 0..w {
            let i = (yy * w + xx) as usize;
            let s = if xx >= off && yy >= off { sh.d[((yy - off) * w + xx - off) as usize] } else { 0.0 };
            let a = cov[i] * op;
            let sa = (s * op * 0.45) * (1.0 - cov[i]);
            if a <= 0.0 && sa <= 0.0 {
                continue;
            }
            let c = &mut p.data[i];
            for k in 0..3 {
                c[k] = c[k] * (1.0 - sa) + shadow[k] * sa;
                c[k] = c[k] * (1.0 - a) + ink[k] * a;
            }
        }
    }
    p
}

/// Output path for one file of a batch, from the rename template. Never overwrites.
pub fn output_path(input: &Path, dir: &Path, template: &str, n: usize, look: &str, ext: &str) -> PathBuf {
    let stem = input.file_stem().and_then(|s| s.to_str()).unwrap_or("photo");
    let t = if template.trim().is_empty() { "{name}_graded" } else { template.trim() };
    let date = today();
    let mut name = t.replace("{name}", stem).replace("{date}", &date).replace("{look}", look);
    // {n} and {n3} (zero-padded)
    for pad in (1..=5).rev() {
        name = name.replace(&format!("{{n{pad}}}"), &format!("{:0pad$}", n, pad = pad));
    }
    name = name.replace("{n}", &n.to_string());
    let name: String = name.chars().map(|c| if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') { '-' } else { c }).collect();
    let name = if name.trim().is_empty() { stem.to_string() } else { name };
    let mut out = dir.join(format!("{name}.{ext}"));
    let mut k = 2;
    while out.exists() || out == input {
        out = dir.join(format!("{name}_{k}.{ext}"));
        k += 1;
    }
    out
}

fn today() -> String {
    // days since epoch -> civil date (Howard Hinnant's algorithm), local enough for file names
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let z = secs.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    format!("{:04}-{:02}-{:02}", if m <= 2 { y + 1 } else { y }, m, d)
}

/// Focus mask (Capture One): where the picture is in sharp focus. Returns 0..1 per pixel.
pub fn focus_mask(p: &Photo) -> Vec<f32> {
    let (w, h) = (p.width as usize, p.height as usize);
    let y = Plane { w, h, d: p.data.par_iter().map(|c| luma(*c)).collect() };
    // a touch of blur so noise doesn't light up, then local contrast (gradient)
    let s = blur(&y, 0.7);
    let g: Vec<f32> = (0..w * h)
        .into_par_iter()
        .map(|i| {
            let (x, yy) = (i % w, i / w);
            let dx = s.d[yy * w + (x + 1).min(w - 1)] - s.d[yy * w + x.saturating_sub(1)];
            let dy = s.d[(yy + 1).min(h - 1) * w + x] - s.d[yy.saturating_sub(1) * w + x];
            (dx * dx + dy * dy).sqrt()
        })
        .collect();
    // threshold relative to the sharpest edges in the frame (and never below a floor)
    let mut sorted: Vec<f32> = g.iter().step_by(7).copied().collect();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p995 = sorted.get(sorted.len() * 995 / 1000).copied().unwrap_or(0.0);
    let t = (p995 * 0.5).max(0.06);
    let m = Plane { w, h, d: g.par_iter().map(|&v| ((v - t) / (t * 0.4)).clamp(0.0, 1.0)).collect() };
    // spread a little so thin edges read clearly
    let m = blur(&m, 1.2);
    m.d.par_iter().map(|&v| (v * 2.5).min(1.0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resize_and_watermark() {
        let p = Photo { width: 400, height: 300, data: vec![[0.3, 0.3, 0.3]; 400 * 300], from_raw: false };
        let r = resize_long(&p, 200);
        assert_eq!((r.width, r.height), (200, 150));
        let wm = Watermark { text: "© Josh".into(), size: 10.0, opacity: 100.0, position: "br".into(), dark: false };
        let o = watermark(r, &wm);
        let bright = o.data.iter().filter(|c| c[0] > 0.9).count();
        assert!(bright > 40, "text drawn: {bright}");
        // bottom-right quadrant only
        let tl = (0..75).flat_map(|y| (0..100).map(move |x| (x, y))).filter(|&(x, y)| o.data[y * 200 + x][0] > 0.9).count();
        assert_eq!(tl, 0);
    }
    #[test]
    fn names() {
        let d = std::env::temp_dir().join("jg_names_test");
        let _ = std::fs::create_dir_all(&d);
        let p = output_path(Path::new("/x/IMG_1.CR3"), &d, "Wedding_{n3}", 7, "natural", "jpg");
        assert_eq!(p.file_name().unwrap().to_str().unwrap(), "Wedding_007.jpg");
        let p = output_path(Path::new("/x/IMG_1.CR3"), &d, "", 1, "natural", "jpg");
        assert_eq!(p.file_name().unwrap().to_str().unwrap(), "IMG_1_graded.jpg");
    }
    #[test]
    fn focus_finds_the_sharp_half() {
        // left half: sharp checkerboard, right half: blurred version
        let (w, h) = (200usize, 100usize);
        let mut data = vec![[0.5f32; 3]; w * h];
        for y in 0..h {
            for x in 0..w {
                let v = if ((x / 4) + (y / 4)) % 2 == 0 { 0.2 } else { 0.8 };
                data[y * w + x] = [v; 3];
            }
        }
        let p = Photo { width: w as u32, height: h as u32, data, from_raw: false };
        let y = Plane { w, h, d: p.data.iter().map(|c| c[0]).collect() };
        let b = blur(&y, 3.0);
        let mut q = p.clone();
        for yy in 0..h {
            for x in w / 2..w {
                let v = b.d[yy * w + x];
                q.data[yy * w + x] = [v; 3];
            }
        }
        let m = focus_mask(&q);
        let left: f32 = (0..h).flat_map(|y| (10..90).map(move |x| (x, y))).map(|(x, y)| m[y * w + x]).sum();
        let right: f32 = (0..h).flat_map(|y| (110..190).map(move |x| (x, y))).map(|(x, y)| m[y * w + x]).sum();
        assert!(left > right * 4.0, "left {left} right {right}");
    }
}
