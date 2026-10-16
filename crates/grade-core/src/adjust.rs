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
    /// Colour mixer, one entry per band (Red, Orange, Yellow, Green, Aqua, Blue, Purple, Magenta).
    pub hue: [f32; 8],
    pub sat: [f32; 8],
    pub lum: [f32; 8],
}

/// Everything that decides how one photo is graded.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub look: String,
    /// 0..2 (1 = the look as designed).
    pub strength: f32,
    pub adjust: Adjust,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { look: "natural".into(), strength: 1.0, adjust: Adjust::default() }
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
    pub fn band_hue_deg(&self, i: usize) -> f32 { self.hue[i] * 0.3 } // ±30°
    pub fn band_sat(&self, i: usize) -> f32 { self.sat[i] / 100.0 } // ×(1 ± 1)
    pub fn band_lum(&self, i: usize) -> f32 { self.lum[i] / 400.0 } // ±0.25
}
