//! Turns the automatic corrections + a look + your adjustments + a strength into a single
//! per-pixel colour transform. Because it's purely per-pixel, the exact same transform
//! can be exported as a 3D LUT (.cube) for Photoshop / Lightroom / Premiere / DaVinci.

use crate::adjust::Adjust;
use crate::analyze::AutoParams;
use crate::color::*;
use crate::looks::LookParams;
use crate::Photo;
use rayon::prelude::*;

const CURVE_MAX: f32 = 3.0;
const CURVE_N: usize = 4096;

#[derive(Debug, Clone)]
pub struct Grade {
    gain: Rgb,
    curve: Vec<f32>,
    vibrance: f32,
    sat: f32,
    mono: bool,
    green_desat: f32,
    band_sat: [f32; 8],
    band_lum: [f32; 8],
    band_hue: [f32; 8],
    any_hue: bool,
    sh_tint: Rgb,
    hl_tint: Rgb,
    fade: f32,
    roll: f32,
    strength: f32,
    /// Your point curves (master, R, G, B) as lookup tables; None = straight.
    user: Option<Box<crate::curves::Tables>>,
    /// Colour grading wheels.
    grading: Option<crate::adjust::Grading>,
    /// Calibration: primaries matrix (linear light) and shadow tint.
    calib: Option<[[f32; 3]; 3]>,
    shadow_tint: f32,
}

impl Grade {
    /// Automatic correction + look only (no manual adjustments).
    pub fn new(auto: &AutoParams, look: &LookParams, strength: f32) -> Self {
        Self::with_adjust(auto, look, &Adjust::default(), strength)
    }

    pub fn with_adjust(auto: &AutoParams, look: &LookParams, adj: &Adjust, strength: f32) -> Self {
        let ev = auto.exposure_ev + look.ev + adj.ev();
        let e = 2f32.powf(ev);
        let temp = look.temp + adj.temp_amt();
        let tint = look.tint + adj.tint_amt();
        let gain = [
            auto.wb_gains[0] * (1.0 + temp) * e,
            auto.wb_gains[1] * (1.0 - tint) * e,
            auto.wb_gains[2] * (1.0 - temp) * e,
        ];
        let black = auto.black;
        let white = auto.white.max(black + 0.2);
        let shadows = (auto.shadows + look.shadows + adj.shadows_amt()).clamp(-0.4, 0.6);
        let contrast = (auto.contrast + look.contrast + adj.contrast_amt()).clamp(-0.5, 0.6);
        let highlights = adj.highlights_amt();
        let (whites, blacks) = (adj.whites_amt(), adj.blacks_amt());
        let param = adj.param.map(|v| v / 100.0 * 0.12);
        // Pulling highlights down also moves the shoulder lower for a gentler roll-off.
        let knee = (auto.highlight_knee + highlights.min(0.0) * 0.4).clamp(0.5, 0.95);

        let curve = (0..CURVE_N)
            .map(|i| {
                let x0 = i as f32 / (CURVE_N - 1) as f32 * CURVE_MAX;
                // 1. levels
                let mut x = ((x0 - black) / (white - black)).max(0.0);
                if x <= 1.0 {
                    // 2. shadows (darks most, fades out by the midtones)
                    x += shadows * 2.5 * x * (1.0 - x).powi(3);
                    // 3. highlights (brights most, fades out by the midtones)
                    x += highlights * 2.5 * x.powi(3) * (1.0 - x);
                    // 4. contrast S-curve pivoting on mid-grey
                    x += contrast * 4.0 * (x - x * x) * (x - 0.5);
                    // 5. whites / blacks: move the ends of the range
                    x += whites * 1.6 * x.powi(4) + blacks * 1.6 * (1.0 - x).powi(4);
                    // 6. parametric curve regions (Highlights, Lights, Darks, Shadows)
                    for (k, c) in [0.875f32, 0.625, 0.375, 0.125].iter().enumerate() {
                        if param[k] != 0.0 {
                            let d = ((x - c) / 0.25).abs();
                            if d < 1.0 {
                                x += param[k] * 0.5 * (1.0 + (std::f32::consts::PI * d).cos()) * (x * (1.0 - x) * 4.0).sqrt();
                            }
                        }
                    }
                    x = x.max(0.0);
                }
                // 5. highlight shoulder – smooth roll-off instead of clipping
                if x > knee {
                    x = knee + (1.0 - knee) * ((x - knee) / (1.0 - knee)).tanh();
                }
                x.clamp(0.0, 1.0)
            })
            .collect();

        let mut band_sat = [1.0f32; 8];
        let mut band_lum = [0.0f32; 8];
        let mut band_hue = [0.0f32; 8];
        for i in 0..8 {
            band_sat[i] = (auto.hsl_sat[i] * (1.0 + adj.band_sat(i))).max(0.0);
            band_lum[i] = auto.hsl_lum[i] + adj.band_lum(i);
            band_hue[i] = adj.band_hue_deg(i);
        }
        let sat = look.sat + adj.saturation_amt() + auto.saturation;

        Grade {
            gain,
            curve,
            vibrance: auto.vibrance + look.vib + adj.vibrance_amt(),
            sat,
            mono: sat <= -0.999,
            green_desat: look.green_desat,
            band_sat,
            band_lum,
            band_hue,
            any_hue: band_hue.iter().any(|h| h.abs() > 0.01),
            sh_tint: tint_vector(look.sh_hue).map(|v| v * look.sh_amt),
            hl_tint: tint_vector(look.hl_hue).map(|v| v * look.hl_amt),
            fade: look.fade,
            roll: if look.roll <= 0.0 { 1.0 } else { look.roll },
            strength: strength.clamp(0.0, 2.0),
            user: None,
            grading: adj.grading.active().then_some(adj.grading),
            calib: adj.calibration.active().then(|| calibration_matrix(&adj.calibration)),
            shadow_tint: adj.calibration.shadows_tint / 100.0,
        }
    }

    /// Add your point curves (applied after everything else in the grade).
    pub fn with_curves(mut self, c: &crate::curves::Curves) -> Self {
        self.user = c.tables().map(Box::new);
        self
    }

    #[inline]
    fn curve_at(&self, x: f32) -> f32 {
        let f = (x.max(0.0) / CURVE_MAX * (CURVE_N - 1) as f32).min((CURVE_N - 1) as f32);
        let i = f as usize;
        if i >= CURVE_N - 1 {
            return self.curve[CURVE_N - 1];
        }
        let t = f - i as f32;
        self.curve[i] * (1.0 - t) + self.curve[i + 1] * t
    }

    /// Grade one display-encoded sRGB pixel.
    #[inline]
    pub fn apply_px(&self, inp: Rgb) -> Rgb {
        // White balance + exposure happen in linear light, like a camera would.
        let mut lin = [
            to_linear(inp[0]) * self.gain[0],
            to_linear(inp[1]) * self.gain[1],
            to_linear(inp[2]) * self.gain[2],
        ];
        if let Some(m) = &self.calib {
            lin = [0, 1, 2].map(|r| (m[r][0] * lin[0] + m[r][1] * lin[1] + m[r][2] * lin[2]).max(0.0));
        }
        let d = lin.map(to_display);

        // Tone curve: half per-channel (natural, adds a little punch) and half
        // hue-preserving (keeps colours from shifting when brightness changes a lot).
        let per = d.map(|v| self.curve_at(v));
        let m = max3(d);
        let keep = if m > 1e-5 {
            let k = self.curve_at(m) / m;
            d.map(|v| v * k)
        } else {
            per
        };
        let mut c = [0, 1, 2].map(|i| 0.5 * per[i] + 0.5 * keep[i]);

        if self.mono {
            let y = luma(c);
            c = [y, y, y];
        } else {
            let s = saturation(c);
            let skin = skin_weight(c);

            // Colour mixer (auto fixes for over-strong colours + your HSL sliders).
            if s > 0.0 {
                let w = band_weights(hue(c));
                let mut bs = 0.0;
                let mut bl = 0.0;
                let mut bh = 0.0;
                for i in 0..8 {
                    if w[i] > 0.0 {
                        bs += w[i] * self.band_sat[i];
                        bl += w[i] * self.band_lum[i];
                        bh += w[i] * self.band_hue[i];
                    }
                }
                if self.any_hue && bh.abs() > 0.01 {
                    let hsv = rgb_to_hsv(c);
                    c = hsv_to_rgb(hsv[0] + bh * s.min(0.4) * 2.5, hsv[1], hsv[2]);
                }
                let y = luma(c);
                // Fade the effect in with saturation, so greys are never tinted.
                let sw = (s * 3.0).min(1.0);
                let f = 1.0 + (bs - 1.0) * sw;
                c = c.map(|v| y + (v - y) * f);
                let lf = 1.0 + bl * sw;
                c = c.map(|v| v * lf);
            }

            // Vibrance + saturation + creative green desaturation, protecting skin.
            let y = luma(c);
            let s = saturation(c);
            let mut factor = 1.0 + self.vibrance * (1.0 - s) + self.sat;
            if self.green_desat > 0.0 {
                let g = hue_band(hue(c), 75.0, 165.0, 25.0) * s.min(0.5) * 2.0;
                factor -= self.green_desat * g;
            }
            // skin gets at most 35% of any saturation change from looks / vibrance
            factor = 1.0 + (factor - 1.0) * (1.0 - 0.65 * skin);
            // never let skin become orange
            if skin > 0.0 && s > 0.55 {
                factor -= skin * (s - 0.55) * 0.6;
            }
            let factor = factor.max(0.0);
            c = c.map(|v| y + (v - y) * factor);

            // Split toning, kept gentle on skin.
            let tint_scale = 1.0 - 0.5 * skin;
            let ws = (1.0 - y).clamp(0.0, 1.0).powi(2) * tint_scale;
            let wh = y.clamp(0.0, 1.0).powi(2) * tint_scale;
            for i in 0..3 {
                c[i] += self.sh_tint[i] * ws + self.hl_tint[i] * wh;
            }

            // Colour grading wheels: tint (and brighten / darken) shadows, midtones,
            // highlights and everything, with soft overlaps.
            if let Some(g) = &self.grading {
                let y = luma(c).clamp(0.0, 1.0);
                let p = 0.5 - 0.3 * g.balance / 100.0;
                let wdt = 0.08 + 0.32 * g.blending / 100.0;
                let ss = |a: f32, b: f32, x: f32| {
                    let t = ((x - a) / (b - a).max(1e-4)).clamp(0.0, 1.0);
                    t * t * (3.0 - 2.0 * t)
                };
                let w_s = 1.0 - ss(p - 2.0 * wdt, p, y);
                let w_h = ss(p, p + 2.0 * wdt, y);
                let w_m = (1.0 - w_s - w_h).max(0.0);
                let protect = 1.0 - 0.3 * skin;
                for (w, wh) in [(w_s, g.shadows), (w_m, g.midtones), (w_h, g.highlights), (1.0, g.global)] {
                    if w <= 0.0 || (wh.sat == 0.0 && wh.lum == 0.0) {
                        continue;
                    }
                    let tv = tint_vector(wh.hue);
                    let amt = wh.sat / 100.0 * 0.22 * w * protect;
                    let lum = wh.lum / 100.0 * 0.25 * w;
                    for i in 0..3 {
                        c[i] += tv[i] * amt + lum * (1.0 - (c[i] - 0.5).abs());
                    }
                }
            }
            // Calibration: shadows tint (− green, + magenta)
            if self.shadow_tint != 0.0 {
                let y = luma(c).clamp(0.0, 1.0);
                let w = (1.0 - y).powi(2) * self.shadow_tint * 0.08;
                c[0] += w * 0.5;
                c[1] -= w;
                c[2] += w * 0.5;
            }
        }

        if let Some(t) = &self.user {
            c = t.apply(c.map(|v| v.clamp(0.0, 1.0)));
        }
        // Film-style output range (faded blacks / soft whites).
        let out = c.map(|v| (self.fade + v.clamp(0.0, 1.0) * (self.roll - self.fade)).clamp(0.0, 1.0));

        // Strength: blend with the original (above 100% exaggerates the grade).
        let s = self.strength;
        [0, 1, 2].map(|i| (inp[i] + (out[i] - inp[i]) * s).clamp(0.0, 1.0))
    }

    pub fn apply(&self, photo: &Photo) -> Photo {
        // Big images: bake the grade into a 3D table once and look pixels up in it (the same
        // trick a .cube LUT uses) — several times faster than working every pixel out.
        if photo.data.len() > 150_000 {
            let lut = self.bake(LUT_N);
            let data = photo.data.par_iter().map(|p| lut_lookup(&lut, LUT_N, *p)).collect();
            return Photo { width: photo.width, height: photo.height, data, from_raw: photo.from_raw };
        }
        let data = photo.data.par_iter().map(|p| self.apply_px(*p)).collect();
        Photo { width: photo.width, height: photo.height, data, from_raw: photo.from_raw }
    }

    /// The grade sampled on an n×n×n grid (r fastest).
    pub fn bake(&self, n: usize) -> Vec<Rgb> {
        let s = (n - 1) as f32;
        (0..n * n * n)
            .into_par_iter()
            .map(|i| {
                let (r, g, b) = (i % n, (i / n) % n, i / (n * n));
                self.apply_px([r as f32 / s, g as f32 / s, b as f32 / s])
            })
            .collect()
    }
}

const LUT_N: usize = 41;

/// Calibration as a 3×3 matrix in linear light: each primary turned around the grey axis
/// (hue, ±30°) and pushed away from / towards grey (saturation); white stays white.
fn calibration_matrix(c: &crate::adjust::Calibration) -> [[f32; 3]; 3] {
    let prim = [(c.red_hue, c.red_sat), (c.green_hue, c.green_sat), (c.blue_hue, c.blue_sat)];
    let mut cols = [[0f32; 3]; 3];
    let k = 1.0 / 3f32.sqrt();
    for (i, (h, s)) in prim.iter().enumerate() {
        let mut v = [0f32; 3];
        v[i] = 1.0;
        // rotate about (1,1,1)/sqrt3 (Rodrigues)
        let th = (h / 100.0 * 30.0).to_radians();
        let (sn, cs) = th.sin_cos();
        let ax = [k, k, k];
        let dot = v[0] * ax[0] + v[1] * ax[1] + v[2] * ax[2];
        let cross = [ax[1] * v[2] - ax[2] * v[1], ax[2] * v[0] - ax[0] * v[2], ax[0] * v[1] - ax[1] * v[0]];
        let mut r = [0f32; 3];
        for j in 0..3 {
            r[j] = v[j] * cs + cross[j] * sn + ax[j] * dot * (1.0 - cs);
        }
        // saturation: distance from grey
        let g = (r[0] + r[1] + r[2]) / 3.0;
        let f = 1.0 + s / 100.0 * 0.6;
        for j in 0..3 {
            cols[i][j] = g + (r[j] - g) * f;
        }
    }
    // matrix with those columns, rows normalised so white maps to white
    let mut m = [[0f32; 3]; 3];
    for r in 0..3 {
        for col in 0..3 {
            m[r][col] = cols[col][r];
        }
        let sum: f32 = m[r].iter().sum();
        if sum.abs() > 1e-4 {
            for col in 0..3 {
                m[r][col] /= sum;
            }
        }
    }
    m
}

/// Trilinear lookup in a baked grade.
#[inline]
fn lut_lookup(lut: &[Rgb], n: usize, c: Rgb) -> Rgb {
    let s = (n - 1) as f32;
    let f = c.map(|v| v.clamp(0.0, 1.0) * s);
    let i = f.map(|v| (v as usize).min(n - 2));
    let t = [f[0] - i[0] as f32, f[1] - i[1] as f32, f[2] - i[2] as f32];
    let at = |r: usize, g: usize, b: usize| lut[(b * n + g) * n + r];
    let mut out = [0f32; 3];
    for (dr, wr) in [(0, 1.0 - t[0]), (1, t[0])] {
        for (dg, wg) in [(0, 1.0 - t[1]), (1, t[1])] {
            for (db, wb) in [(0, 1.0 - t[2]), (1, t[2])] {
                let w = wr * wg * wb;
                if w > 0.0 {
                    let v = at(i[0] + dr, i[1] + dg, i[2] + db);
                    out[0] += v[0] * w;
                    out[1] += v[1] * w;
                    out[2] += v[2] * w;
                }
            }
        }
    }
    out
}

impl Grade {
    /// Every pixel worked out exactly (used for the LUT test and small images).
    pub fn apply_exact(&self, photo: &Photo) -> Photo {
        let data = photo.data.par_iter().map(|p| self.apply_px(*p)).collect();
        Photo { width: photo.width, height: photo.height, data, from_raw: photo.from_raw }
    }
}
