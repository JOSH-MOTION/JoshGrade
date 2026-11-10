//! Local adjustments (masks), like Lightroom's Masking panel.
//!
//! A mask says *where*: the person (AI), the sky (AI), the background, a painted brush, a
//! linear gradient or a radial gradient. Each mask has its own small set of adjustments
//! (exposure, contrast, highlights, shadows, temperature, tint, saturation, clarity) that
//! apply only there, blended smoothly at the edges.
//!
//! Positions are fractions of the upright photo (x of the width, y of the height), so a
//! mask fits any resolution of the same photo.

use crate::color::{luma, to_display, to_linear, Rgb};
use crate::retouch::{blur, f32s, sample, Plane, Plan};
use crate::Photo;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use tract_onnx::prelude::*;

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct MaskAdjust {
    pub exposure: f32,
    pub contrast: f32,
    pub highlights: f32,
    pub shadows: f32,
    pub temp: f32,
    pub tint: f32,
    pub saturation: f32,
    pub clarity: f32,
}

impl MaskAdjust {
    fn is_zero(&self) -> bool {
        *self == MaskAdjust::default()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Stroke {
    pub pts: Vec<[f32; 2]>,
    /// brush radius, fraction of the photo's long side
    pub size: f32,
    /// 0 (hard) .. 1 (very soft)
    pub feather: f32,
    /// 0..1
    pub flow: f32,
    pub erase: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Mask {
    /// "subject" | "sky" | "background" | "brush" | "linear" | "radial"
    pub kind: String,
    pub name: String,
    pub enabled: bool,
    pub invert: bool,
    /// 0..1 overall strength of this mask's adjustments
    pub amount: f32,
    pub adjust: MaskAdjust,
    pub strokes: Vec<Stroke>,
    /// linear: full effect at `a`, none at `b`. radial: centre `a`, radii `b` (fractions of w, h).
    pub a: [f32; 2],
    pub b: [f32; 2],
    /// radial: rotation (degrees)
    pub angle: f32,
    /// radial: 0..100 softness of the edge
    pub feather: f32,
}

impl Default for Mask {
    fn default() -> Self {
        Mask {
            kind: "brush".into(),
            name: String::new(),
            enabled: true,
            invert: false,
            amount: 1.0,
            adjust: MaskAdjust::default(),
            strokes: vec![],
            a: [0.5, 0.2],
            b: [0.5, 0.6],
            angle: 0.0,
            feather: 50.0,
        }
    }
}

pub fn needs_ai(masks: &[Mask]) -> bool {
    masks.iter().any(|m| m.enabled && matches!(m.kind.as_str(), "subject" | "sky" | "background"))
}

pub fn active(masks: &[Mask]) -> bool {
    masks.iter().any(|m| m.enabled && !m.adjust.is_zero() && m.amount > 0.0)
}

// ------------------------------------------------------------------ AI segmentation

pub const SUBJECT_FILE: &str = "subject_modnet.onnx";
pub const SUBJECT_URL: &str = "https://huggingface.co/Xenova/modnet/resolve/main/onnx/model.onnx";
pub const SKY_FILE: &str = "scene_segformer_b0.onnx";
pub const SKY_URL: &str = "https://huggingface.co/Xenova/segformer-b0-finetuned-ade-512-512/resolve/main/onnx/model.onnx";

/// Where the person and the sky are (probabilities on a small grid).
#[derive(Clone)]
pub struct Segments {
    pub(crate) subject: Plane,
    pub(crate) sky: Plane,
}

static SUBJECT: Mutex<Option<(usize, usize, Plan)>> = Mutex::new(None);
static SKY: Mutex<Option<Plan>> = Mutex::new(None);

fn load(path: &std::path::Path, shape: [usize; 4]) -> Result<Plan, String> {
    if !path.exists() {
        return Err("model-missing".into());
    }
    // tract needs a deep stack while it plans big models
    let p = path.to_path_buf();
    std::thread::Builder::new()
        .stack_size(64 << 20)
        .spawn(move || -> Result<Plan, String> {
            Ok(tract_onnx::onnx()
                .model_for_path(&p)
                .and_then(|m| m.with_input_fact(0, f32::fact(shape).into()))
                .and_then(|m| m.into_optimized())
                .and_then(|m| m.into_runnable())
                .map_err(|e| e.to_string())?
                .into())
        })
        .map_err(|e| e.to_string())?
        .join()
        .map_err(|_| "model failed to load".to_string())?
}

fn resize_rgb(p: &Photo, w: usize, h: usize) -> Vec<[f32; 3]> {
    let (sw, sh) = (p.width as usize, p.height as usize);
    (0..w * h)
        .into_par_iter()
        .map(|i| {
            let (x, y) = (i % w, i / w);
            // box-average the source area
            let (x0, x1) = (x * sw / w, ((x + 1) * sw / w).max(x * sw / w + 1).min(sw));
            let (y0, y1) = (y * sh / h, ((y + 1) * sh / h).max(y * sh / h + 1).min(sh));
            let mut a = [0f32; 3];
            let mut n = 0.0;
            for yy in y0..y1 {
                for xx in x0..x1 {
                    let c = p.data[yy * sw + xx];
                    for k in 0..3 {
                        a[k] += c[k];
                    }
                    n += 1.0;
                }
            }
            a.map(|v| v / n)
        })
        .collect()
}

/// Find the person and the sky. Run once per photo, on a preview-size copy.
pub fn segment(photo: &Photo) -> Result<Segments, String> {
    let dir = crate::retouch::model_dir();
    // --- subject (MODNet portrait matting): short side 512, multiple of 32
    let (w, h) = (photo.width as f32, photo.height as f32);
    let s = 512.0 / w.min(h);
    let sw = (((w * s) / 32.0).round() as usize * 32).clamp(256, 1024);
    let sh = (((h * s) / 32.0).round() as usize * 32).clamp(256, 1024);
    let plan = {
        let mut g = SUBJECT.lock().unwrap();
        match &*g {
            Some((a, b, p)) if *a == sw && *b == sh => p.clone(),
            _ => {
                let p = load(&dir.join(SUBJECT_FILE), [1, 3, sh, sw])?;
                *g = Some((sw, sh, p.clone()));
                p
            }
        }
    };
    let px = resize_rgb(photo, sw, sh);
    let mut input = vec![0f32; 3 * sw * sh];
    for (i, c) in px.iter().enumerate() {
        for k in 0..3 {
            input[k * sw * sh + i] = (c[k] - 0.5) / 0.5;
        }
    }
    let t = Tensor::from_shape(&[1, 3, sh, sw], &input).map_err(|e| e.to_string())?;
    let o = plan.run(tvec!(t.into())).map_err(|e| e.to_string())?;
    let subject = Plane { w: sw, h: sh, d: f32s(&o[0])? };

    // --- sky (SegFormer, ADE20K class 2)
    let plan = {
        let mut g = SKY.lock().unwrap();
        match &*g {
            Some(p) => p.clone(),
            None => {
                let p = load(&dir.join(SKY_FILE), [1, 3, 512, 512])?;
                *g = Some(p.clone());
                p
            }
        }
    };
    const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
    const STD: [f32; 3] = [0.229, 0.224, 0.225];
    let px = resize_rgb(photo, 512, 512);
    let mut input = vec![0f32; 3 * 512 * 512];
    for (i, c) in px.iter().enumerate() {
        for k in 0..3 {
            input[k * 512 * 512 + i] = (c[k] - MEAN[k]) / STD[k];
        }
    }
    let t = Tensor::from_shape(&[1, 3, 512, 512], &input).map_err(|e| e.to_string())?;
    let o = plan.run(tvec!(t.into())).map_err(|e| e.to_string())?;
    let lg = f32s(&o[0])?;
    let n = lg.len() / 150;
    let side = (n as f32).sqrt() as usize;
    let sky: Vec<f32> = (0..n)
        .map(|i| {
            let mut mx = f32::MIN;
            for c in 0..150 {
                mx = mx.max(lg[c * n + i]);
            }
            let mut sum = 0.0;
            for c in 0..150 {
                sum += (lg[c * n + i] - mx).exp();
            }
            (lg[2 * n + i] - mx).exp() / sum
        })
        .collect();
    Ok(Segments { subject, sky: Plane { w: side, h: side, d: sky } })
}

// ------------------------------------------------------------------ rendering masks

/// Which part of the full photo a buffer covers (pixels of the full photo).
#[derive(Debug, Clone, Copy)]
pub struct View {
    pub full_w: f32,
    pub full_h: f32,
    pub x0: f32,
    pub y0: f32,
    /// full-photo pixels per buffer pixel
    pub step: f32,
}

impl View {
    pub fn whole(p: &Photo) -> View {
        View { full_w: p.width as f32, full_h: p.height as f32, x0: 0.0, y0: 0.0, step: 1.0 }
    }
}

fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Edge-aware refinement of a soft mask using the photo's brightness (follows hair, edges).
fn refine(mask: Plane, guide: &Plane, sigma: f32) -> Plane {
    let mi = blur(guide, sigma);
    let mp = blur(&mask, sigma);
    let mii = blur(&guide.zip(guide, |a, b| a * b), sigma);
    let mip = blur(&guide.zip(&mask, |a, b| a * b), sigma);
    let n = mask.d.len();
    let eps = 2e-3;
    let a: Vec<f32> = (0..n).into_par_iter().map(|i| (mip.d[i] - mi.d[i] * mp.d[i]) / (mii.d[i] - mi.d[i] * mi.d[i] + eps)).collect();
    let a = Plane { w: mask.w, h: mask.h, d: a };
    let b = mp.zip(&a.zip(&mi, |a, m| a * m), |m, am| m - am);
    let (ma, mb) = (blur(&a, sigma), blur(&b, sigma));
    Plane { w: mask.w, h: mask.h, d: (0..n).into_par_iter().map(|i| (ma.d[i] * guide.d[i] + mb.d[i]).clamp(0.0, 1.0)).collect() }
}

/// Paint brush strokes onto a small canvas covering the whole photo (they are soft anyway).
fn brush_canvas(m: &Mask, full_w: f32, full_h: f32) -> Plane {
    let k = (900.0 / full_w.max(full_h)).min(1.0);
    let (w, h) = (((full_w * k).round() as usize).max(1), ((full_h * k).round() as usize).max(1));
    let long = w.max(h) as f32;
    let mut c = Plane::new(w, h, 0.0);
    for s in &m.strokes {
        let r = (s.size * long).max(0.5);
        let inner = r * (1.0 - s.feather.clamp(0.0, 1.0));
        let flow = s.flow.clamp(0.01, 1.0);
        // walk the stroke in small steps
        let mut pts: Vec<[f32; 2]> = Vec::new();
        for (i, p) in s.pts.iter().enumerate() {
            let q = [p[0] * w as f32, p[1] * h as f32];
            if i == 0 {
                pts.push(q);
                continue;
            }
            let last = *pts.last().unwrap();
            let d = ((q[0] - last[0]).powi(2) + (q[1] - last[1]).powi(2)).sqrt();
            let n = (d / (r * 0.25).max(0.5)).ceil() as usize;
            for j in 1..=n.max(1) {
                let t = j as f32 / n.max(1) as f32;
                pts.push([last[0] + (q[0] - last[0]) * t, last[1] + (q[1] - last[1]) * t]);
            }
        }
        // stamp: take the max along one stroke so overlapping dabs don't build up
        let mut stroke = Plane::new(w, h, 0.0);
        for p in &pts {
            let (x0, x1) = (((p[0] - r).floor().max(0.0)) as usize, ((p[0] + r + 1.0).ceil() as usize).min(w));
            let (y0, y1) = (((p[1] - r).floor().max(0.0)) as usize, ((p[1] + r + 1.0).ceil() as usize).min(h));
            for y in y0..y1 {
                for x in x0..x1 {
                    let d = ((x as f32 + 0.5 - p[0]).powi(2) + (y as f32 + 0.5 - p[1]).powi(2)).sqrt();
                    let v = if d <= inner { 1.0 } else { 1.0 - smooth((d - inner) / (r - inner).max(0.5)) };
                    let o = &mut stroke.d[y * w + x];
                    *o = o.max(v * flow);
                }
            }
        }
        for i in 0..c.d.len() {
            let v = stroke.d[i];
            c.d[i] = if s.erase { c.d[i] * (1.0 - v) } else { c.d[i] + (1.0 - c.d[i]) * v };
        }
    }
    c
}

/// The mask (0..1) for a buffer of `w×h` pixels at `view`. `guide` = the buffer's brightness.
pub(crate) fn render(m: &Mask, seg: Option<&Segments>, view: &View, w: usize, h: usize, guide: Option<&Plane>) -> Plane {
    let at = |i: usize| -> (f32, f32) { (view.x0 + ((i % w) as f32 + 0.5) * view.step, view.y0 + ((i / w) as f32 + 0.5) * view.step) };
    let (fw, fh) = (view.full_w, view.full_h);
    let mut out = match m.kind.as_str() {
        "subject" | "background" | "sky" => {
            let Some(seg) = seg else { return Plane::new(w, h, 0.0) };
            let src = if m.kind == "sky" { &seg.sky } else { &seg.subject };
            let p = Plane {
                w,
                h,
                d: (0..w * h)
                    .into_par_iter()
                    .map(|i| {
                        let (x, y) = at(i);
                        sample(src, x / fw * src.w as f32 - 0.5, y / fh * src.h as f32 - 0.5)
                    })
                    .collect(),
            };
            // snap the soft AI edge to the real edges in the photo
            let p = match guide {
                Some(g) => refine(p, g, (fw.max(fh) / view.step * 0.004).max(1.0)),
                None => p,
            };
            if m.kind == "background" { p.map(|v| 1.0 - v) } else { p }
        }
        "brush" => {
            let c = brush_canvas(m, fw, fh);
            Plane {
                w,
                h,
                d: (0..w * h)
                    .into_par_iter()
                    .map(|i| {
                        let (x, y) = at(i);
                        sample(&c, x / fw * c.w as f32 - 0.5, y / fh * c.h as f32 - 0.5)
                    })
                    .collect(),
            }
        }
        "linear" => {
            let (ax, ay, bx, by) = (m.a[0] * fw, m.a[1] * fh, m.b[0] * fw, m.b[1] * fh);
            let (dx, dy) = (bx - ax, by - ay);
            let l2 = (dx * dx + dy * dy).max(1.0);
            Plane {
                w,
                h,
                d: (0..w * h)
                    .into_par_iter()
                    .map(|i| {
                        let (x, y) = at(i);
                        1.0 - smooth(((x - ax) * dx + (y - ay) * dy) / l2)
                    })
                    .collect(),
            }
        }
        "radial" => {
            let (cx, cy) = (m.a[0] * fw, m.a[1] * fh);
            let (rx, ry) = ((m.b[0] * fw).max(1.0), (m.b[1] * fh).max(1.0));
            let (sn, cs) = (-m.angle.to_radians()).sin_cos();
            let f = (m.feather / 100.0).clamp(0.01, 1.0);
            Plane {
                w,
                h,
                d: (0..w * h)
                    .into_par_iter()
                    .map(|i| {
                        let (x, y) = at(i);
                        let (px, py) = (x - cx, y - cy);
                        let (u, v) = ((px * cs - py * sn) / rx, (px * sn + py * cs) / ry);
                        let d = (u * u + v * v).sqrt();
                        1.0 - smooth((d - (1.0 - f)) / f)
                    })
                    .collect(),
            }
        }
        _ => Plane::new(w, h, 0.0),
    };
    if m.invert {
        out = out.map(|v| 1.0 - v);
    }
    let a = m.amount.clamp(0.0, 1.0);
    if a < 1.0 {
        out = out.map(|v| v * a);
    }
    out
}

fn brightness(p: &Photo) -> Plane {
    Plane { w: p.width as usize, h: p.height as usize, d: p.data.par_iter().map(|c| luma(*c)).collect() }
}

/// Apply every mask's adjustments. `view` says where this buffer sits in the full photo.
pub fn apply(p: &Photo, masks: &[Mask], seg: Option<&Segments>, view: &View) -> Photo {
    if !active(masks) {
        return p.clone();
    }
    let (w, h) = (p.width as usize, p.height as usize);
    let mut out = p.clone();
    for m in masks.iter().filter(|m| m.enabled && !m.adjust.is_zero() && m.amount > 0.0) {
        let guide = brightness(&out);
        let mask = render(m, seg, view, w, h, Some(&guide));
        let a = &m.adjust;
        let ev = 2f32.powf(a.exposure / 50.0);
        let (temp, tint) = (a.temp / 330.0, a.tint / 250.0);
        let gain = [(1.0 + temp) * ev, (1.0 - tint) * ev, (1.0 - temp) * ev];
        let (con, hi, sh) = (a.contrast / 250.0, a.highlights / 200.0, a.shadows / 250.0);
        let sat = a.saturation / 100.0;
        let clarity = if a.clarity != 0.0 {
            let long = view.full_w.max(view.full_h) / view.step;
            Some(blur(&guide, long * 0.012))
        } else {
            None
        };
        let cl = a.clarity / 100.0 * 0.9;
        out.data.par_iter_mut().enumerate().for_each(|(i, c)| {
            let mv = mask.d[i];
            if mv <= 0.001 {
                return;
            }
            let lin = [to_linear(c[0]) * gain[0], to_linear(c[1]) * gain[1], to_linear(c[2]) * gain[2]];
            let mut d: Rgb = lin.map(|v| to_display(v).min(1.5));
            let y = luma(d).max(1e-5);
            let mut x = y.min(1.0);
            x += sh * 2.5 * x * (1.0 - x).powi(3);
            x += hi * 2.5 * x.powi(3) * (1.0 - x);
            x += con * 4.0 * (x - x * x) * (x - 0.5);
            if let Some(b) = &clarity {
                let mid = (4.0 * y * (1.0 - y)).clamp(0.0, 1.0);
                x += cl * (y - b.d[i]) * mid;
            }
            let k = x.max(0.0) / y;
            d = d.map(|v| v * k);
            if sat != 0.0 {
                let yy = luma(d);
                d = d.map(|v| yy + (v - yy) * (1.0 + sat).max(0.0));
            }
            for k in 0..3 {
                c[k] = (c[k] + (d[k].clamp(0.0, 1.0) - c[k]) * mv).clamp(0.0, 1.0);
            }
        });
    }
    let _ = (w, h);
    out
}

/// Where one mask is (0..1 per pixel of `p`), for drawing an overlay.
pub fn values(p: &Photo, m: &Mask, seg: Option<&Segments>, view: &View) -> Vec<f32> {
    let (w, h) = (p.width as usize, p.height as usize);
    let mut mm = m.clone();
    mm.amount = 1.0;
    render(&mm, seg, view, w, h, Some(&brightness(p))).d
}

/// The two mask models (person + scene) are on this computer.
pub fn models_ready() -> bool {
    let d = crate::retouch::model_dir();
    d.join(SUBJECT_FILE).exists() && d.join(SKY_FILE).exists()
}

pub fn shared(seg: Segments) -> Arc<Segments> {
    Arc::new(seg)
}
