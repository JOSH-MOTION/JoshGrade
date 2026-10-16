//! Creative "looks" applied on top of the automatic correction.
//! Every look is just a handful of numbers, so new ones are easy to add.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
pub struct LookParams {
    /// Extra exposure in stops.
    pub ev: f32,
    /// + warmer / - cooler.
    pub temp: f32,
    /// + magenta / - green.
    pub tint: f32,
    /// Extra contrast (-0.3..0.3).
    pub contrast: f32,
    /// Extra shadow lift (0..0.3).
    pub shadows: f32,
    /// Lifted ("faded") blacks, output level of pure black (0..0.08).
    pub fade: f32,
    /// Output level of pure white (0.9..1.0); below 1 gives a soft, filmic top end.
    pub roll: f32,
    /// Global saturation change (-1 = black & white).
    pub sat: f32,
    /// Extra vibrance (boosts muted colours more than vivid ones).
    pub vib: f32,
    /// Desaturate greens (0..1) – common in cinematic grades.
    pub green_desat: f32,
    /// Shadow tint hue (degrees) and strength.
    pub sh_hue: f32,
    pub sh_amt: f32,
    /// Highlight tint hue (degrees) and strength.
    pub hl_hue: f32,
    pub hl_amt: f32,
}

#[derive(Debug, Clone, Serialize)]
pub struct Look {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub params: LookParams,
}

const BASE: LookParams = LookParams {
    ev: 0.0, temp: 0.0, tint: 0.0, contrast: 0.0, shadows: 0.0, fade: 0.0, roll: 1.0,
    sat: 0.0, vib: 0.0, green_desat: 0.0, sh_hue: 0.0, sh_amt: 0.0, hl_hue: 0.0, hl_amt: 0.0,
};

pub fn all_looks() -> Vec<Look> {
    vec![
        Look {
            id: "normal",
            name: "Normal",
            description: "Only JoshGrade's automatic correction: right exposure, true whites, balanced colour. No styling added.",
            params: BASE,
        },
        Look {
            id: "natural",
            name: "Natural",
            description: "Accurate correction with a gentle professional finish: true whites, balanced exposure, a touch of contrast and colour.",
            params: LookParams { contrast: 0.06, vib: 0.1, ..BASE },
        },
        Look {
            id: "vivid",
            name: "Vivid",
            description: "Rich, punchy colour and contrast. Great for fashion, events, food and travel. Skin stays natural.",
            params: LookParams { contrast: 0.16, shadows: 0.04, vib: 0.32, sat: 0.06, ..BASE },
        },
        Look {
            id: "warm",
            name: "Warm",
            description: "Golden, inviting tones. Great for portraits, weddings and lifestyle.",
            params: LookParams {
                temp: 0.09, tint: 0.01, contrast: 0.06, vib: 0.12,
                sh_hue: 22.0, sh_amt: 0.04, hl_hue: 38.0, hl_amt: 0.09, ..BASE
            },
        },
        Look {
            id: "cinematic",
            name: "Cinematic",
            description: "Teal shadows, warm highlights, punchy contrast, muted greens. Skin stays natural.",
            params: LookParams {
                temp: 0.02, contrast: 0.2, fade: 0.02, sat: -0.06, green_desat: 0.5,
                sh_hue: 195.0, sh_amt: 0.2, hl_hue: 32.0, hl_amt: 0.14, ..BASE
            },
        },
        Look {
            id: "moody",
            name: "Moody",
            description: "Darker, desaturated and cool with soft faded blacks.",
            params: LookParams {
                ev: -0.4, temp: -0.03, contrast: 0.14, fade: 0.06, roll: 0.95, sat: -0.28,
                green_desat: 0.4, sh_hue: 210.0, sh_amt: 0.11, ..BASE
            },
        },
        Look {
            id: "airy",
            name: "Bright & Airy",
            description: "Light, soft and clean. Lifted shadows, gentle contrast, soft pastel colour.",
            params: LookParams {
                ev: 0.4, temp: -0.015, contrast: -0.15, shadows: 0.22, fade: 0.03, sat: -0.08,
                vib: 0.05, hl_hue: 340.0, hl_amt: 0.03, ..BASE
            },
        },
        Look {
            id: "film",
            name: "Film",
            description: "Analog feel: faded blacks, soft highlights, warm tones, green-tinted shadows.",
            params: LookParams {
                temp: 0.04, contrast: 0.08, fade: 0.08, roll: 0.96, sat: -0.12,
                sh_hue: 165.0, sh_amt: 0.09, hl_hue: 42.0, hl_amt: 0.1, ..BASE
            },
        },
        Look {
            id: "bw",
            name: "Black & White",
            description: "Rich monochrome with a full tonal range.",
            params: LookParams { contrast: 0.14, sat: -1.0, ..BASE },
        },
    ]
}

pub fn find_look(id: &str) -> Option<Look> {
    all_looks().into_iter().find(|l| l.id.eq_ignore_ascii_case(id))
}
