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
            id: "natural",
            name: "Natural",
            description: "Clean, accurate correction only. True whites, balanced exposure, honest colour.",
            params: BASE,
        },
        Look {
            id: "warm",
            name: "Warm",
            description: "Golden, inviting tones. Great for portraits, weddings and lifestyle.",
            params: LookParams { temp: 0.05, vib: 0.06, hl_hue: 36.0, hl_amt: 0.05, ..BASE },
        },
        Look {
            id: "cinematic",
            name: "Cinematic",
            description: "Teal shadows, warm highlights, punchy contrast, muted greens. Skin stays natural.",
            params: LookParams {
                contrast: 0.12, fade: 0.015, sat: -0.05, green_desat: 0.35,
                sh_hue: 192.0, sh_amt: 0.11, hl_hue: 30.0, hl_amt: 0.08, ..BASE
            },
        },
        Look {
            id: "moody",
            name: "Moody",
            description: "Darker, desaturated and slightly cool with soft faded blacks.",
            params: LookParams {
                ev: -0.3, temp: -0.02, contrast: 0.08, fade: 0.045, roll: 0.96, sat: -0.2,
                green_desat: 0.3, sh_hue: 215.0, sh_amt: 0.06, ..BASE
            },
        },
        Look {
            id: "airy",
            name: "Bright & Airy",
            description: "Light, soft and clean. Lifted shadows, gentle contrast, soft pastel colour.",
            params: LookParams {
                ev: 0.3, temp: -0.01, contrast: -0.12, shadows: 0.15, fade: 0.025, sat: -0.06,
                hl_hue: 340.0, hl_amt: 0.02, ..BASE
            },
        },
        Look {
            id: "film",
            name: "Film",
            description: "Analog feel: faded blacks, soft highlights, warm tones, gently green shadows.",
            params: LookParams {
                temp: 0.025, contrast: 0.05, fade: 0.06, roll: 0.97, sat: -0.08,
                sh_hue: 160.0, sh_amt: 0.05, hl_hue: 40.0, hl_amt: 0.06, ..BASE
            },
        },
        Look {
            id: "bw",
            name: "Black & White",
            description: "Rich monochrome with a full tonal range.",
            params: LookParams { contrast: 0.1, sat: -1.0, ..BASE },
        },
    ]
}

pub fn find_look(id: &str) -> Option<Look> {
    all_looks().into_iter().find(|l| l.id.eq_ignore_ascii_case(id))
}
