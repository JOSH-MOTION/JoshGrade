//! Lens corrections and Transform, like Lightroom's panels of the same names:
//! distortion, lens vignetting, chromatic aberration (colour fringes at edges), defringe,
//! and perspective (vertical / horizontal keystone, rotate, aspect, scale, offset).
//!
//! Applied to the upright photo before grading. It's one geometric warp: every output pixel
//! looks up where it comes from in the original, so it works the same on the preview, on a
//! zoomed-in piece and on the full-size save.

use crate::color::{luma, to_display, to_linear, Rgb};
use crate::Photo;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Lens {
    /// − barrel (bulging) … + pincushion: what to correct.
    pub distortion: f32,
    /// Brighten (+) or darken (−) the corners to undo the lens' own vignetting.
    pub vignetting: f32,
    pub vignetting_mid: f32,
    /// Remove lateral chromatic aberration (red / blue edges) automatically.
    pub remove_ca: bool,
    /// Take out purple / green fringes along high-contrast edges (0..100).
    pub defringe_purple: f32,
    pub defringe_green: f32,
    /// Transform: keystone. Vertical + makes leaning-back buildings straight.
    pub vertical: f32,
    pub horizontal: f32,
    /// Degrees.
    pub rotate: f32,
    /// − squeeze / + stretch.
    pub aspect: f32,
    /// Percent (100 = no change).
    pub scale: f32,
    pub x_offset: f32,
    pub y_offset: f32,
    /// Zoom in just enough that no empty (black) edges show — Lightroom's "Constrain crop".
    pub constrain: bool,
}

impl Default for Lens {
    fn default() -> Self {
        Lens {
            distortion: 0.0,
            vignetting: 0.0,
            vignetting_mid: 50.0,
            remove_ca: false,
            defringe_purple: 0.0,
            defringe_green: 0.0,
            vertical: 0.0,
            horizontal: 0.0,
            rotate: 0.0,
            aspect: 0.0,
            scale: 100.0,
            x_offset: 0.0,
            y_offset: 0.0,
            constrain: true,
        }
    }
}

impl Lens {
    pub fn geometric(&self) -> bool {
        self.distortion != 0.0 || self.vertical != 0.0 || self.horizontal != 0.0 || self.rotate != 0.0 || self.aspect != 0.0 || (self.scale - 100.0).abs() > 0.01 || self.x_offset != 0.0 || self.y_offset != 0.0
    }
    pub fn active(&self) -> bool {
        self.geometric() || self.vignetting != 0.0 || self.remove_ca || self.defringe_purple > 0.0 || self.defringe_green > 0.0
    }
}

/// Where in the whole photo an output buffer sits (pixels of the full output frame).
#[derive(Debug, Clone, Copy)]
pub struct Window {
    pub full_w: f32,
    pub full_h: f32,
    pub x0: f32,
    pub y0: f32,
    /// full-frame pixels per buffer pixel
    pub step: f32,
}

impl Window {
    pub fn whole(p: &Photo) -> Window {
        Window { full_w: p.width as f32, full_h: p.height as f32, x0: 0.0, y0: 0.0, step: 1.0 }
    }
}

/// The geometric part: output point (normalised, centre 0, long side −1..1) → source point.
struct Warp {
    k1: f32,
    rot: (f32, f32),
    persp: (f32, f32),
    aspect: f32,
    scale: f32,
    off: (f32, f32),
}

impl Warp {
    fn new(l: &Lens, fw: f32, fh: f32) -> Warp {
        let mut w = Warp::raw(l);
        if l.constrain {
            // smallest extra zoom where every edge point of the output lands on the photo
            let half = fw.max(fh) / 2.0;
            let (ax, ay) = (fw / 2.0 / half, fh / 2.0 / half);
            let inside = |w: &Warp| {
                (0..=40).all(|i| {
                    let t = i as f32 / 40.0 * 2.0 - 1.0;
                    [(t * ax, -ay), (t * ax, ay), (-ax, t * ay), (ax, t * ay)].iter().all(|&(x, y)| {
                        let (sx, sy) = w.map(x, y);
                        sx.abs() <= ax * 1.0005 && sy.abs() <= ay * 1.0005
                    })
                })
            };
            if !inside(&w) {
                let base = w.scale;
                let (mut lo, mut hi) = (1.0f32, 4.0f32);
                for _ in 0..24 {
                    let mid = (lo + hi) / 2.0;
                    w.scale = base * mid;
                    if inside(&w) { hi = mid } else { lo = mid }
                }
                w.scale = base * hi;
            }
        }
        w
    }

    fn raw(l: &Lens) -> Warp {
        let r = (-l.rotate).to_radians();
        Warp {
            k1: -l.distortion / 100.0 * 0.12,
            rot: (r.sin(), r.cos()),
            persp: (l.horizontal / 100.0 * 0.45, l.vertical / 100.0 * 0.45),
            aspect: 1.0 + l.aspect / 100.0 * 0.3,
            scale: (l.scale / 100.0).max(0.2),
            off: (-l.x_offset / 100.0 * 0.5, -l.y_offset / 100.0 * 0.5),
        }
    }
    /// (x, y) on the output → (x, y) in the source, both centred, units of half the long side.
    #[inline]
    fn map(&self, x: f32, y: f32) -> (f32, f32) {
        // scale, offset, aspect
        let mut x = x / self.scale + self.off.0;
        let mut y = y / self.scale + self.off.1;
        x /= self.aspect.sqrt();
        y *= self.aspect.sqrt();
        // rotate
        let (s, c) = self.rot;
        let (rx, ry) = (x * c - y * s, x * s + y * c);
        x = rx;
        y = ry;
        // perspective (keystone): points further up/left are pulled in or pushed out
        let w = 1.0 + self.persp.0 * x + self.persp.1 * y;
        let w = w.max(0.1);
        x /= w;
        y /= w;
        // lens distortion (radial)
        let r2 = x * x + y * y;
        let f = 1.0 + self.k1 * r2;
        (x * f, y * f)
    }
}

fn bilinear(p: &Photo, fx: f32, fy: f32) -> Option<Rgb> {
    let (w, h) = (p.width as usize, p.height as usize);
    if fx < -0.5 || fy < -0.5 || fx > w as f32 - 0.5 || fy > h as f32 - 0.5 {
        return None;
    }
    let fx = fx.clamp(0.0, (w - 1) as f32);
    let fy = fy.clamp(0.0, (h - 1) as f32);
    let (x0, y0) = (fx as usize, fy as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (ax, ay) = (fx - x0 as f32, fy - y0 as f32);
    let px = |x: usize, y: usize| p.data[y * w + x];
    let (a, b, c, d) = (px(x0, y0), px(x1, y0), px(x0, y1), px(x1, y1));
    Some([0, 1, 2].map(|k| (a[k] * (1.0 - ax) + b[k] * ax) * (1.0 - ay) + (c[k] * (1.0 - ax) + d[k] * ax) * ay))
}

fn channel(p: &Photo, k: usize, fx: f32, fy: f32) -> f32 {
    bilinear(p, fx, fy).map(|c| c[k]).unwrap_or(0.0)
}

/// Lateral chromatic aberration: how much bigger (+) or smaller (−) the red and blue images
/// are than green, found by trying small scalings and keeping the one where the colour
/// edges line up best.
pub fn estimate_ca(p: &Photo) -> (f32, f32) {
    // work on a copy of at most ~1600 px
    let small;
    let p = if p.width.max(p.height) > 1700 {
        small = crate::decode::shrink_photo(p, 1600);
        &small
    } else {
        p
    };
    let (w, h) = (p.width as usize, p.height as usize);
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    // sample points: strong edges away from the centre (where CA is visible)
    let lum: Vec<f32> = p.data.iter().map(|c| luma(*c)).collect();
    let mut pts: Vec<(usize, usize)> = Vec::new();
    let st = 3;
    for y in (2..h - 2).step_by(st) {
        for x in (2..w - 2).step_by(st) {
            let r = (((x as f32 - cx) / cx).powi(2) + ((y as f32 - cy) / cy).powi(2)).sqrt();
            if r < 0.45 {
                continue;
            }
            let gx = lum[y * w + x + 1] - lum[y * w + x - 1];
            let gy = lum[(y + 1) * w + x] - lum[(y - 1) * w + x];
            if gx * gx + gy * gy > 0.02 {
                pts.push((x, y));
            }
        }
    }
    if pts.len() < 200 {
        return (0.0, 0.0);
    }
    let cost = |k: usize, s: f32| -> f32 {
        pts.par_iter()
            .map(|&(x, y)| {
                let (dx, dy) = (x as f32 - cx, y as f32 - cy);
                let v = channel(p, k, cx + dx * (1.0 + s), cy + dy * (1.0 + s));
                (v - p.data[y * w + x][1]).abs()
            })
            .sum::<f32>()
    };
    let best = |k: usize| -> f32 {
        let mut b = (0.0f32, f32::MAX);
        for i in -12..=12 {
            let s = i as f32 * 0.0004;
            let c = cost(k, s);
            if c < b.1 {
                b = (s, c);
            }
        }
        // only correct if it clearly helps
        if cost(k, 0.0) * 0.985 < b.1 { 0.0 } else { b.0 }
    };
    (best(0), best(2))
}

/// Apply lens corrections + transform to a buffer at `win` of the whole output frame.
/// `src` is the whole uncorrected photo (any scale; `src_scale` = src pixels per full pixel).
pub fn apply_window(src: &Photo, l: &Lens, ca: (f32, f32), win: &Window, out_w: usize, out_h: usize) -> Photo {
    let (fw, fh) = (win.full_w, win.full_h);
    let warp = Warp::new(l, fw, fh);
    let half = fw.max(fh) / 2.0;
    let (sw, sh) = (src.width as f32, src.height as f32);
    let sk = sw / fw; // src px per full px
    let vig = l.vignetting / 100.0;
    let vmid = 0.2 + l.vignetting_mid / 100.0 * 0.8;
    let geo = l.geometric();
    let has_ca = ca.0 != 0.0 || ca.1 != 0.0;
    let data: Vec<Rgb> = (0..out_w * out_h)
        .into_par_iter()
        .map(|i| {
            let fx = win.x0 + ((i % out_w) as f32 + 0.5) * win.step;
            let fy = win.y0 + ((i / out_w) as f32 + 0.5) * win.step;
            let (nx, ny) = ((fx - fw / 2.0) / half, (fy - fh / 2.0) / half);
            let (sx, sy) = if geo { warp.map(nx, ny) } else { (nx, ny) };
            let px = (sx * half + fw / 2.0) * sk - 0.5;
            let py = (sy * half + fh / 2.0) * sk - 0.5;
            let mut c = if has_ca {
                let (cx, cy) = (sw / 2.0 - 0.5, sh / 2.0 - 0.5);
                let at = |k: usize, s: f32| channel(src, k, cx + (px - cx) * (1.0 + s), cy + (py - cy) * (1.0 + s));
                match bilinear(src, px, py) {
                    Some(g) => [at(0, ca.0), g[1], at(2, ca.1)],
                    None => [0.0; 3],
                }
            } else {
                bilinear(src, px, py).unwrap_or([0.0; 3])
            };
            if vig != 0.0 {
                // radius to the corner = 1
                let r = ((sx * half / (fw / 2.0)).powi(2) + (sy * half / (fh / 2.0)).powi(2)).sqrt() / std::f32::consts::SQRT_2;
                let t = ((r - (1.0 - vmid)) / vmid).clamp(0.0, 1.0);
                let g = 1.0 + vig * t * t * 1.2;
                c = c.map(|v| to_display((to_linear(v) * g).min(1.0)));
            }
            c
        })
        .collect();
    let mut out = Photo { width: out_w as u32, height: out_h as u32, data, from_raw: src.from_raw };
    if l.defringe_purple > 0.0 || l.defringe_green > 0.0 {
        defringe(&mut out, l.defringe_purple / 100.0, l.defringe_green / 100.0);
    }
    out
}

/// Whole photo.
pub fn apply(p: &Photo, l: &Lens) -> Photo {
    if !l.active() {
        return p.clone();
    }
    let ca = if l.remove_ca { estimate_ca(p) } else { (0.0, 0.0) };
    apply_window(p, l, ca, &Window::whole(p), p.width as usize, p.height as usize)
}

/// Purple / green fringes: colourful pixels of those hues right next to strong brightness
/// edges lose their colour.
fn defringe(p: &mut Photo, purple: f32, green: f32) {
    let (w, h) = (p.width as usize, p.height as usize);
    let lum: Vec<f32> = p.data.iter().map(|c| luma(*c)).collect();
    // edge strength, spread a couple of pixels
    let e: Vec<f32> = (0..w * h)
        .into_par_iter()
        .map(|i| {
            let (x, y) = (i % w, i / w);
            let mut m: f32 = 0.0;
            for dy in -2i32..=2 {
                for dx in -2i32..=2 {
                    let (xx, yy) = ((x as i32 + dx).clamp(1, w as i32 - 2) as usize, (y as i32 + dy).clamp(1, h as i32 - 2) as usize);
                    let gx = lum[yy * w + xx + 1] - lum[yy * w + xx - 1];
                    let gy = lum[(yy + 1) * w + xx] - lum[(yy - 1) * w + xx];
                    m = m.max(gx.abs() + gy.abs());
                }
            }
            (m * 3.0).min(1.0)
        })
        .collect();
    p.data.par_iter_mut().enumerate().for_each(|(i, c)| {
        if e[i] < 0.05 {
            return;
        }
        let h = crate::color::hue(*c);
        let s = crate::color::saturation(*c);
        let near = |a: f32, b: f32| -> f32 {
            if h >= a && h <= b {
                1.0
            } else {
                let d = (h - a).abs().min((h - b).abs());
                (1.0 - d / 20.0).max(0.0)
            }
        };
        let amt = (purple * near(250.0, 320.0) + green * near(85.0, 150.0)).min(1.0) * e[i] * (s * 3.0).min(1.0);
        if amt > 0.0 {
            let y = luma(*c);
            *c = c.map(|v| v + (y - v) * amt);
        }
    });
}

/// Auto straighten: the small angle that makes the strongest near-horizontal and
/// near-vertical lines level (degrees, positive = clockwise, as `Crop::angle`).
pub fn auto_level(p: &Photo) -> Option<f32> {
    let small;
    let p = if p.width.max(p.height) > 900 {
        small = crate::decode::shrink_photo(p, 800);
        &small
    } else {
        p
    };
    let (w, h) = (p.width as usize, p.height as usize);
    let lum: Vec<f32> = p.data.iter().map(|c| luma(*c)).collect();
    // histogram of edge directions within ±12° of level / plumb, weighted by strength
    let bins = 241; // 0.1° steps
    let mut hist = vec![0f32; bins];
    let grads: Vec<(f32, f32)> = (0..w * h)
        .map(|i| {
            let (x, y) = (i % w, i / w);
            if x == 0 || y == 0 || x == w - 1 || y == h - 1 {
                return (0.0, 0.0);
            }
            ((lum[y * w + x + 1] - lum[y * w + x - 1]) * 0.5, (lum[(y + 1) * w + x] - lum[(y - 1) * w + x]) * 0.5)
        })
        .collect();
    // only the strongest 10% of edges vote
    let mut mags: Vec<f32> = grads.iter().map(|g| g.0 * g.0 + g.1 * g.1).collect();
    let k = mags.len() * 9 / 10;
    let thr = (*mags.select_nth_unstable_by(k, |a, b| a.partial_cmp(b).unwrap()).1).max(1e-6);
    for &(gx, gy) in &grads {
        {
            let m = gx * gx + gy * gy;
            if m < thr {
                continue;
            }
            // edge direction is perpendicular to the gradient
            let mut a = gy.atan2(gx).to_degrees() + 90.0;
            // fold to -45..45 around the nearest axis
            a = ((a + 45.0).rem_euclid(90.0)) - 45.0;
            if a.abs() <= 12.0 {
                let b = ((a + 12.0) * 10.0).round() as usize;
                hist[b.min(bins - 1)] += m.sqrt();
            }
        }
    }
    // smooth and find the peak
    let sm: Vec<f32> = (0..bins).map(|i| (i.saturating_sub(5)..(i + 6).min(bins)).map(|j| hist[j]).sum()).collect();
    let (bi, bv) = sm.iter().enumerate().fold((0, 0f32), |b, (i, &v)| if v > b.1 { (i, v) } else { b });
    let total: f32 = sm.iter().sum::<f32>() / bins as f32;
    if bv < total * 2.0 {
        return None; // no clear lines
    }
    // refine: centre of mass of the raw histogram within ±1° of the peak
    let (mut sw, mut sv) = (0f32, 0f32);
    for j in bi.saturating_sub(10)..(bi + 11).min(bins) {
        sw += hist[j];
        sv += hist[j] * j as f32;
    }
    let tilt = if sw > 0.0 { sv / sw } else { bi as f32 } / 10.0 - 12.0;
    if tilt.abs() < 0.15 {
        return Some(0.0);
    }
    // the photo's lines are tilted by `tilt` (counter-clockwise in screen coords when +);
    // straightening rotates the other way
    Some(-tilt)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identity_is_identity() {
        let p = Photo { width: 40, height: 30, data: (0..1200).map(|i| [(i % 40) as f32 / 40.0, (i / 40) as f32 / 30.0, 0.5]).collect(), from_raw: false };
        let mut l = Lens::default();
        l.vignetting = 0.0001; // force a pass
        let o = apply(&p, &l);
        let d: f32 = p.data.iter().zip(&o.data).map(|(a, b)| (a[0] - b[0]).abs() + (a[1] - b[1]).abs()).sum::<f32>() / 1200.0;
        assert!(d < 0.01, "{d}");
    }
    #[test]
    fn finds_a_tilt() {
        // stripes tilted by 3 degrees
        let (w, h) = (400usize, 300usize);
        let t = 3f32.to_radians();
        let data = (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32, (i / w) as f32);
                let v = ((y * t.cos() - x * t.sin()) / 12.0).sin() * 0.4 + 0.5;
                [v, v, v]
            })
            .collect();
        let p = Photo { width: w as u32, height: h as u32, data, from_raw: false };
        let a = auto_level(&p).unwrap();
        assert!((a + 3.0).abs() < 0.3, "{a}");
    }
}

/// Read the corrected photo at any full-frame point (geometry only), straight from the
/// uncorrected source — used where a tool needs pixels outside the piece being drawn.
pub fn sampler<'a>(src: &'a Photo, l: &Lens, full_w: f32, full_h: f32) -> impl Fn(f32, f32) -> Rgb + Sync + 'a {
    let warp = Warp::new(l, full_w, full_h);
    let geo = l.geometric();
    let half = full_w.max(full_h) / 2.0;
    let sk = src.width as f32 / full_w;
    move |fx: f32, fy: f32| {
        let (nx, ny) = ((fx - full_w / 2.0) / half, (fy - full_h / 2.0) / half);
        let (sx, sy) = if geo { warp.map(nx, ny) } else { (nx, ny) };
        bilinear(src, (sx * half + full_w / 2.0) * sk - 0.5, (sy * half + full_h / 2.0) * sk - 0.5).unwrap_or([0.0; 3])
    }
}
