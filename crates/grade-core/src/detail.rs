//! Detail and effects, like Lightroom's Detail and Effects panels plus Texture / Clarity /
//! Dehaze: these look at neighbouring pixels, so they run after the per-pixel grade.
//!
//! Sizes are fractions of the photo (or full-resolution pixels for sharpening), and `k` is
//! this image's size relative to the full-resolution photo — so the preview and the saved
//! file look the same.

use crate::color::{luma, Rgb};
use crate::retouch::{blur, Plane};
use crate::Photo;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Detail {
    /// 0..150
    pub sharpen: f32,
    /// 0.5..3 (full-resolution pixels)
    pub sharpen_radius: f32,
    /// 0..100: only sharpen edges, not flat areas like skin and sky
    pub sharpen_masking: f32,
    /// 0..100
    pub noise: f32,
    /// 0..100
    pub color_noise: f32,
    /// -100..100
    pub texture: f32,
    pub clarity: f32,
    pub dehaze: f32,
    /// -100 (dark edges) .. 100 (light edges)
    pub vignette: f32,
    /// 0..100: how far in it starts
    pub vignette_midpoint: f32,
    /// 0..100: how soft the transition is
    pub vignette_feather: f32,
    /// 0..100
    pub grain: f32,
    /// 0..100
    pub grain_size: f32,
}

impl Default for Detail {
    fn default() -> Self {
        Detail {
            sharpen: 0.0,
            sharpen_radius: 1.0,
            sharpen_masking: 0.0,
            noise: 0.0,
            color_noise: 0.0,
            texture: 0.0,
            clarity: 0.0,
            dehaze: 0.0,
            vignette: 0.0,
            vignette_midpoint: 50.0,
            vignette_feather: 50.0,
            grain: 0.0,
            grain_size: 25.0,
        }
    }
}

impl Detail {
    pub fn spatial(&self) -> bool {
        self.sharpen > 0.0 || self.noise > 0.0 || self.color_noise > 0.0 || self.texture != 0.0 || self.clarity != 0.0 || self.dehaze != 0.0
    }
    pub fn framed(&self) -> bool {
        self.vignette != 0.0 || self.grain > 0.0
    }
}

fn planes(p: &Photo) -> (Plane, Plane, Plane) {
    let (w, h) = (p.width as usize, p.height as usize);
    let y: Vec<f32> = p.data.par_iter().map(|c| luma(*c)).collect();
    let cb: Vec<f32> = p.data.par_iter().zip(&y).map(|(c, y)| c[2] - y).collect();
    let cr: Vec<f32> = p.data.par_iter().zip(&y).map(|(c, y)| c[0] - y).collect();
    (Plane { w, h, d: y }, Plane { w, h, d: cb }, Plane { w, h, d: cr })
}

fn compose(y: &Plane, cb: &Plane, cr: &Plane, from_raw: bool) -> Photo {
    let data = (0..y.d.len())
        .into_par_iter()
        .map(|i| {
            let (yy, r, b) = (y.d[i], cr.d[i] + y.d[i], cb.d[i] + y.d[i]);
            let g = (yy - 0.2126 * r - 0.0722 * b) / 0.7152;
            [r.clamp(0.0, 1.0), g.clamp(0.0, 1.0), b.clamp(0.0, 1.0)]
        })
        .collect();
    Photo { width: y.w as u32, height: y.h as u32, data, from_raw }
}

/// Edge-preserving smoothing of `p` guided by `g` (He et al. guided filter).
fn guided(g: &Plane, p: &Plane, sigma: f32, eps: f32) -> Plane {
    let mi = blur(g, sigma);
    let mp = blur(p, sigma);
    let mii = blur(&g.zip(g, |a, b| a * b), sigma);
    let mip = blur(&g.zip(p, |a, b| a * b), sigma);
    let n = g.d.len();
    let a: Vec<f32> = (0..n).into_par_iter().map(|i| (mip.d[i] - mi.d[i] * mp.d[i]) / (mii.d[i] - mi.d[i] * mi.d[i] + eps)).collect();
    let a = Plane { w: g.w, h: g.h, d: a };
    let b = mp.zip(&a.zip(&mi, |a, m| a * m), |m, am| m - am);
    let (ma, mb) = (blur(&a, sigma), blur(&b, sigma));
    Plane { w: g.w, h: g.h, d: (0..n).into_par_iter().map(|i| ma.d[i] * g.d[i] + mb.d[i]).collect() }
}

fn dehaze(p: &Photo, amount: f32) -> Photo {
    let (w, h) = (p.width as usize, p.height as usize);
    // dark channel at low resolution
    let dark = Plane { w, h, d: p.data.par_iter().map(|c| c[0].min(c[1]).min(c[2])).collect() };
    let k = (256.0 / w.max(h) as f32).min(1.0);
    let small = crate::retouch::shrink(&dark, k);
    // local minimum (3×3 erosion, twice) then smooth
    let mut m = small.clone();
    for _ in 0..2 {
        let src = m.clone();
        for y in 0..m.h {
            for x in 0..m.w {
                let mut v = f32::MAX;
                for dy in y.saturating_sub(1)..(y + 2).min(m.h) {
                    for dx in x.saturating_sub(1)..(x + 2).min(m.w) {
                        v = v.min(src.at(dx, dy));
                    }
                }
                m.d[y * m.w + x] = v;
            }
        }
    }
    let m = blur(&m, 2.0);
    // airlight: brightest of the haziest pixels
    let mut sorted = m.d.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let a = sorted[(sorted.len() as f32 * 0.995) as usize].clamp(0.5, 1.0);
    let data = p
        .data
        .par_iter()
        .enumerate()
        .map(|(i, c)| {
            let (x, y) = ((i % w) as f32 * k, (i / w) as f32 * k);
            let dk = crate::retouch::sample(&m, x, y);
            if amount > 0.0 {
                let t = (1.0 - 0.95 * amount * dk / a).max(0.25);
                c.map(|v| ((v - a) / t + a).clamp(0.0, 1.0))
            } else {
                let f = -amount * 0.5;
                c.map(|v| v * (1.0 - f) + a * 0.92 * f)
            }
        })
        .collect();
    Photo { width: p.width, height: p.height, data, from_raw: p.from_raw }
}

/// Dehaze, noise reduction, texture, clarity and sharpening.
pub fn apply(p: &Photo, d: &Detail, k: f32) -> Photo {
    if !d.spatial() {
        return p.clone();
    }
    let long = p.width.max(p.height) as f32;
    let mut p = if d.dehaze != 0.0 { dehaze(p, d.dehaze / 100.0) } else { p.clone() };
    if !(d.sharpen > 0.0 || d.noise > 0.0 || d.color_noise > 0.0 || d.texture != 0.0 || d.clarity != 0.0) {
        return p;
    }
    let (mut y, mut cb, mut cr) = planes(&p);
    if d.noise > 0.0 {
        let n = d.noise / 100.0;
        y = guided(&y, &y, (1.0 + 3.0 * n) * k.max(0.25), n * n * 0.004 + 1e-6);
    }
    if d.color_noise > 0.0 {
        let n = d.color_noise / 100.0;
        let s = (1.0 + 7.0 * n) * k.max(0.15);
        cb = guided(&y, &cb, s, 2e-4);
        cr = guided(&y, &cr, s, 2e-4);
        // a final gentle blur takes out the blotches
        cb = blur(&cb, s * 0.5);
        cr = blur(&cr, s * 0.5);
    }
    if d.texture != 0.0 {
        let (a, b) = (blur(&y, long * 0.0004), blur(&y, long * 0.0025));
        let t = d.texture / 100.0 * 1.3;
        y = Plane { w: y.w, h: y.h, d: (0..y.d.len()).into_par_iter().map(|i| y.d[i] + t * (a.d[i] - b.d[i])).collect() };
    }
    if d.clarity != 0.0 {
        let big = blur(&y, long * 0.012);
        let c = d.clarity / 100.0 * 0.9;
        y = Plane {
            w: y.w,
            h: y.h,
            d: (0..y.d.len())
                .into_par_iter()
                .map(|i| {
                    let v = y.d[i];
                    let mid = (4.0 * v * (1.0 - v)).clamp(0.0, 1.0);
                    v + c * (v - big.d[i]) * mid
                })
                .collect(),
        };
    }
    if d.sharpen > 0.0 {
        let sigma = d.sharpen_radius.clamp(0.5, 3.0) * k;
        if sigma >= 0.3 {
            let soft = blur(&y, sigma);
            let amt = d.sharpen / 100.0 * 1.2;
            let mask = if d.sharpen_masking > 0.0 {
                // edge strength: how much the blurred image changes locally
                let (w, h) = (y.w, y.h);
                let gx: Vec<f32> = (0..w * h)
                    .into_par_iter()
                    .map(|i| {
                        let (x, yy) = (i % w, i / w);
                        let dx = soft.at((x + 1).min(w - 1), yy) - soft.at(x.saturating_sub(1), yy);
                        let dy = soft.at(x, (yy + 1).min(h - 1)) - soft.at(x, yy.saturating_sub(1));
                        (dx * dx + dy * dy).sqrt()
                    })
                    .collect();
                let e = blur(&Plane { w, h, d: gx }, sigma * 2.0);
                let t0 = d.sharpen_masking / 100.0 * 0.04 / k.max(0.25);
                Some(e.map(|v| ((v - t0) / (t0 * 0.6 + 1e-4)).clamp(0.0, 1.0)))
            } else {
                None
            };
            y = Plane {
                w: y.w,
                h: y.h,
                d: (0..y.d.len())
                    .into_par_iter()
                    .map(|i| {
                        let m = mask.as_ref().map(|m| m.d[i]).unwrap_or(1.0);
                        y.d[i] + amt * m * (y.d[i] - soft.d[i])
                    })
                    .collect(),
            };
        }
    }
    p = compose(&y, &cb, &cr, p.from_raw);
    p
}

/// Where the final picture's frame sits in this image (pixels): centre, half-size and the
/// straighten angle — so the vignette follows the crop in the preview too.
#[derive(Debug, Clone, Copy)]
pub struct Frame {
    pub cx: f32,
    pub cy: f32,
    pub hw: f32,
    pub hh: f32,
    /// image centre (rotation pivot) and angle (degrees)
    pub px: f32,
    pub py: f32,
    pub angle: f32,
    whole: bool,
}

impl Frame {
    pub fn whole() -> Frame {
        Frame { cx: 0.0, cy: 0.0, hw: 0.0, hh: 0.0, px: 0.0, py: 0.0, angle: 0.0, whole: true }
    }
    /// The crop frame of `crop` on a `w×h` (already quarter-turned) image.
    pub fn of_crop(c: &crate::geometry::Crop, w: f32, h: f32) -> Frame {
        Frame { cx: (c.x + c.w / 2.0) * w, cy: (c.y + c.h / 2.0) * h, hw: c.w * w / 2.0, hh: c.h * h / 2.0, px: w / 2.0, py: h / 2.0, angle: c.angle, whole: false }
    }
}

fn hash(x: u32, y: u32) -> f32 {
    let mut h = x.wrapping_mul(374761393).wrapping_add(y.wrapping_mul(668265263));
    h = (h ^ (h >> 13)).wrapping_mul(1274126177);
    h ^= h >> 16;
    (h & 0xffff) as f32 / 65535.0 - 0.5
}

/// Vignette and grain, measured on the final frame.
pub fn finish_frame(p: &Photo, d: &Detail, f: &Frame, k: f32) -> Photo {
    if !d.framed() {
        return p.clone();
    }
    let (w, h) = (p.width as usize, p.height as usize);
    let fr = if f.whole { Frame { cx: w as f32 / 2.0, cy: h as f32 / 2.0, hw: w as f32 / 2.0, hh: h as f32 / 2.0, px: w as f32 / 2.0, py: h as f32 / 2.0, angle: 0.0, whole: false } } else { *f };
    let (sn, cs) = fr.angle.to_radians().sin_cos();
    let r0 = 0.35 + d.vignette_midpoint / 100.0 * 0.9;
    let fw = 0.15 + d.vignette_feather / 100.0 * 1.0;
    let va = d.vignette / 100.0;
    let g_amt = d.grain / 100.0 * 0.09;
    let cell = (0.6 + d.grain_size / 100.0 * 3.0) * k.max(0.1);
    let data = p
        .data
        .par_iter()
        .enumerate()
        .map(|(i, c)| {
            let (x, y) = ((i % w) as f32 + 0.5, (i / w) as f32 + 0.5);
            let mut c: Rgb = *c;
            if va != 0.0 {
                // into the frame's coordinates (rotate with the straighten angle)
                let (dx, dy) = (x - fr.px, y - fr.py);
                let (qx, qy) = (dx * cs - dy * sn + fr.px, dx * sn + dy * cs + fr.py);
                let u = (qx - fr.cx) / fr.hw.max(1.0);
                let v = (qy - fr.cy) / fr.hh.max(1.0);
                let r = (u * u + v * v).sqrt();
                let t = ((r - (r0 - fw / 2.0)) / fw).clamp(0.0, 1.0);
                let s = t * t * (3.0 - 2.0 * t);
                if va < 0.0 {
                    c = c.map(|v| v * (1.0 + va * s));
                } else {
                    c = c.map(|v| v + (1.0 - v) * va * s);
                }
            }
            if g_amt > 0.0 {
                // value noise on a grid of `cell` pixels, smooth between cells
                let (gx, gy) = (x / cell, y / cell);
                let (ix, iy) = (gx.floor() as i64 as u32, gy.floor() as i64 as u32);
                let (fx, fy) = (gx - gx.floor(), gy - gy.floor());
                let n = (hash(ix, iy) * (1.0 - fx) + hash(ix + 1, iy) * fx) * (1.0 - fy) + (hash(ix, iy + 1) * (1.0 - fx) + hash(ix + 1, iy + 1) * fx) * fy;
                let yv = luma(c);
                let wgt = 0.4 + 0.6 * (4.0 * yv * (1.0 - yv)).clamp(0.0, 1.0);
                let add = n * 2.0 * g_amt * wgt;
                c = c.map(|v| v + add);
            }
            c.map(|v| v.clamp(0.0, 1.0))
        })
        .collect();
    Photo { width: p.width, height: p.height, data, from_raw: p.from_raw }
}
