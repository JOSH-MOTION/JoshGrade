//! A truthful before → after report: the original and the current grade are measured with
//! the same rules, so every line shows what really changed with *these* settings.

use crate::color::*;
use crate::scopes::band_stats;
use crate::Photo;
use serde::Serialize;

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Measure {
    /// Median brightness (midtones), 0..100.
    pub midtones: f32,
    /// Spread of brightness, 0..100 (higher = more contrast).
    pub contrast: f32,
    pub blown: f32,
    pub crushed: f32,
    /// Colour cast of things that should be neutral: + warm / − cool, + green / − magenta.
    pub warmth: Option<f32>,
    pub green: Option<f32>,
    /// Average colour intensity, 0..100.
    pub colour: f32,
    /// Colours that are so strong they've lost detail.
    pub losing_detail: usize,
    /// Skin colour intensity 0..100 (None if no skin found).
    pub skin: Option<f32>,
}

pub fn measure(photo: &Photo) -> Measure {
    let step = (photo.data.len() / 200_000).max(1);
    let px: Vec<Rgb> = photo.data.iter().step_by(step).copied().collect();
    let n = px.len().max(1) as f32;

    let mut ys: Vec<f32> = px.iter().map(|p| to_display(luma(p.map(to_linear)))).collect();
    ys.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let median = ys[ys.len() / 2];
    let mean = ys.iter().sum::<f32>() / n;
    let sd = (ys.iter().map(|y| (y - mean) * (y - mean)).sum::<f32>() / n).sqrt();

    let blown = px.iter().filter(|p| max3(**p) >= 0.995).count() as f32 / n * 100.0;
    let crushed = px.iter().filter(|p| max3(**p) <= 0.01).count() as f32 / n * 100.0;

    // Colour cast from near-neutral, non-skin pixels (white walls, grey backdrops, clothes).
    let (mut nr, mut ng, mut nb, mut nw) = (0f64, 0f64, 0f64, 0f64);
    let (mut sat_sum, mut sat_n) = (0f32, 0f32);
    let (mut skin_s, mut skin_w) = (0f32, 0f32);
    for p in &px {
        let v = max3(*p);
        let s = saturation(*p);
        let sk = skin_weight(*p);
        if (0.15..0.95).contains(&v) && s < 0.22 && sk < 0.3 {
            let w = (1.0 - s / 0.22) as f64;
            nr += to_linear(p[0]) as f64 * w;
            ng += to_linear(p[1]) as f64 * w;
            nb += to_linear(p[2]) as f64 * w;
            nw += w;
        }
        if v > 0.12 {
            sat_sum += s;
            sat_n += 1.0;
        }
        if sk > 0.3 {
            skin_s += s * sk;
            skin_w += sk;
        }
    }
    let (warmth, green) = if nw / n as f64 > 0.01 && nb > 0.0 && nr > 0.0 {
        let (r, g, b) = ((nr / nw) as f32, (ng / nw) as f32, (nb / nw) as f32);
        (Some((r / b).ln() * 100.0), Some((g / ((r + b) * 0.5)).ln() * 100.0))
    } else {
        (None, None)
    };
    let skin = if skin_w / n > 0.02 { Some(skin_s / skin_w * 100.0) } else { None };

    Measure {
        midtones: median * 100.0,
        contrast: sd * 100.0,
        blown,
        crushed,
        warmth,
        green,
        colour: if sat_n > 0.0 { sat_s(sat_sum, sat_n) } else { 0.0 },
        losing_detail: band_stats(&px).iter().filter(|b| b.over).count(),
        skin,
    }
}

fn sat_s(sum: f32, n: f32) -> f32 {
    sum / n * 100.0
}

#[derive(Debug, Clone, Serialize)]
pub struct Line {
    /// Stable id the app uses for its explanation tooltip.
    pub key: &'static str,
    pub label: &'static str,
    pub before: String,
    pub after: String,
    /// Plain-language verdict on the graded photo.
    pub verdict: String,
    /// "good", "warn" or "info".
    pub status: &'static str,
}

fn exposure_verdict(m: f32) -> (&'static str, &'static str) {
    match m {
        x if x < 25.0 => ("too dark", "warn"),
        x if x < 36.0 => ("a little dark", "info"),
        x if x <= 60.0 => ("balanced", "good"),
        x if x <= 70.0 => ("a little bright", "info"),
        _ => ("too bright", "warn"),
    }
}

fn cast_words(w: f32, g: f32) -> String {
    let mut parts = Vec::new();
    if w > 6.0 {
        parts.push(if w > 15.0 { "very warm" } else { "warm" });
    } else if w < -6.0 {
        parts.push(if w < -15.0 { "very cool / blue" } else { "cool / blue" });
    }
    if g > 5.0 {
        parts.push("green");
    } else if g < -5.0 {
        parts.push("magenta");
    }
    if parts.is_empty() {
        "neutral".into()
    } else {
        parts.join(", ")
    }
}

/// Compare the original with the graded version.
pub fn compare(before: &Measure, after: &Measure) -> Vec<Line> {
    let mut lines = Vec::new();
    let pct = |v: f32| format!("{:.0}%", v);
    let pct1 = |v: f32| if v < 10.0 { format!("{:.1}%", v) } else { format!("{:.0}%", v) };

    let (ev, es) = exposure_verdict(after.midtones);
    lines.push(Line { key: "exposure", label: "Exposure (midtones)", before: pct(before.midtones), after: pct(after.midtones), verdict: ev.into(), status: es });

    lines.push(Line {
        key: "blown",
        label: "Blown highlights",
        before: pct1(before.blown),
        after: pct1(after.blown),
        verdict: if after.blown < 0.5 { "none".into() } else if after.blown < 3.0 { "small areas".into() } else { "large areas lost".into() },
        status: if after.blown < 0.5 { "good" } else if after.blown < 3.0 { "info" } else { "warn" },
    });

    lines.push(Line {
        key: "crushed",
        label: "Crushed shadows",
        before: pct1(before.crushed),
        after: pct1(after.crushed),
        verdict: if after.crushed < 1.0 { "none".into() } else if after.crushed < 6.0 { "a little (fine for deep blacks)".into() } else { "large areas pure black (fine for a black background)".into() },
        status: if after.crushed < 1.0 { "good" } else if after.crushed < 6.0 { "info" } else { "warn" },
    });

    if let (Some(bw), Some(bg), Some(aw), Some(ag)) = (before.warmth, before.green, after.warmth, after.green) {
        let neutral = aw.abs() <= 6.0 && ag.abs() <= 5.0;
        lines.push(Line {
            key: "cast",
            label: "Colour cast",
            before: cast_words(bw, bg),
            after: cast_words(aw, ag),
            verdict: if neutral { "whites are white".into() } else { "tinted (may be your look)".into() },
            status: if neutral { "good" } else { "info" },
        });
    }

    let c = after.contrast;
    lines.push(Line {
        key: "contrast",
        label: "Contrast",
        before: format!("{:.0}", before.contrast),
        after: format!("{:.0}", c),
        verdict: if c < 15.0 { "flat".into() } else if c <= 30.0 { "healthy".into() } else { "very punchy".into() },
        status: if (15.0..=30.0).contains(&c) { "good" } else { "info" },
    });

    let col = after.colour;
    lines.push(Line {
        key: "colour",
        label: "Colour intensity",
        before: pct(before.colour),
        after: pct(col),
        verdict: if col < 12.0 { "muted".into() } else if col <= 45.0 { "natural".into() } else { "very rich".into() },
        status: if (12.0..=45.0).contains(&col) { "good" } else { "info" },
    });

    lines.push(Line {
        key: "detail",
        label: "Colours losing detail",
        before: before.losing_detail.to_string(),
        after: after.losing_detail.to_string(),
        verdict: if after.losing_detail == 0 { "none".into() } else { "see colour wheel".into() },
        status: if after.losing_detail == 0 { "good" } else { "warn" },
    });

    if let (Some(bs), Some(a_s)) = (before.skin, after.skin) {
        lines.push(Line {
            key: "skin",
            label: "Skin colour",
            before: pct(bs),
            after: pct(a_s),
            verdict: if a_s > 58.0 { "too orange / red".into() } else if a_s < 18.0 { "a bit grey".into() } else { "natural".into() },
            status: if (18.0..=58.0).contains(&a_s) { "good" } else { "warn" },
        });
    }
    lines
}
