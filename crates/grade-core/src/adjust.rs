//! The user's own fine-tuning, layered on top of the automatic correction.
//! Every value uses the familiar -100..+100 slider range (0 = no change).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct Adjust {
    pub exposure: f32,
    pub contrast: f32,
    pub highlights: f32,
    pub shadows: f32,
    pub temp: f32,
    pub tint: f32,
    pub vibrance: f32,
    pub saturation: f32,
    pub whites: f32,
    pub blacks: f32,
    /// Colour mixer, one entry per band (Red, Orange, Yellow, Green, Aqua, Blue, Purple, Magenta).
    pub hue: [f32; 8],
    pub sat: [f32; 8],
    pub lum: [f32; 8],
    /// Colour grading wheels (like Lightroom's Color Grading / Capture One's Color Balance).
    pub grading: Grading,
    /// Parametric tone curve: Highlights, Lights, Darks, Shadows (−100..100).
    pub param: [f32; 4],
    /// Calibration: shift the camera's red / green / blue primaries, and shadow tint.
    pub calibration: Calibration,
}

/// One colour wheel: hue (0..360°), saturation (0..100) and luminance (−100..100).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct Wheel {
    pub hue: f32,
    pub sat: f32,
    pub lum: f32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Grading {
    pub shadows: Wheel,
    pub midtones: Wheel,
    pub highlights: Wheel,
    pub global: Wheel,
    /// How much the three ranges overlap (0..100, default 50).
    pub blending: f32,
    /// Moves the split between shadows and highlights (−100..100).
    pub balance: f32,
}

impl Default for Grading {
    fn default() -> Self {
        Grading { shadows: Wheel::default(), midtones: Wheel::default(), highlights: Wheel::default(), global: Wheel::default(), blending: 50.0, balance: 0.0 }
    }
}

impl Grading {
    pub fn active(&self) -> bool {
        [self.shadows, self.midtones, self.highlights, self.global].iter().any(|w| w.sat.abs() > 0.01 || w.lum.abs() > 0.01)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct Calibration {
    /// Shadows tint: − green, + magenta.
    pub shadows_tint: f32,
    pub red_hue: f32,
    pub red_sat: f32,
    pub green_hue: f32,
    pub green_sat: f32,
    pub blue_hue: f32,
    pub blue_sat: f32,
}

impl Calibration {
    pub fn active(&self) -> bool {
        *self != Calibration::default()
    }
}

/// Everything that decides how one photo is graded.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    /// Apply JoshGrade's automatic correction (exposure, white balance, contrast, colour).
    pub auto: bool,
    pub look: String,
    /// 0..2 (1 = the look as designed).
    pub strength: f32,
    pub adjust: Adjust,
    /// Automatic portrait retouch (off unless turned on).
    pub retouch: crate::RetouchSettings,
    /// Crop, rotate, straighten.
    pub crop: crate::geometry::Crop,
    /// Point curves.
    pub curves: crate::curves::Curves,
    /// Sharpening, noise reduction, clarity, texture, dehaze, vignette, grain.
    pub detail: crate::detail::Detail,
    /// Local adjustments.
    pub masks: Vec<crate::masks::Mask>,
    /// Lens corrections and Transform.
    pub lens: crate::lens::Lens,
    /// Spot removal and red eye.
    pub spots: Vec<crate::spots::Spot>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { auto: true, look: "natural".into(), strength: 1.0, adjust: Adjust::default(), retouch: Default::default(), crop: Default::default(), curves: Default::default(), detail: Default::default(), masks: vec![], lens: Default::default(), spots: vec![] }
    }
}

// Conversions from slider units to engine units.
impl Adjust {
    pub fn ev(&self) -> f32 { self.exposure / 50.0 } // ±2 stops
    pub fn contrast_amt(&self) -> f32 { self.contrast / 250.0 } // ±0.4
    pub fn highlights_amt(&self) -> f32 { self.highlights / 200.0 } // ±0.5
    pub fn shadows_amt(&self) -> f32 { self.shadows / 250.0 } // ±0.4
    pub fn temp_amt(&self) -> f32 { self.temp / 330.0 } // ±0.3
    pub fn tint_amt(&self) -> f32 { self.tint / 250.0 } // ±0.4
    pub fn vibrance_amt(&self) -> f32 { self.vibrance / 160.0 } // ±0.6
    pub fn saturation_amt(&self) -> f32 { self.saturation / 100.0 } // ±1 (−100 = mono)
    pub fn whites_amt(&self) -> f32 { self.whites / 400.0 } // ±0.25
    pub fn blacks_amt(&self) -> f32 { self.blacks / 400.0 } // ±0.25
    pub fn band_hue_deg(&self, i: usize) -> f32 { self.hue[i] * 0.3 } // ±30°
    pub fn band_sat(&self, i: usize) -> f32 { self.sat[i] / 100.0 } // ×(1 ± 1)
    pub fn band_lum(&self, i: usize) -> f32 { self.lum[i] / 400.0 } // ±0.25
}

/// The automatic correction expressed in slider units, so the app can show what JoshGrade
/// chose (e.g. Exposure −35 = it pulled exposure down 0.7 stops). Your edits are added on top.
pub fn auto_sliders(p: &crate::AutoParams) -> Adjust {
    // White balance gains -> temperature / tint (exact inverse of how Grade applies them).
    let rg = p.wb_gains[0] / p.wb_gains[1];
    let bg = p.wb_gains[2] / p.wb_gains[1];
    let k = 2.0 / (rg + bg);
    let tint = 1.0 - k;
    let temp = rg * k - 1.0;
    let r = |v: f32| (v * 10.0).round() / 10.0;
    Adjust {
        exposure: r(p.exposure_ev * 50.0),
        contrast: r(p.contrast * 250.0),
        highlights: r(-((0.88 - p.highlight_knee).max(0.0) / 0.2 * 50.0)),
        shadows: r(p.shadows * 250.0),
        temp: r(temp * 330.0),
        tint: r(tint * 250.0),
        vibrance: r(p.vibrance * 160.0),
        saturation: r(p.saturation * 100.0),
        // the automatic black / white points show on Blacks / Whites
        whites: r((1.0 - p.white) * 400.0),
        blacks: r(-p.black * 400.0),
        hue: [0.0; 8],
        sat: p.hsl_sat.map(|v| r((v - 1.0) * 100.0)),
        lum: p.hsl_lum.map(|v| r(v * 400.0)),
        ..Default::default()
    }
}
