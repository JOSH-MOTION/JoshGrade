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

#[derive(Deserialize)]
pub struct Model {
    pub n_features: usize,
    targets: Vec<Ensemble>,
}

impl Model {
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
        Correction(out).clamped()
    }
}

static MODEL: OnceLock<Option<Model>> = OnceLock::new();
const MODEL_JSON: &str = include_str!("../models/grade_model.json");

pub fn model() -> Option<&'static Model> {
    MODEL.get_or_init(|| serde_json::from_str(MODEL_JSON).ok()).as_ref()
}
