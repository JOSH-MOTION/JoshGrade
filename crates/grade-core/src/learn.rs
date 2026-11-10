//! The learned grade: a model trained on thousands of photos edited by professional
//! retouchers (MIT-Adobe FiveK) predicts the correction for a new photo from its
//! measurements. The rule-based analysis is one of its inputs, so it starts from the
//! rules and learns where pros differ.
//!
//! Model = gradient-boosted regression trees (one ensemble per parameter), stored as JSON
//! and evaluated here — tiny, instant and fully offline.

use crate::analyze::AutoParams;
use crate::color::*;
use crate::Photo;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

/// The parameters the model predicts (also what the fitter solves for).
pub const PARAMS: [&str; 10] = ["ev", "temp", "tint", "black", "white", "shadows", "contrast", "knee", "vibrance", "sat"];

/// Clamp ranges for each parameter.
pub const RANGES: [(f32, f32); 10] = [
    (-2.5, 2.5),
    (-0.45, 0.45),
    (-0.4, 0.4),
    (-0.1, 0.3),
    (0.55, 1.3),
    (-0.4, 0.6),
    (-0.5, 0.6),
    (0.5, 0.95),
    (-0.6, 0.8),
    (-0.5, 0.6),
];

/// A full correction in model units.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Correction(pub [f32; 10]);

impl Correction {
    pub fn identity() -> Self {
        Correction([0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.92, 0.0, 0.0])
    }

    pub fn clamped(mut self) -> Self {
        for i in 0..10 {
            self.0[i] = self.0[i].clamp(RANGES[i].0, RANGES[i].1);
        }
        if self.0[4] < self.0[3] + 0.3 {
            self.0[4] = self.0[3] + 0.3;
        }
        self
    }

    /// As automatic parameters (keeps `base`'s colour-band fixes). Saturation is folded
    /// into vibrance's companion: returned separately because AutoParams has no global sat.
    pub fn to_auto(&self, base: &AutoParams) -> (AutoParams, f32) {
        let p = &self.0;
        let mut a = *base;
        a.exposure_ev = p[0];
        a.wb_gains = [1.0 + p[1], 1.0 - p[2], 1.0 - p[1]];
        a.black = p[3];
        a.white = p[4];
        a.shadows = p[5];
        a.contrast = p[6];
        a.highlight_knee = p[7];
        a.vibrance = p[8];
        (a, p[9])
    }

    pub fn from_auto(a: &AutoParams) -> Self {
        // wb gains -> temp/tint relative to green
        let g = a.wb_gains[1].max(1e-3);
        let (r, b) = (a.wb_gains[0] / g, a.wb_gains[2] / g);
        let k = 2.0 / (r + b);
        let tint = 1.0 - k;
        let temp = r * k - 1.0;
        // overall gain folded into exposure
        let ev = a.exposure_ev + (g / k).log2();
        Correction([ev, temp, tint, a.black, a.white, a.shadows, a.contrast, a.highlight_knee, a.vibrance, 0.0])
    }
}

// ------------------------------------------------------------------ features

/// CIE L*a*b* (D65) from display sRGB.
pub fn lab(c: Rgb) -> [f32; 3] {
    let l = c.map(to_linear);
    let x = (0.4124 * l[0] + 0.3576 * l[1] + 0.1805 * l[2]) / 0.95047;
    let y = 0.2126 * l[0] + 0.7152 * l[1] + 0.0722 * l[2];
    let z = (0.0193 * l[0] + 0.1192 * l[1] + 0.9505 * l[2]) / 1.08883;
    let f = |t: f32| if t > 0.008856 { t.cbrt() } else { 7.787 * t + 16.0 / 116.0 };
    let (fx, fy, fz) = (f(x), f(y), f(z));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

/// Measurements of a photo that a colourist would look at: the brightness distribution,
/// colour casts in shadows / mids / highlights, how colourful it is, clipping, skin.
/// Plus the rule-based analysis' own answer.
pub fn features(photo: &Photo, rules: &AutoParams) -> Vec<f32> {
    let step = (photo.data.len() / 60_000).max(1);
    let px: Vec<Rgb> = photo.data.iter().step_by(step).copied().collect();
    let n = px.len().max(1) as f32;
    let labs: Vec<[f32; 3]> = px.iter().map(|p| lab(*p)).collect();
    let mut ls: Vec<f32> = labs.iter().map(|l| l[0]).collect();
    ls.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let pct = |q: f32| ls[((ls.len() - 1) as f32 * q) as usize];
    let mut f = vec![];
    for q in [0.01, 0.05, 0.1, 0.25, 0.5, 0.75, 0.9, 0.95, 0.99] {
        f.push(pct(q) / 100.0);
    }
    let mean_l = ls.iter().sum::<f32>() / n;
    let sd_l = (ls.iter().map(|v| (v - mean_l).powi(2)).sum::<f32>() / n).sqrt();
    f.push(mean_l / 100.0);
    f.push(sd_l / 100.0);
    // log-luminance (key) like a light meter
    let key = (px.iter().map(|p| (luma(p.map(to_linear)) + 1e-3).ln()).sum::<f32>() / n).exp();
    f.push(key);
    // colour casts by tone zone
    for (lo, hi) in [(0.0, 30.0), (30.0, 70.0), (70.0, 101.0)] {
        let (mut a, mut b, mut c) = (0.0, 0.0, 0.0);
        for l in &labs {
            if l[0] >= lo && l[0] < hi {
                a += l[1];
                b += l[2];
                c += 1.0;
            }
        }
        f.push(if c > 0.0 { a / c / 50.0 } else { 0.0 });
        f.push(if c > 0.0 { b / c / 50.0 } else { 0.0 });
        f.push(c / n);
    }
    // chroma distribution
    let mut ch: Vec<f32> = labs.iter().map(|l| (l[1] * l[1] + l[2] * l[2]).sqrt()).collect();
    ch.sort_by(|a, b| a.partial_cmp(b).unwrap());
    for q in [0.25, 0.5, 0.75, 0.95] {
        f.push(ch[((ch.len() - 1) as f32 * q) as usize] / 100.0);
    }
    // grey-world and bright-pixel colour ratios (linear)
    let mut sum = [0f32; 3];
    for p in &px {
        let l = p.map(to_linear);
        for c in 0..3 {
            sum[c] += l[c];
        }
    }
    f.push((sum[0] / sum[1].max(1e-4)).ln());
    f.push((sum[2] / sum[1].max(1e-4)).ln());
    // clipping
    f.push(px.iter().filter(|p| max3(**p) >= 0.98).count() as f32 / n);
    f.push(px.iter().filter(|p| max3(**p) <= 0.02).count() as f32 / n);
    // skin share and skin lightness
    let (mut sw, mut sl) = (0.0, 0.0);
    for (p, l) in px.iter().zip(&labs) {
        let w = skin_weight(*p);
        sw += w;
        sl += w * l[0];
    }
    f.push(sw / n);
    f.push(if sw > 0.0 { sl / sw / 100.0 } else { 0.0 });
    // the rules' answer
    f.extend_from_slice(&Correction::from_auto(rules).0[..9]);
    f
}

// ------------------------------------------------------------------ model

#[derive(Deserialize)]
struct Tree {
    /// feature index per node (-1 = leaf)
    f: Vec<i32>,
    t: Vec<f32>,
    l: Vec<i32>,
    r: Vec<i32>,
    v: Vec<f32>,
}

#[derive(Deserialize)]
struct Ensemble {
    init: f32,
    lr: f32,
    trees: Vec<Tree>,
}

/// Nearest neighbours: the corrections pros made to the most similar photos.
#[derive(Deserialize)]
struct Knn {
    k: usize,
    mean: Vec<f32>,
    std: Vec<f32>,
    x: Vec<Vec<f32>>,
    y: Vec<Vec<f32>>,
}

impl Knn {
    fn predict(&self, x: &[f32]) -> [f32; 10] {
        let q: Vec<f32> = x.iter().zip(&self.mean).zip(&self.std).map(|((v, m), s)| (v - m) / s).collect();
        let mut d: Vec<(f32, usize)> = self.x.iter().enumerate().map(|(i, r)| (r.iter().zip(&q).map(|(a, b)| (a - b) * (a - b)).sum::<f32>().sqrt(), i)).collect();
        let k = self.k.min(d.len());
        d.select_nth_unstable_by(k.saturating_sub(1), |a, b| a.0.partial_cmp(&b.0).unwrap());
        let mut out = [0f32; 10];
        let mut wsum = 0.0;
        for &(dist, i) in &d[..k] {
            let w = 1.0 / dist.max(1e-6);
            wsum += w;
            for j in 0..10 {
                out[j] += w * self.y[i][j];
            }
        }
        out.map(|v| v / wsum.max(1e-9))
    }
}

#[derive(Deserialize)]
pub struct Model {
    pub n_features: usize,
    targets: Vec<Ensemble>,
    #[serde(default)]
    knn: Option<Knn>,
}

impl Model {
    pub fn parse(json: &str) -> Result<Model, String> {
        serde_json::from_str(json).map_err(|e| e.to_string())
    }

    pub fn predict(&self, x: &[f32]) -> Correction {
        let mut out = [0f32; 10];
        for (k, e) in self.targets.iter().enumerate().take(10) {
            let mut y = e.init;
            for t in &e.trees {
                let mut i = 0usize;
                while t.f[i] >= 0 {
                    i = if x[t.f[i] as usize] <= t.t[i] { t.l[i] as usize } else { t.r[i] as usize };
                }
                y += e.lr * t.v[i];
            }
            out[k] = y;
        }
        // average of the trees and the nearest neighbours (better than either alone)
        if let Some(knn) = &self.knn {
            let n = knn.predict(x);
            for k in 0..10 {
                out[k] = 0.5 * (out[k] + n[k]);
            }
        }
        Correction(out).clamped()
    }
}

static MODEL: OnceLock<Option<Model>> = OnceLock::new();
const MODEL_JSON: &str = include_str!("../models/grade_model.json");

pub fn model() -> Option<&'static Model> {
    MODEL.get_or_init(|| if std::env::var_os("JOSHGRADE_RULES_ONLY").is_some() { None } else { serde_json::from_str(MODEL_JSON).ok() }).as_ref()
}

/// Plain-English description of a correction, for the app's notes.
pub fn describe(c: &Correction) -> Vec<String> {
    let p = &c.0;
    let mut n = vec![];
    if p[0] > 0.15 {
        n.push(format!("Brightened the photo {:.1} stops", p[0]));
    } else if p[0] < -0.15 {
        n.push(format!("Brought exposure down {:.1} stops", -p[0]));
    }
    if p[1] > 0.04 {
        n.push("Warmed the colour (it was too cool / blue)".into());
    } else if p[1] < -0.04 {
        n.push("Cooled the colour (it was too warm / yellow)".into());
    }
    if p[2] > 0.03 {
        n.push("Removed a green tint".into());
    } else if p[2] < -0.03 {
        n.push("Removed a magenta tint".into());
    }
    if p[3] > 0.04 {
        n.push("Set a true black point (it looked hazy)".into());
    }
    if p[4] < 0.93 {
        n.push("Brightened the whites".into());
    }
    if p[5] > 0.06 {
        n.push("Opened up the shadows".into());
    } else if p[5] < -0.06 {
        n.push("Deepened the shadows".into());
    }
    if p[6] > 0.06 {
        n.push("Added contrast".into());
    } else if p[6] < -0.06 {
        n.push("Softened contrast".into());
    }
    if p[7] < 0.85 {
        n.push("Recovered bright highlights".into());
    }
    let col = p[8] + p[9];
    if col > 0.08 {
        n.push("Richer colour".into());
    } else if col < -0.08 {
        n.push("Calmer colour".into());
    }
    if n.is_empty() {
        n.push("Photo was already well graded – left it almost untouched".into());
    }
    n
}
