//! "Camera look" for RAW files.
//!
//! A RAW file developed with no styling looks flat, dark and grey next to the camera's own
//! JPEG — the camera applies a tone curve and its colour style (Canon Picture Style, Nikon
//! Picture Control, Sony Creative Look…). Every RAW file carries that JPEG inside it, so we
//! measure the camera's rendering from it and apply the same curve and colour to the RAW
//! data. The result looks like the camera's picture but keeps the RAW's highlight and
//! shadow detail. Without a usable preview, a standard camera-like curve is used.

use crate::color::luma;
use image::DynamicImage;
use rayon::prelude::*;

const N: usize = 128;
/// Curve input range (display-encoded; RAW values above 1 are highlight headroom).
const XMAX: f32 = 1.6;

#[derive(Debug, Clone)]
pub struct CameraLook {
    /// Output brightness for N+1 evenly spaced input brightnesses on 0..XMAX.
    curve: Vec<f32>,
    /// Colour matrix applied after the curve.
    m: [[f32; 3]; 3],
}

impl CameraLook {
    /// A typical camera rendering (gentle S-curve, a little more colour).
    pub fn standard() -> Self {
        let curve = (0..=N)
            .map(|i| {
                let x = i as f32 / N as f32 * XMAX;
                if x <= 1.0 {
                    // lift mids slightly, S-shape, soft shoulder
                    let y = x + 0.30 * x * (1.0 - x) * (1.0 - 0.6 * x);
                    let y = y + 0.18 * (y - y * y) * (y - 0.5) * 4.0 * 0.5;
                    y.clamp(0.0, 1.0) * 0.97
                } else {
                    0.97 + 0.03 * ((x - 1.0) / (XMAX - 1.0)).min(1.0)
                }
            })
            .collect();
        let s = 1.18; // saturation
        let (a, b) = ((1.0 + 2.0 * s) / 3.0, (1.0 - s) / 3.0);
        CameraLook { curve, m: [[a, b, b], [b, a, b], [b, b, a]] }
    }

    fn curve_at(&self, x: f32) -> f32 {
        let f = (x.max(0.0) / XMAX * N as f32).min(N as f32);
        let i = (f as usize).min(N - 1);
        let t = f - i as f32;
        self.curve[i] * (1.0 - t) + self.curve[i + 1] * t
    }

    #[inline]
    fn map(&self, p: [f32; 3]) -> [f32; 3] {
        let y = luma(p).max(0.0);
        let k = if y > 1e-5 { self.curve_at(y) / y } else { 1.0 };
        let q = p.map(|v| v.max(0.0) * k);
        let m = &self.m;
        [0, 1, 2].map(|r| (m[r][0] * q[0] + m[r][1] * q[1] + m[r][2] * q[2]).clamp(0.0, 1.0))
    }

    /// Apply to an interleaved RGB display-encoded buffer, in place.
    pub fn apply(&self, flat: &mut [f32]) {
        flat.par_chunks_mut(3).for_each(|c| {
            let o = self.map([c[0], c[1], c[2]]);
            c.copy_from_slice(&o);
        });
    }
}

/// Area-average an interleaved RGB buffer down to `sw`×`sh`.
fn shrink(flat: &[f32], w: usize, h: usize, sw: usize, sh: usize) -> Vec<[f32; 3]> {
    (0..sw * sh)
        .into_par_iter()
        .map(|i| {
            let (x, y) = (i % sw, i / sw);
            let (x0, x1) = (x * w / sw, ((x + 1) * w / sw).max(x * w / sw + 1));
            let (y0, y1) = (y * h / sh, ((y + 1) * h / sh).max(y * h / sh + 1));
            let mut a = [0f32; 3];
            let step = ((x1 - x0) / 4).max(1);
            let mut n = 0.0;
            for yy in (y0..y1).step_by(step) {
                for xx in (x0..x1).step_by(step) {
                    let j = (yy * w + xx) * 3;
                    for c in 0..3 {
                        a[c] += flat[j + c];
                    }
                    n += 1.0;
                }
            }
            a.map(|v| v / n)
        })
        .collect()
}

fn solve3(a: [[f64; 3]; 3], b: [f64; 3]) -> Option<[f64; 3]> {
    let det = |m: [[f64; 3]; 3]| m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0]) + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    let d = det(a);
    if d.abs() < 1e-12 {
        return None;
    }
    let mut out = [0.0; 3];
    for k in 0..3 {
        let mut m = a;
        for r in 0..3 {
            m[r][k] = b[r];
        }
        out[k] = det(m) / d;
    }
    Some(out)
}

/// Measure the camera's rendering from its JPEG. `flat` = the RAW developed to display
/// values (interleaved RGB, w×h, same orientation as the sensor).
pub fn fit(flat: &[f32], w: usize, h: usize, preview: &DynamicImage) -> Option<CameraLook> {
    let (pw, ph) = (preview.width() as usize, preview.height() as usize);
    // Same framing? (allow small crop differences)
    if pw < 64 || ((pw as f32 / ph as f32) / (w as f32 / h as f32) - 1.0).abs() > 0.03 {
        return None;
    }
    let sw = 360.min(pw);
    let sh = (sw as f32 * h as f32 / w as f32).round() as usize;
    let raw = shrink(flat, w, h, sw, sh);
    let pv = preview.resize_exact(sw as u32, sh as u32, image::imageops::FilterType::Triangle).to_rgb32f();
    let jpg: Vec<[f32; 3]> = pv.pixels().map(|p| [p.0[0], p.0[1], p.0[2]]).collect();

    // 1. brightness curve: median camera brightness for each RAW brightness
    let mut bins: Vec<Vec<f32>> = vec![vec![]; N + 1];
    for (r, j) in raw.iter().zip(&jpg) {
        let (yr, yj) = (luma(*r), luma(*j));
        // clipped in the JPEG: says nothing about the curve
        if yr <= 0.0 || j.iter().cloned().fold(0.0, f32::max) >= 0.985 {
            continue;
        }
        let i = ((yr / XMAX) * N as f32).round().clamp(0.0, N as f32) as usize;
        bins[i].push(yj);
    }
    let mut pts: Vec<(usize, f32)> = vec![(0, 0.0)];
    for (i, b) in bins.iter_mut().enumerate().skip(1) {
        if b.len() >= 6 {
            let k = b.len() / 2;
            let med = *b.select_nth_unstable_by(k, |a, c| a.partial_cmp(c).unwrap()).1;
            pts.push((i, med));
        }
    }
    if pts.len() < 12 {
        return None;
    }
    // monotone
    for k in 1..pts.len() {
        if pts[k].1 < pts[k - 1].1 {
            pts[k].1 = pts[k - 1].1;
        }
    }
    // beyond the brightest measured point, roll smoothly up to white (keeps RAW highlight detail)
    let (li, ly) = *pts.last().unwrap();
    if li < N {
        pts.push((N, 1.0f32.max(ly)));
    }
    let mut curve = vec![0f32; N + 1];
    for win in pts.windows(2) {
        let ((i0, y0), (i1, y1)) = (win[0], win[1]);
        for i in i0..=i1 {
            let t = if i1 > i0 { (i - i0) as f32 / (i1 - i0) as f32 } else { 0.0 };
            curve[i] = y0 + (y1 - y0) * t;
        }
    }
    // light smoothing
    let c2 = curve.clone();
    for i in 1..N {
        curve[i] = (c2[i - 1] + 2.0 * c2[i] + c2[i + 1]) / 4.0;
    }
    let mut look = CameraLook { curve, m: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]] };

    // 2. colour: least-squares 3×3 matrix from curve-mapped RAW to the camera's colours
    let mut ata = [[0f64; 3]; 3];
    let mut atb = [[0f64; 3]; 3];
    let mut used = 0;
    for (r, j) in raw.iter().zip(&jpg) {
        let mx = j.iter().cloned().fold(0.0, f32::max);
        if mx >= 0.97 || mx <= 0.03 {
            continue;
        }
        let q = look.map(*r);
        for a in 0..3 {
            for b in 0..3 {
                ata[a][b] += (q[a] * q[b]) as f64;
                atb[a][b] += (q[a] * j[b]) as f64;
            }
        }
        used += 1;
    }
    if used < 500 {
        return None;
    }
    let mut m = [[0f32; 3]; 3];
    for out_c in 0..3 {
        let col = solve3(ata, [atb[0][out_c], atb[1][out_c], atb[2][out_c]])?;
        for k in 0..3 {
            m[out_c][k] = col[k] as f32;
        }
    }
    look.m = m;

    // 3. sanity: it must actually reproduce the camera picture
    let mut err = 0.0;
    let mut n = 0.0;
    for (r, j) in raw.iter().zip(&jpg) {
        let q = look.map(*r);
        err += (0..3).map(|c| (q[c] - j[c]).abs()).sum::<f32>() / 3.0;
        n += 1.0;
    }
    if err / n > 0.06 {
        return None;
    }
    Some(look)
}
