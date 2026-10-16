//! Measurements shown in the app so you can *see* whether a photo is correct:
//! a histogram with clipping warnings, an exposure verdict, and per-colour strength
//! for the colour wheel (with the "too strong" limits used by the auto-correction).

use crate::color::*;
use crate::Photo;
use serde::Serialize;

pub const HIST_BINS: usize = 128;
pub const WHEEL_SIZE: usize = 64;

/// Saturation above which a pixel has lost detail: one colour channel is crushed to
/// zero, so texture in that colour turns into a flat blob ("colour clipping").
pub const SATURATED: f32 = 0.97;
/// A colour is "too much" when more than this % of it is colour-clipped.
pub const SATURATED_LIMIT: f32 = 12.0;

#[derive(Debug, Clone, Serialize)]
pub struct BandStat {
    pub name: &'static str,
    pub hue: f32,
    /// Share of the colourful part of the photo that is this colour (0..1).
    pub share: f32,
    /// How intense this colour gets (90th percentile saturation, 0..1).
    pub strength: f32,
    /// % of this colour's pixels that are colour-clipped (lost detail).
    pub saturated: f32,
    /// % of this colour's pixels that are clipped (a channel hit 255).
    pub clipped: f32,
    pub over: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Scopes {
    pub hist_r: Vec<u32>,
    pub hist_g: Vec<u32>,
    pub hist_b: Vec<u32>,
    pub hist_l: Vec<u32>,
    /// % of pixels blown out to white / crushed to black.
    pub clip_high: f32,
    pub clip_low: f32,
    /// Median brightness 0..1.
    pub median: f32,
    /// "under", "good" or "over".
    pub exposure: &'static str,
    /// WHEEL_SIZE² density grid (0..255) of where the photo's colours sit on the colour wheel.
    pub wheel: Vec<u8>,
    pub bands: Vec<BandStat>,
}

pub fn band_stats(px: &[Rgb]) -> Vec<BandStat> {
    // Per band: weighted saturation histogram for the percentile, plus share and clipping.
    const SB: usize = 64;
    let mut hist = [[0f32; SB]; 8];
    let mut wsum = [0f32; 8];
    let mut clip = [0f32; 8];
    let mut satc = [0f32; 8];
    let mut total = 0f32;
    for p in px {
        let v = max3(*p);
        let s = saturation(*p);
        if v < 0.15 || s < 0.12 {
            continue;
        }
        let w = band_weights(hue(*p));
        let si = ((s * (SB - 1) as f32) as usize).min(SB - 1);
        let clipped = if v >= 0.985 { 1.0 } else { 0.0 };
        let sat_clip = if s >= SATURATED { 1.0 } else { 0.0 };
        total += 1.0;
        for b in 0..8 {
            if w[b] > 0.0 {
                hist[b][si] += w[b];
                wsum[b] += w[b];
                clip[b] += w[b] * clipped;
                satc[b] += w[b] * sat_clip;
            }
        }
    }
    (0..8)
        .map(|b| {
            let share = if total > 0.0 { wsum[b] / total } else { 0.0 };
            let strength = if wsum[b] > 0.0 {
                let target = wsum[b] * 0.9;
                let mut acc = 0.0;
                let mut out = 1.0;
                for (i, c) in hist[b].iter().enumerate() {
                    acc += c;
                    if acc >= target {
                        out = (i as f32 + 0.5) / SB as f32;
                        break;
                    }
                }
                out
            } else {
                0.0
            };
            let clipped = if wsum[b] > 0.0 { clip[b] / wsum[b] * 100.0 } else { 0.0 };
            let saturated = if wsum[b] > 0.0 { satc[b] / wsum[b] * 100.0 } else { 0.0 };
            BandStat {
                name: BAND_NAMES[b],
                hue: BAND_HUES[b],
                share,
                strength,
                saturated,
                clipped,
                over: share > 0.03 && (saturated > SATURATED_LIMIT || clipped > 15.0),
            }
        })
        .collect()
}

pub fn compute(photo: &Photo) -> Scopes {
    let step = (photo.data.len() / 250_000).max(1);
    let px: Vec<Rgb> = photo.data.iter().step_by(step).copied().collect();
    let n = px.len().max(1) as f32;

    let mut hr = vec![0u32; HIST_BINS];
    let mut hg = vec![0u32; HIST_BINS];
    let mut hb = vec![0u32; HIST_BINS];
    let mut hl = vec![0u32; HIST_BINS];
    let bin = |v: f32| ((v.clamp(0.0, 1.0) * (HIST_BINS - 1) as f32).round() as usize).min(HIST_BINS - 1);
    let mut high = 0usize;
    let mut low = 0usize;
    let mut wheel = vec![0f32; WHEEL_SIZE * WHEEL_SIZE];
    for p in &px {
        hr[bin(p[0])] += 1;
        hg[bin(p[1])] += 1;
        hb[bin(p[2])] += 1;
        let y = to_display(luma(p.map(to_linear)));
        hl[bin(y)] += 1;
        if max3(*p) >= 0.995 {
            high += 1;
        }
        if max3(*p) <= 0.01 {
            low += 1;
        }
        // Colour wheel: angle = hue (red at the top, clockwise), distance = saturation.
        let s = saturation(*p);
        if max3(*p) > 0.06 {
            let a = hue(*p).to_radians();
            let x = (0.5 + 0.5 * s * a.sin()) * (WHEEL_SIZE - 1) as f32;
            let yy = (0.5 - 0.5 * s * a.cos()) * (WHEEL_SIZE - 1) as f32;
            wheel[yy.round() as usize * WHEEL_SIZE + x.round() as usize] += 1.0;
        }
    }
    // Log scale so faint colours remain visible next to the dominant ones.
    let wmax = wheel.iter().cloned().fold(0.0f32, f32::max).max(1.0);
    let wheel = wheel.iter().map(|v| ((1.0 + v).ln() / (1.0 + wmax).ln() * 255.0) as u8).collect();

    let mut acc = 0u32;
    let half = (n / 2.0) as u32;
    let mut median = 0.5;
    for (i, c) in hl.iter().enumerate() {
        acc += c;
        if acc >= half {
            median = i as f32 / (HIST_BINS - 1) as f32;
            break;
        }
    }
    let clip_high = high as f32 / n * 100.0;
    let clip_low = low as f32 / n * 100.0;
    // Judge exposure by the midtones. Clipping is reported separately: a black background
    // or a white sky can be clipped on purpose in a well-exposed photo.
    let exposure = if median < 0.22 || (median < 0.3 && clip_low > 15.0) {
        "under"
    } else if median > 0.76 || (median > 0.62 && clip_high > 6.0) {
        "over"
    } else {
        "good"
    };
    Scopes { hist_r: hr, hist_g: hg, hist_b: hb, hist_l: hl, clip_high, clip_low, median, exposure, wheel, bands: band_stats(&px) }
}

/// Transparent overlay: red where highlights are blown, blue where shadows are crushed.
pub fn clipping_overlay(photo: &Photo) -> image::RgbaImage {
    let mut img = image::RgbaImage::new(photo.width, photo.height);
    for (i, p) in photo.data.iter().enumerate() {
        let m = max3(*p);
        let px = if m >= 0.995 {
            [255, 40, 40, 220]
        } else if m <= 0.01 {
            [40, 120, 255, 220]
        } else {
            [0, 0, 0, 0]
        };
        let (x, y) = (i as u32 % photo.width, i as u32 / photo.width);
        img.put_pixel(x, y, image::Rgba(px));
    }
    img
}
