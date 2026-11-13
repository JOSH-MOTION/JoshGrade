//! Spot removal (heal / clone) and red-eye, like Lightroom's Remove and Red Eye tools.
//!
//! A spot is a circle on the photo (upright, after lens corrections; fractions of width and
//! height, radius as a fraction of the long side) and the place its replacement comes from.
//! Heal copies the texture from the source and matches it to the colour and brightness
//! around the spot, so it blends in; Clone copies it exactly.

use crate::color::Rgb;
use crate::lens::Window;
use crate::Photo;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Spot {
    /// "heal", "clone" or "redeye"
    pub kind: SpotKind,
    pub x: f32,
    pub y: f32,
    pub r: f32,
    /// Source centre (heal / clone).
    pub sx: f32,
    pub sy: f32,
    /// 0..1 edge softness.
    pub feather: f32,
    /// 0..1
    pub opacity: f32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum SpotKind {
    #[default]
    Heal,
    Clone,
    Redeye,
}

impl Default for Spot {
    fn default() -> Self {
        Spot { kind: SpotKind::Heal, x: 0.5, y: 0.5, r: 0.02, sx: 0.55, sy: 0.5, feather: 0.5, opacity: 1.0 }
    }
}

fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Apply spots to a buffer covering `win` of the whole frame. `fetch(x, y)` returns the
/// unretouched photo at full-frame pixel (x, y).
pub fn apply(dst: &mut Photo, spots: &[Spot], win: &Window, fetch: &(dyn Fn(f32, f32) -> Rgb + Sync)) {
    let (fw, fh) = (win.full_w, win.full_h);
    let long = fw.max(fh);
    let (w, h) = (dst.width as usize, dst.height as usize);
    for s in spots {
        let (cx, cy, r) = (s.x * fw, s.y * fh, (s.r * long).max(1.0));
        // which buffer pixels it touches
        let bx0 = (((cx - r - win.x0) / win.step).floor().max(0.0)) as usize;
        let by0 = (((cy - r - win.y0) / win.step).floor().max(0.0)) as usize;
        let bx1 = ((((cx + r - win.x0) / win.step).ceil()) as isize).clamp(0, w as isize) as usize;
        let by1 = ((((cy + r - win.y0) / win.step).ceil()) as isize).clamp(0, h as isize) as usize;
        if bx0 >= bx1 || by0 >= by1 {
            continue;
        }
        let inner = r * (1.0 - s.feather.clamp(0.0, 1.0) * 0.8);
        let op = s.opacity.clamp(0.0, 1.0);
        match s.kind {
            SpotKind::Redeye => {
                for by in by0..by1 {
                    for bx in bx0..bx1 {
                        let (fx, fy) = (win.x0 + (bx as f32 + 0.5) * win.step, win.y0 + (by as f32 + 0.5) * win.step);
                        let d = ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt();
                        if d > r {
                            continue;
                        }
                        let m = (1.0 - smooth((d - inner) / (r - inner).max(0.5))) * op;
                        let c = &mut dst.data[by * w + bx];
                        let gb = c[1].max(c[2]);
                        // only clearly red pixels: the pupil, not skin around it
                        let red = ((c[0] - gb * 1.4) / (c[0] + 1e-4) * 3.0).clamp(0.0, 1.0);
                        let a = m * red;
                        if a > 0.0 {
                            let t = (c[1] + c[2]) * 0.5 * 0.85;
                            c[0] += (t - c[0]) * a;
                            c[1] += (c[1] * 0.85 - c[1]) * a;
                            c[2] += (c[2] * 0.85 - c[2]) * a;
                        }
                    }
                }
            }
            SpotKind::Heal | SpotKind::Clone => {
                let (sx, sy) = (s.sx * fw, s.sy * fh);
                let (dx, dy) = (sx - cx, sy - cy);
                // Heal: the difference between the area around the spot and around the source,
                // as a plane (level + gradient) fitted on a ring just outside the circle.
                let corr = if s.kind == SpotKind::Heal { ring_plane(fetch, cx, cy, dx, dy, r) } else { [[0.0; 3]; 3] };
                let rows: Vec<(usize, Vec<(usize, Rgb, f32)>)> = (by0..by1)
                    .into_par_iter()
                    .map(|by| {
                        let mut row = Vec::new();
                        for bx in bx0..bx1 {
                            let (fx, fy) = (win.x0 + (bx as f32 + 0.5) * win.step, win.y0 + (by as f32 + 0.5) * win.step);
                            let d = ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt();
                            if d > r {
                                continue;
                            }
                            let m = (1.0 - smooth((d - inner) / (r - inner).max(0.5))) * op;
                            let src = fetch(fx + dx, fy + dy);
                            let (ux, uy) = ((fx - cx) / r, (fy - cy) / r);
                            let v = [0, 1, 2].map(|k| (src[k] + corr[k][0] + corr[k][1] * ux + corr[k][2] * uy).clamp(0.0, 1.0));
                            row.push((bx, v, m));
                        }
                        (by, row)
                    })
                    .collect();
                for (by, row) in rows {
                    for (bx, v, m) in row {
                        let c = &mut dst.data[by * w + bx];
                        for k in 0..3 {
                            c[k] += (v[k] - c[k]) * m;
                        }
                    }
                }
            }
        }
    }
}

/// Least-squares plane a + b·u + c·v (per channel) of (target − source) on the ring r..1.35r.
fn ring_plane(fetch: &(dyn Fn(f32, f32) -> Rgb + Sync), cx: f32, cy: f32, dx: f32, dy: f32, r: f32) -> [[f32; 3]; 3] {
    let mut out = [[0f32; 3]; 3];
    let n = 48;
    let mut pts: Vec<(f32, f32, Rgb)> = Vec::new();
    for ring in [1.08f32, 1.2, 1.32] {
        for i in 0..n {
            let a = i as f32 / n as f32 * std::f32::consts::TAU;
            let (u, v) = (a.cos() * ring, a.sin() * ring);
            let (x, y) = (cx + u * r, cy + v * r);
            let t = fetch(x, y);
            let s = fetch(x + dx, y + dy);
            pts.push((u, v, [t[0] - s[0], t[1] - s[1], t[2] - s[2]]));
        }
    }
    // robust-ish: drop the 20% largest differences (something else in the ring)
    let mut mags: Vec<f32> = pts.iter().map(|p| p.2.iter().map(|v| v.abs()).sum()).collect();
    let mut sorted = mags.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let cut = sorted[sorted.len() * 8 / 10];
    let keep: Vec<&(f32, f32, Rgb)> = pts.iter().zip(mags.iter_mut()).filter(|(_, m)| **m <= cut + 1e-6).map(|(p, _)| p).collect();
    // normal equations for [1, u, v]
    let mut ata = [[0f32; 3]; 3];
    let mut atb = [[0f32; 3]; 3];
    for &&(u, v, d) in &keep {
        let row = [1.0, u, v];
        for i in 0..3 {
            for j in 0..3 {
                ata[i][j] += row[i] * row[j];
            }
            for k in 0..3 {
                atb[k][i] += row[i] * d[k];
            }
        }
    }
    let inv = invert3(ata);
    if let Some(inv) = inv {
        for k in 0..3 {
            for i in 0..3 {
                out[k][i] = (0..3).map(|j| inv[i][j] * atb[k][j]).sum();
            }
        }
    }
    out
}

fn invert3(m: [[f32; 3]; 3]) -> Option<[[f32; 3]; 3]> {
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0]) + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    if det.abs() < 1e-8 {
        return None;
    }
    let d = 1.0 / det;
    Some([
        [(m[1][1] * m[2][2] - m[1][2] * m[2][1]) * d, (m[0][2] * m[2][1] - m[0][1] * m[2][2]) * d, (m[0][1] * m[1][2] - m[0][2] * m[1][1]) * d],
        [(m[1][2] * m[2][0] - m[1][0] * m[2][2]) * d, (m[0][0] * m[2][2] - m[0][2] * m[2][0]) * d, (m[0][2] * m[1][0] - m[0][0] * m[1][2]) * d],
        [(m[1][0] * m[2][1] - m[1][1] * m[2][0]) * d, (m[0][1] * m[2][0] - m[0][0] * m[2][1]) * d, (m[0][0] * m[1][1] - m[0][1] * m[1][0]) * d],
    ])
}

/// Pixel fetch from a whole photo at full-frame coordinates (photo may be a smaller copy).
pub fn fetcher<'a>(p: &'a Photo, full_w: f32) -> impl Fn(f32, f32) -> Rgb + Sync + 'a {
    let k = p.width as f32 / full_w;
    let (w, h) = (p.width as usize, p.height as usize);
    move |x: f32, y: f32| {
        let fx = (x * k - 0.5).clamp(0.0, (w - 1) as f32);
        let fy = (y * k - 0.5).clamp(0.0, (h - 1) as f32);
        let (x0, y0) = (fx as usize, fy as usize);
        let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
        let (ax, ay) = (fx - x0 as f32, fy - y0 as f32);
        let px = |x: usize, y: usize| p.data[y * w + x];
        let (a, b, c, d) = (px(x0, y0), px(x1, y0), px(x0, y1), px(x1, y1));
        [0, 1, 2].map(|i| (a[i] * (1.0 - ax) + b[i] * ax) * (1.0 - ay) + (c[i] * (1.0 - ax) + d[i] * ax) * ay)
    }
}

/// Whole photo.
pub fn apply_whole(p: &Photo, spots: &[Spot]) -> Photo {
    if spots.is_empty() {
        return p.clone();
    }
    let mut out = p.clone();
    let f = fetcher(p, p.width as f32);
    apply(&mut out, spots, &Window::whole(p), &f);
    out
}

/// Pick a good source for a spot: a nearby place whose surroundings look most like the
/// spot's surroundings (so the patch blends in), never overlapping the spot.
pub fn find_source(p: &Photo, x: f32, y: f32, r: f32) -> (f32, f32) {
    let (fw, fh) = (p.width as f32, p.height as f32);
    let long = fw.max(fh);
    let (cx, cy, rr) = (x * fw, y * fh, (r * long).max(2.0));
    let f = fetcher(p, fw);
    let ring = |ox: f32, oy: f32| -> Vec<Rgb> {
        let mut v = Vec::new();
        for ring in [1.1f32, 1.4] {
            for i in 0..32 {
                let a = i as f32 / 32.0 * std::f32::consts::TAU;
                v.push(f(ox + a.cos() * rr * ring, oy + a.sin() * rr * ring));
            }
        }
        v
    };
    let target = ring(cx, cy);
    let tmean = target.iter().fold([0f32; 3], |a, c| [a[0] + c[0], a[1] + c[1], a[2] + c[2]]).map(|v| v / target.len() as f32);
    let mut best = (cx + 2.5 * rr, cy, f32::MAX);
    for dist in [2.3f32, 3.0, 4.0, 5.5] {
        for i in 0..16 {
            let a = i as f32 / 16.0 * std::f32::consts::TAU;
            let (sx, sy) = (cx + a.cos() * rr * dist, cy + a.sin() * rr * dist);
            if sx - rr * 1.4 < 0.0 || sy - rr * 1.4 < 0.0 || sx + rr * 1.4 > fw || sy + rr * 1.4 > fh {
                continue;
            }
            let cand = ring(sx, sy);
            let cmean = cand.iter().fold([0f32; 3], |a, c| [a[0] + c[0], a[1] + c[1], a[2] + c[2]]).map(|v| v / cand.len() as f32);
            // texture difference after removing the overall level (heal fixes level anyway),
            // plus a little for the level itself and for distance
            let mut e = 0.0;
            for (t, c) in target.iter().zip(&cand) {
                for k in 0..3 {
                    e += ((t[k] - tmean[k]) - (c[k] - cmean[k])).abs();
                }
            }
            e += (0..3).map(|k| (tmean[k] - cmean[k]).abs()).sum::<f32>() * 8.0 + dist * 0.15;
            // the inside of the source should be calm (no other blemish to copy)
            let inside: Vec<Rgb> = (0..12).map(|j| {
                let b = j as f32 / 12.0 * std::f32::consts::TAU;
                f(sx + b.cos() * rr * 0.5, sy + b.sin() * rr * 0.5)
            }).collect();
            let im = inside.iter().fold([0f32; 3], |a, c| [a[0] + c[0], a[1] + c[1], a[2] + c[2]]).map(|v| v / 12.0);
            e += inside.iter().map(|c| (0..3).map(|k| (c[k] - im[k]).abs()).sum::<f32>()).sum::<f32>() * 2.0;
            if e < best.2 {
                best = (sx, sy, e);
            }
        }
    }
    (best.0 / fw, best.1 / fh)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn heal_removes_a_dot() {
        let (w, h) = (120usize, 90usize);
        // gentle gradient background + a dark dot
        let mut data: Vec<Rgb> = (0..w * h).map(|i| {
            let x = (i % w) as f32 / w as f32;
            [0.6 + 0.1 * x, 0.5, 0.4]
        }).collect();
        for y in 40..50 {
            for x in 55..65 {
                data[y * w + x] = [0.1, 0.1, 0.1];
            }
        }
        let p = Photo { width: w as u32, height: h as u32, data, from_raw: false };
        let (sx, sy) = find_source(&p, 60.0 / w as f32, 45.0 / h as f32, 8.0 / w as f32);
        let s = Spot { x: 60.0 / w as f32, y: 45.0 / h as f32, r: 9.0 / w as f32, sx, sy, ..Default::default() };
        let o = apply_whole(&p, &[s]);
        let c = o.data[45 * w + 60];
        let expect = 0.6 + 0.1 * 60.0 / w as f32;
        assert!((c[0] - expect).abs() < 0.03 && (c[1] - 0.5).abs() < 0.03, "{c:?}");
    }
}
