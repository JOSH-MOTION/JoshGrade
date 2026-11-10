//! Crop, rotate and straighten.
//!
//! Order: quarter turns / flip first, then the photo is straightened (rotated by `angle`
//! about its centre) underneath a fixed crop frame `x, y, w, h` (fractions of the turned
//! photo's width and height) — the same model as Lightroom's crop tool.

use crate::Photo;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Crop {
    /// Quarter turns clockwise (0..=3).
    pub rot90: u8,
    pub flip_h: bool,
    /// Straighten, degrees (positive = clockwise).
    pub angle: f32,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Default for Crop {
    fn default() -> Self {
        Crop { rot90: 0, flip_h: false, angle: 0.0, x: 0.0, y: 0.0, w: 1.0, h: 1.0 }
    }
}

impl Crop {
    pub fn active(&self) -> bool {
        self.rot90 % 4 != 0 || self.flip_h || self.angle.abs() > 0.01 || self.x > 0.0005 || self.y > 0.0005 || self.w < 0.9995 || self.h < 0.9995
    }
}

/// Quarter turns and mirror only (cheap; used for previews too).
pub fn turn(p: &Photo, rot90: u8, flip_h: bool) -> Photo {
    let r = rot90 % 4;
    if r == 0 && !flip_h {
        return p.clone();
    }
    let (w, h) = (p.width as usize, p.height as usize);
    let (ow, oh) = if r % 2 == 1 { (h, w) } else { (w, h) };
    let data = (0..ow * oh)
        .into_par_iter()
        .map(|i| {
            let (mut x, y) = (i % ow, i / ow);
            if flip_h {
                x = ow - 1 - x;
            }
            // source coordinates for output (x, y)
            let (sx, sy) = match r {
                0 => (x, y),
                1 => (y, h - 1 - x),
                2 => (w - 1 - x, h - 1 - y),
                _ => (w - 1 - y, x),
            };
            p.data[sy * w + sx]
        })
        .collect();
    Photo { width: ow as u32, height: oh as u32, data, from_raw: p.from_raw }
}

fn bilinear(p: &Photo, fx: f32, fy: f32) -> [f32; 3] {
    let (w, h) = (p.width as usize, p.height as usize);
    let fx = fx.clamp(0.0, (w - 1) as f32);
    let fy = fy.clamp(0.0, (h - 1) as f32);
    let (x0, y0) = (fx as usize, fy as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (ax, ay) = (fx - x0 as f32, fy - y0 as f32);
    let px = |x: usize, y: usize| p.data[y * w + x];
    let (a, b, c, d) = (px(x0, y0), px(x1, y0), px(x0, y1), px(x1, y1));
    [0, 1, 2].map(|k| (a[k] * (1.0 - ax) + b[k] * ax) * (1.0 - ay) + (c[k] * (1.0 - ax) + d[k] * ax) * ay)
}

/// Apply the whole crop.
pub fn apply(p: &Photo, c: &Crop) -> Photo {
    if !c.active() {
        return p.clone();
    }
    let t = turn(p, c.rot90, c.flip_h);
    let (w, h) = (t.width as f32, t.height as f32);
    let ow = ((c.w * w).round() as usize).max(1);
    let oh = ((c.h * h).round() as usize).max(1);
    let (x0, y0) = (c.x * w, c.y * h);
    if c.angle.abs() <= 0.01 {
        let (ix, iy) = (x0.round() as usize, y0.round() as usize);
        let ow = ow.min(t.width as usize - ix.min(t.width as usize - 1));
        let oh = oh.min(t.height as usize - iy.min(t.height as usize - 1));
        let data = (0..ow * oh).into_par_iter().map(|i| t.data[(iy + i / ow) * t.width as usize + ix + i % ow]).collect();
        return Photo { width: ow as u32, height: oh as u32, data, from_raw: p.from_raw };
    }
    let (s, co) = (-c.angle.to_radians()).sin_cos();
    let (cx, cy) = (w / 2.0, h / 2.0);
    let data = (0..ow * oh)
        .into_par_iter()
        .map(|i| {
            let px = x0 + (i % ow) as f32 + 0.5 - cx;
            let py = y0 + (i / ow) as f32 + 0.5 - cy;
            // rotate back into the photo
            let sx = px * co - py * s + cx - 0.5;
            let sy = px * s + py * co + cy - 0.5;
            bilinear(&t, sx, sy)
        })
        .collect();
    Photo { width: ow as u32, height: oh as u32, data, from_raw: p.from_raw }
}

/// Largest scale (≤1) of a crop frame `w×h` (pixels) centred at (cx, cy) that keeps every
/// corner inside a `W×H` photo rotated by `angle` degrees. Used to auto-fit when straightening.
pub fn fit_scale(cw: f32, ch: f32, cx: f32, cy: f32, iw: f32, ih: f32, angle: f32) -> f32 {
    let (s, c) = (-angle.to_radians()).sin_cos();
    let mut k: f32 = 1.0;
    for (dx, dy) in [(-0.5, -0.5), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)] {
        // corner relative to image centre, rotated into the photo
        for _ in 0..30 {
            let px = cx + dx * cw * k - iw / 2.0;
            let py = cy + dy * ch * k - ih / 2.0;
            let sx = px * c - py * s + iw / 2.0;
            let sy = px * s + py * c + ih / 2.0;
            if sx >= -0.5 && sy >= -0.5 && sx <= iw + 0.5 && sy <= ih + 0.5 {
                break;
            }
            k *= 0.97;
        }
    }
    k
}

#[cfg(test)]
mod tests {
    use super::*;
    fn img(w: u32, h: u32) -> Photo {
        Photo { width: w, height: h, data: (0..w * h).map(|i| [(i % w) as f32, (i / w) as f32, 0.0]).collect(), from_raw: false }
    }
    #[test]
    fn quarter_turns() {
        let p = img(3, 2);
        let r = turn(&p, 1, false);
        assert_eq!((r.width, r.height), (2, 3));
        // clockwise: top-left of output is bottom-left of input
        assert_eq!(r.data[0], [0.0, 1.0, 0.0]);
        let back = turn(&turn(&turn(&r, 1, false), 1, false), 1, false);
        assert_eq!(back.data, p.data);
    }
    #[test]
    fn plain_crop() {
        let p = img(10, 10);
        let c = Crop { x: 0.2, y: 0.3, w: 0.5, h: 0.4, ..Default::default() };
        let o = apply(&p, &c);
        assert_eq!((o.width, o.height), (5, 4));
        assert_eq!(o.data[0], [2.0, 3.0, 0.0]);
    }
}
