//! Small colour-math helpers. All "display" values are sRGB-encoded (gamma) in 0..1,
//! "linear" values are scene-linear sRGB primaries.

pub type Rgb = [f32; 3];

/// Rec.709 / sRGB luma weights.
pub const LUMA: Rgb = [0.2126, 0.7152, 0.0722];

#[inline]
pub fn to_linear(v: f32) -> f32 {
    if v <= 0.040_45 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

/// Linear -> sRGB encoding. Works for values above 1.0 too (extended range),
/// which lets the tone curve roll bright values off smoothly instead of clipping.
#[inline]
pub fn to_display(v: f32) -> f32 {
    if v <= 0.003_130_8 {
        v.max(0.0) * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

#[inline]
pub fn luma(c: Rgb) -> f32 {
    c[0] * LUMA[0] + c[1] * LUMA[1] + c[2] * LUMA[2]
}

#[inline]
pub fn max3(c: Rgb) -> f32 {
    c[0].max(c[1]).max(c[2])
}

#[inline]
pub fn min3(c: Rgb) -> f32 {
    c[0].min(c[1]).min(c[2])
}

/// HSV-style saturation in 0..1.
#[inline]
pub fn saturation(c: Rgb) -> f32 {
    let m = max3(c);
    if m <= 1e-5 {
        0.0
    } else {
        ((m - min3(c)) / m).clamp(0.0, 1.0)
    }
}

/// Hue in degrees 0..360 (0 = red). Returns 0 for greys.
#[inline]
pub fn hue(c: Rgb) -> f32 {
    let (r, g, b) = (c[0], c[1], c[2]);
    let mx = max3(c);
    let mn = min3(c);
    let d = mx - mn;
    if d <= 1e-6 {
        return 0.0;
    }
    let h = if mx == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if mx == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    h * 60.0
}

/// Smooth 0..1 weight for being inside a hue band [lo, hi] (degrees, may wrap) with soft edges.
#[inline]
pub fn hue_band(h: f32, lo: f32, hi: f32, soft: f32) -> f32 {
    // distance outside the band, handling wrap-around
    let inside = if lo <= hi { h >= lo && h <= hi } else { h >= lo || h <= hi };
    if inside {
        return 1.0;
    }
    let dist = |a: f32, b: f32| {
        let d = (a - b).abs() % 360.0;
        d.min(360.0 - d)
    };
    let d = dist(h, lo).min(dist(h, hi));
    (1.0 - d / soft).clamp(0.0, 1.0)
}

/// How much a pixel looks like skin (0..1). Skin sits in a narrow orange-red hue band
/// with moderate saturation, across all skin tones (they differ mostly in brightness).
#[inline]
pub fn skin_weight(c: Rgb) -> f32 {
    let v = max3(c);
    if v < 0.12 || !(c[0] >= c[1] && c[1] >= c[2] * 0.9) {
        return 0.0;
    }
    let s = saturation(c);
    let h = hue(c);
    let hw = hue_band(h, 8.0, 40.0, 12.0);
    let sw = if s < 0.12 {
        0.0
    } else if s < 0.2 {
        (s - 0.12) / 0.08
    } else if s <= 0.62 {
        1.0
    } else {
        (1.0 - (s - 0.62) / 0.15).max(0.0)
    };
    hw * sw
}

/// A fully-saturated colour of the given hue with its luma removed: adding it to a pixel
/// tints the pixel without changing its brightness.
pub fn tint_vector(hue_deg: f32) -> Rgb {
    let h = (hue_deg.rem_euclid(360.0)) / 60.0;
    let x = 1.0 - ((h % 2.0) - 1.0).abs();
    let c = match h as i32 {
        0 => [1.0, x, 0.0],
        1 => [x, 1.0, 0.0],
        2 => [0.0, 1.0, x],
        3 => [0.0, x, 1.0],
        4 => [x, 0.0, 1.0],
        _ => [1.0, 0.0, x],
    };
    let y = luma(c);
    [c[0] - y, c[1] - y, c[2] - y]
}
