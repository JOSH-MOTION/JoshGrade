//! "The rules": measure the photo and decide the corrections a careful editor would make.
//!
//! Every decision is based on standard editing practice:
//! * neutral things (whites, greys) should have no colour cast -> white balance
//! * the middle of the tonal range should sit around mid-grey -> exposure
//! * a photo should use the full range: a real black and a clean white -> levels
//! * deep shadows should keep detail, highlights shouldn't clip -> shadows / highlights
//! * flat photos need contrast, harsh ones need softening -> contrast
//! * muted colour needs vibrance, garish colour needs calming, skin is protected -> saturation

use crate::color::*;
use crate::Photo;
use serde::Serialize;

#[derive(Debug, Clone, Copy, Serialize)]
pub struct AutoParams {
    /// Linear white-balance multipliers (R, G, B).
    pub wb_gains: [f32; 3],
    /// Exposure change in stops.
    pub exposure_ev: f32,
    /// Input level mapped to black / white.
    pub black: f32,
    pub white: f32,
    /// Shadow lift amount (0..0.35).
    pub shadows: f32,
    /// Contrast change (-0.25..0.35).
    pub contrast: f32,
    /// Highlight shoulder start; lower = stronger highlight recovery.
    pub highlight_knee: f32,
    /// Vibrance change.
    pub vibrance: f32,
}

impl AutoParams {
    pub fn identity() -> Self {
        AutoParams {
            wb_gains: [1.0; 3], exposure_ev: 0.0, black: 0.0, white: 1.0, shadows: 0.0,
            contrast: 0.0, highlight_knee: 0.92, vibrance: 0.0,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Analysis {
    pub params: AutoParams,
    /// Plain-English explanation of every correction, shown in the app.
    pub notes: Vec<String>,
    pub stats: Stats,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Stats {
    pub median_brightness: f32,
    pub clipped_highlights_pct: f32,
    pub skin_pct: f32,
    pub mean_saturation: f32,
}

/// A 0..range histogram for fast percentiles.
struct Hist {
    bins: Vec<u32>,
    range: f32,
    n: u64,
}
impl Hist {
    fn new(range: f32) -> Self {
        Hist { bins: vec![0; 4096], range, n: 0 }
    }
    fn add(&mut self, v: f32) {
        let i = ((v / self.range) * 4095.0).clamp(0.0, 4095.0) as usize;
        self.bins[i] += 1;
        self.n += 1;
    }
    fn pct(&self, p: f32) -> f32 {
        if self.n == 0 {
            return 0.0;
        }
        let target = (p * self.n as f32) as u64;
        let mut acc = 0u64;
        for (i, c) in self.bins.iter().enumerate() {
            acc += *c as u64;
            if acc > target {
                return (i as f32 + 0.5) / 4095.0 * self.range;
            }
        }
        self.range
    }
}

/// Up to ~`max` evenly spread pixels, enough for stable statistics.
fn sample(photo: &Photo, max: usize) -> Vec<Rgb> {
    let step = (photo.data.len() / max).max(1);
    photo.data.iter().step_by(step).copied().collect()
}

pub fn analyze(photo: &Photo) -> Analysis {
    let px = sample(photo, 300_000);
    let n = px.len().max(1) as f32;
    let mut notes = Vec::new();

    let clipped = px.iter().filter(|p| max3(**p) >= 0.995).count() as f32 / n;
    let skin = px.iter().map(|p| skin_weight(*p)).sum::<f32>() / n;

    // ---------------------------------------------------------------- white balance
    // Rule: things that should be neutral (white walls, grey clothes, clouds) must be neutral.
    // Collect near-neutral pixels (weighted by how grey they are); fall back to a
    // "shades of grey" estimate of the whole scene when the photo has few neutrals.
    let mut neutral = [0f64; 3];
    let mut neutral_w = 0f64;
    let mut neutral_count = 0usize;
    let mut sog = [0f64; 3];
    let mut sog_n = 0f64;
    for p in &px {
        let m = max3(*p);
        if !(0.03..=0.985).contains(&m) {
            continue;
        }
        let lin = [to_linear(p[0]) as f64, to_linear(p[1]) as f64, to_linear(p[2]) as f64];
        for c in 0..3 {
            sog[c] += lin[c].powi(6);
        }
        sog_n += 1.0;
        let s = saturation(*p);
        if s < 0.3 && m > 0.1 && skin_weight(*p) < 0.3 {
            // brighter neutrals (white walls, clouds, shirts) are the most trustworthy
            let w = ((1.0 - s / 0.3) * (0.3 + m)) as f64;
            for c in 0..3 {
                neutral[c] += lin[c] * w;
            }
            neutral_w += w;
            neutral_count += 1;
        }
    }
    let sog_est = if sog_n > 0.0 {
        [0, 1, 2].map(|c| (sog[c] / sog_n).powf(1.0 / 6.0) as f32)
    } else {
        [1.0; 3]
    };
    let neutral_frac = neutral_count as f32 / n;
    let (est, confidence) = if neutral_w > 0.0 && neutral_frac > 0.03 {
        let e = neutral.map(|v| (v / neutral_w) as f32);
        (e, (neutral_frac * 5.0).clamp(0.55, 1.0))
    } else if neutral_w > 0.0 {
        let e = neutral.map(|v| (v / neutral_w) as f32);
        ([0, 1, 2].map(|c| 0.5 * e[c] + 0.5 * sog_est[c]), 0.4)
    } else {
        (sog_est, 0.3)
    };
    // Cross-check against the whole-scene estimate. If they disagree on whether the
    // light is warm or cool, the "neutrals" were probably coloured objects (e.g. a sky
    // turned grey by a strong yellow cast) – so correct only gently.
    let confidence = {
        let a = (est[0] / est[2].max(1e-6)).ln();
        let b = (sog_est[0] / sog_est[2].max(1e-6)).ln();
        if a * b < 0.0 && a.abs() > 0.05 && b.abs() > 0.05 {
            confidence * 0.3
        } else {
            confidence
        }
    };
    let mut gains = [1.0f32; 3];
    if est.iter().all(|v| *v > 1e-4) {
        let raw = [est[1] / est[0], 1.0, est[1] / est[2]];
        // keep overall brightness unchanged by the white balance itself
        let before = luma(est);
        let after = luma([est[0] * raw[0], est[1], est[2] * raw[2]]);
        let k = before / after.max(1e-6);
        // Don't fully neutralise: a little of the original light's mood is usually wanted.
        let max_gain = 1.25 + 0.45 * confidence;
        gains = raw.map(|g| (1.0 + (g * k - 1.0) * confidence * 0.9).clamp(1.0 / max_gain, max_gain));
        let rb = est[0] / est[2];
        let g_vs_rb = est[1] / ((est[0] + est[2]) * 0.5);
        if confidence > 0.35 {
            if rb > 1.08 {
                notes.push("Removed a warm / yellow colour cast".into());
            } else if rb < 0.92 {
                notes.push("Removed a cool / blue colour cast".into());
            }
            if g_vs_rb > 1.06 {
                notes.push("Removed a green tint".into());
            } else if g_vs_rb < 0.94 {
                notes.push("Removed a magenta tint".into());
            }
        }
    }

    // ---------------------------------------------------------------- exposure
    // Rule: the middle of the photo's tones should land near mid-grey.
    let lin_luma = |p: &Rgb, ev_gain: f32| -> f32 {
        let l = [to_linear(p[0]) * gains[0], to_linear(p[1]) * gains[1], to_linear(p[2]) * gains[2]];
        luma(l) * ev_gain
    };
    let mut h = Hist::new(1.5);
    for p in &px {
        h.add(to_display(lin_luma(p, 1.0)));
    }
    let median = h.pct(0.5).max(0.01);
    let p95 = h.pct(0.95).max(0.01);
    let target = 0.43;
    let mut ev = (to_linear(target) / to_linear(median)).log2() * 0.75;
    // Rule: don't brighten a photo whose bright parts are already bright – it's a
    // low-key / dark-background scene (stage, night, sports crowd), not underexposed.
    if ev > 0.0 {
        let ev_hl = (to_linear(0.9) / to_linear(p95)).log2();
        ev = ev.min((ev_hl + 0.25).max(0.0));
    }
    // Likewise don't darken a high-key scene whose shadows are already dark.
    if ev < 0.0 {
        let p10 = h.pct(0.10).max(0.005);
        let ev_sh = (to_linear(0.06) / to_linear(p10)).log2();
        ev = ev.max((ev_sh - 0.25).min(0.0));
    }
    let mut ev = ev.clamp(-1.6, 2.0);
    if ev.abs() < 0.1 {
        ev = 0.0;
    }
    if ev <= -0.15 {
        notes.push(format!("Photo was too bright – brought exposure down {:.1} stops", -ev));
    } else if ev >= 0.15 {
        notes.push(format!("Photo was too dark – raised exposure {:.1} stops", ev));
    }
    let ev_gain = 2f32.powf(ev);

    // ---------------------------------------------------------------- levels
    // Rule: use the full tonal range – a true black and a clean white.
    let mut hmin = Hist::new(2.0);
    let mut hmax = Hist::new(2.0);
    for p in &px {
        let d = [0, 1, 2].map(|c| to_display(to_linear(p[c]) * gains[c] * ev_gain));
        hmin.add(min3(d));
        hmax.add(max3(d));
    }
    let p_low = hmin.pct(0.005);
    let p_high = hmax.pct(0.995);
    let black = if p_low > 0.03 { ((p_low - 0.012) * 0.95).min(0.22) } else { 0.0 };
    let white = if p_high < 0.93 { (1.0 - (0.98 - p_high) * 0.9).max(0.7) } else { 1.0 };
    if black > 0.02 {
        notes.push("Photo looked hazy / washed out – set a true black point".into());
    }
    if white < 0.97 {
        notes.push("Brightest areas looked dull grey – brightened whites".into());
    }

    // ---------------------------------------------------------------- tone stats after levels
    let lev = |x: f32| ((x - black) / (white - black)).max(0.0);
    let mut hl = Hist::new(2.0);
    let mut sum = 0f64;
    let mut sum2 = 0f64;
    let mut over = 0usize;
    let mut sat_sum = 0f32;
    let mut sat_n = 0f32;
    for p in &px {
        let d = [0, 1, 2].map(|c| lev(to_display(to_linear(p[c]) * gains[c] * ev_gain)));
        let y = to_display(luma(d.map(to_linear)));
        hl.add(y);
        sum += y as f64;
        sum2 += (y * y) as f64;
        if max3(d) > 0.97 {
            over += 1;
        }
        let v = max3(d);
        if (0.12..0.95).contains(&v) {
            sat_sum += saturation(d);
            sat_n += 1.0;
        }
    }
    let mean = (sum / n as f64) as f32;
    let sd = ((sum2 / n as f64) as f32 - mean * mean).max(0.0).sqrt();
    let p5 = hl.pct(0.05);
    let p25 = hl.pct(0.25);

    // Shadows – Rule: keep detail in the darks unless the photo is meant to be dark.
    let mut shadows = 0.0;
    if p5 < 0.04 {
        shadows += ((0.06 - p5) * 3.0).clamp(0.0, 0.2);
    }
    if p25 < 0.18 {
        shadows += ((0.18 - p25) * 1.2).clamp(0.0, 0.2);
    }
    // A photo we just "de-hazed" shouldn't get its shadows lifted straight back up.
    let shadows = if black > 0.02 { shadows * 0.25 } else { shadows }.min(0.35);
    if shadows > 0.05 {
        notes.push("Opened up deep shadows to reveal detail".into());
    }

    // Contrast – Rule: aim for healthy separation between darks and lights.
    // Only soften genuinely harsh photos; most photos benefit from contrast, not less.
    let contrast = if sd > 0.27 { -((sd - 0.27) * 1.5).min(0.12) } else { ((0.21 - sd) * 2.2).clamp(0.0, 0.35) };
    let contrast = if contrast.abs() < 0.03 { 0.0 } else { contrast };
    if contrast > 0.03 {
        notes.push("Added contrast – the photo was flat".into());
    } else if contrast < -0.03 {
        notes.push("Softened harsh contrast".into());
    }

    // Highlights – Rule: bright areas should roll off smoothly, not clip.
    let over_frac = over as f32 / n;
    let highlight_knee = if over_frac > 0.04 {
        0.68
    } else if over_frac > 0.01 {
        0.78
    } else {
        0.88
    };
    if highlight_knee < 0.8 {
        notes.push("Recovered bright highlights (skies, windows, white clothing)".into());
    }

    // Colour – Rule: lift muted colour, calm garish colour, never push skin.
    let mean_sat = if sat_n > 0.0 { sat_sum / sat_n } else { 0.0 };
    let vibrance = ((0.27 - mean_sat) * 1.5).clamp(-0.3, 0.35);
    let vibrance = if vibrance.abs() < 0.03 { 0.0 } else { vibrance };
    if vibrance > 0.03 {
        notes.push("Boosted dull colours".into());
    } else if vibrance < -0.03 {
        notes.push("Calmed over-saturated colours".into());
    }
    if skin > 0.04 {
        notes.push("Skin tones detected – protected them from colour shifts".into());
    }
    if clipped > 0.02 {
        notes.push(format!(
            "Note: {:.0}% of the original is pure white (blown out) – that detail can't be recovered",
            clipped * 100.0
        ));
    }
    if notes.is_empty() {
        notes.push("Photo was already well balanced – only gentle refinements".into());
    }

    Analysis {
        params: AutoParams {
            wb_gains: gains, exposure_ev: ev, black, white, shadows, contrast, highlight_knee, vibrance,
        },
        notes,
        stats: Stats {
            median_brightness: median,
            clipped_highlights_pct: clipped * 100.0,
            skin_pct: skin * 100.0,
            mean_saturation: mean_sat,
        },
    }
}
