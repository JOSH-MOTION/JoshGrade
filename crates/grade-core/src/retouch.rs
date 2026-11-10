//! Automatic portrait retouch: blemish removal, texture-preserving skin smoothing, evening
//! out redness, and taming shine — the way a careful retoucher works, not a beauty filter.
//!
//! ```text
//! find faces (YuNet) -> map skin / eyes / lips / hair per face (BiSeNet face parsing)
//!   -> protect beard & stubble, nostrils, everything near eyes / brows / lips / hair
//!   -> find spots (scale-space blobs that are darker or redder than the skin around them)
//!        acne & marks are healed; moles, beauty marks and freckles are kept by default
//!   -> heal: colour of the surrounding skin + real pore texture from nearby skin
//!   -> smooth: frequency separation in log space — pores are never touched, only the
//!      blotchy middle band is reduced, and strong features (folds, real shape) pass through
//! ```
//!
//! Every distance is a fraction of the face width, and the maths is done on log values
//! (ratios), so it behaves the same on a headshot or a full-length shot, and on light or
//! dark skin. Nothing changes outside the skin, and the skin's own colour is kept.

use crate::color::Rgb;
use crate::Photo;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use tract_onnx::prelude::*;

// ------------------------------------------------------------------ settings

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct RetouchSettings {
    pub enabled: bool,
    /// 0..1: how much of each found blemish is healed.
    pub blemishes: f32,
    /// 0..1: how much of the blotchy middle band is reduced (pores always stay).
    pub smooth: f32,
    /// 0..1: how much red patches are brought back to the face's own skin colour.
    pub even: f32,
    /// 0..1: how much shiny highlights are calmed.
    pub shine: f32,
    pub keep_moles: bool,
    pub keep_freckles: bool,
}

impl Default for RetouchSettings {
    fn default() -> Self {
        RetouchSettings { enabled: false, blemishes: 1.0, smooth: 0.45, even: 0.5, shine: 0.35, keep_moles: true, keep_freckles: true }
    }
}

impl RetouchSettings {
    pub fn active(&self) -> bool {
        self.enabled && (self.blemishes > 0.0 || self.smooth > 0.0 || self.even > 0.0 || self.shine > 0.0)
    }
}

// ------------------------------------------------------------------ models

/// The face map model (53 MB) is downloaded once; the small face finder is built in.
pub const PARSER_FILE: &str = "face_parsing_resnet18.onnx";
pub const PARSER_URL: &str = "https://huggingface.co/yakhyo/uniface-weights/resolve/main/parsing_resnet18.onnx";
pub const PARSER_MB: u32 = 53;
const YUNET: &[u8] = include_bytes!("../models/face_detection_yunet_2023mar.onnx");

#[derive(Debug, thiserror::Error)]
pub enum RetouchError {
    #[error("The retouch engine isn't downloaded yet.")]
    ModelMissing,
    #[error("retouch engine: {0}")]
    Model(String),
}

type Plan = Arc<SimplePlan<TypedFact, Box<dyn TypedOp>, Graph<TypedFact, Box<dyn TypedOp>>>>;

static MODEL_DIR: OnceLock<PathBuf> = OnceLock::new();
static DETECTOR: OnceLock<Result<Plan, String>> = OnceLock::new();
static PARSER: Mutex<Option<Plan>> = Mutex::new(None);

/// Where the downloaded face map model lives (set once at startup).
pub fn set_model_dir(dir: PathBuf) {
    let _ = MODEL_DIR.set(dir);
}

pub fn model_dir() -> PathBuf {
    MODEL_DIR.get().cloned().unwrap_or_else(|| {
        std::env::var_os("JOSHGRADE_MODELS")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".joshgrade").join("models")))
            .unwrap_or_else(|| PathBuf::from("models"))
    })
}

pub fn parser_path() -> PathBuf {
    model_dir().join(PARSER_FILE)
}

pub fn model_ready() -> bool {
    parser_path().exists()
}

fn detector() -> Result<Plan, RetouchError> {
    DETECTOR
        .get_or_init(|| {
            (|| -> TractResult<Plan> {
                Ok(tract_onnx::onnx()
                    .model_for_read(&mut std::io::Cursor::new(YUNET))?
                    .with_input_fact(0, f32::fact([1, 3, 640, 640]).into())?
                    .into_optimized()?
                    .into_runnable()?
                    .into())
            })()
            .map_err(|e| e.to_string())
        })
        .clone()
        .map_err(RetouchError::Model)
}

fn parser() -> Result<Plan, RetouchError> {
    let mut g = PARSER.lock().unwrap();
    if let Some(p) = &*g {
        return Ok(p.clone());
    }
    let path = parser_path();
    if !path.exists() {
        return Err(RetouchError::ModelMissing);
    }
    let p: Plan = (|| -> TractResult<Plan> {
        Ok(tract_onnx::onnx()
            .model_for_path(&path)?
            .with_input_fact(0, f32::fact([1, 3, 512, 512]).into())?
            .into_optimized()?
            .into_runnable()?
            .into())
    })()
    .map_err(|e| RetouchError::Model(e.to_string()))?;
    *g = Some(p.clone());
    Ok(p)
}

// ------------------------------------------------------------------ planes & filters

/// One channel of floats.
#[derive(Clone)]
struct Plane {
    w: usize,
    h: usize,
    d: Vec<f32>,
}

impl Plane {
    fn new(w: usize, h: usize, v: f32) -> Plane {
        Plane { w, h, d: vec![v; w * h] }
    }
    fn map(&self, f: impl Fn(f32) -> f32 + Sync) -> Plane {
        Plane { w: self.w, h: self.h, d: self.d.par_iter().map(|&v| f(v)).collect() }
    }
    fn zip(&self, o: &Plane, f: impl Fn(f32, f32) -> f32 + Sync) -> Plane {
        Plane { w: self.w, h: self.h, d: self.d.par_iter().zip(&o.d).map(|(&a, &b)| f(a, b)).collect() }
    }
    fn at(&self, x: usize, y: usize) -> f32 {
        self.d[y * self.w + x]
    }
}

/// Box sizes whose three passes approximate a Gaussian of `sigma` (Kovesi).
fn boxes(sigma: f32) -> [usize; 3] {
    let n = 3.0;
    let wi = ((12.0 * sigma * sigma / n) + 1.0).sqrt().floor();
    let wl = if wi as i64 % 2 == 0 { wi - 1.0 } else { wi };
    let wu = wl + 2.0;
    let m = ((12.0 * sigma * sigma - n * wl * wl - 4.0 * n * wl - 3.0 * n) / (-4.0 * wl - 4.0)).round();
    let r = |i: f32| (((if i < m { wl } else { wu }) - 1.0) / 2.0).max(0.0) as usize;
    [r(0.0), r(1.0), r(2.0)]
}

fn box_rows(src: &[f32], dst: &mut [f32], w: usize, r: usize) {
    if r == 0 {
        dst.copy_from_slice(src);
        return;
    }
    dst.par_chunks_mut(w).zip(src.par_chunks(w)).for_each(|(o, row)| {
        let n = (2 * r + 1) as f32;
        let get = |i: isize| row[i.clamp(0, w as isize - 1) as usize];
        let mut acc: f32 = (-(r as isize)..=r as isize).map(get).sum();
        for x in 0..w {
            o[x] = acc / n;
            acc += get(x as isize + r as isize + 1) - get(x as isize - r as isize);
        }
    });
}

fn transpose(src: &[f32], w: usize, h: usize) -> Vec<f32> {
    let mut out = vec![0.0; w * h];
    const B: usize = 32;
    out.par_chunks_mut(h * B).enumerate().for_each(|(bx, chunk)| {
        // chunk holds output rows x in [bx*B, bx*B+B)
        let x0 = bx * B;
        let xs = chunk.len() / h;
        for y0 in (0..h).step_by(B) {
            for y in y0..(y0 + B).min(h) {
                let row = &src[y * w..];
                for dx in 0..xs {
                    chunk[dx * h + y] = row[x0 + dx];
                }
            }
        }
    });
    out
}

fn blur(p: &Plane, sigma: f32) -> Plane {
    if sigma < 0.4 {
        return p.clone();
    }
    let bs = boxes(sigma);
    let (w, h) = (p.w, p.h);
    let mut a = p.d.clone();
    let mut b = vec![0.0; w * h];
    for &r in &bs {
        box_rows(&a, &mut b, w, r);
        std::mem::swap(&mut a, &mut b);
    }
    let mut t = transpose(&a, w, h);
    let mut t2 = vec![0.0; w * h];
    for &r in &bs {
        box_rows(&t, &mut t2, h, r);
        std::mem::swap(&mut t, &mut t2);
    }
    Plane { w, h, d: transpose(&t, h, w) }
}

/// Blur that only listens to pixels with weight `wt` (normalised convolution).
fn norm_blur(p: &Plane, wt: &Plane, sigma: f32) -> Plane {
    let num = blur(&p.zip(wt, |a, b| a * b), sigma);
    let den = blur(wt, sigma);
    num.zip(&den, |n, d| n / d.max(1e-4))
}

/// Median of the values where `mask` > t (on a sample, which is plenty).
fn masked_median(p: &Plane, mask: &Plane, t: f32) -> f32 {
    let step = (p.d.len() / 200_000).max(1);
    let mut v: Vec<f32> = p.d.iter().zip(&mask.d).step_by(step).filter(|(_, &m)| m > t).map(|(&x, _)| x).collect();
    if v.is_empty() {
        return 0.0;
    }
    let k = v.len() / 2;
    *v.select_nth_unstable_by(k, |a, b| a.partial_cmp(b).unwrap()).1
}

/// Area-average downscale by an integer-ish factor (for fast analysis).
fn shrink(p: &Plane, k: f32) -> Plane {
    if k >= 0.999 {
        return p.clone();
    }
    let w = ((p.w as f32 * k).round() as usize).max(1);
    let h = ((p.h as f32 * k).round() as usize).max(1);
    let sx = p.w as f32 / w as f32;
    let sy = p.h as f32 / h as f32;
    let d = (0..w * h)
        .into_par_iter()
        .map(|i| {
            let (x, y) = (i % w, i / w);
            let (x0, x1) = ((x as f32 * sx) as usize, (((x + 1) as f32 * sx) as usize).min(p.w).max((x as f32 * sx) as usize + 1));
            let (y0, y1) = ((y as f32 * sy) as usize, (((y + 1) as f32 * sy) as usize).min(p.h).max((y as f32 * sy) as usize + 1));
            let mut s = 0.0;
            for yy in y0..y1 {
                for xx in x0..x1 {
                    s += p.d[yy * p.w + xx];
                }
            }
            s / ((x1 - x0) * (y1 - y0)) as f32
        })
        .collect();
    Plane { w, h, d }
}

fn sample(p: &Plane, fx: f32, fy: f32) -> f32 {
    let fx = fx.clamp(0.0, (p.w - 1) as f32);
    let fy = fy.clamp(0.0, (p.h - 1) as f32);
    let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(p.w - 1), (y0 + 1).min(p.h - 1));
    let (ax, ay) = (fx - x0 as f32, fy - y0 as f32);
    let top = p.at(x0, y0) * (1.0 - ax) + p.at(x1, y0) * ax;
    let bot = p.at(x0, y1) * (1.0 - ax) + p.at(x1, y1) * ax;
    top * (1.0 - ay) + bot * ay
}

/// Fill a disc / ellipse with value 1 (anti-aliased edge).
fn fill_ellipse(p: &mut Plane, cx: f32, cy: f32, rx: f32, ry: f32) {
    let (x0, x1) = (((cx - rx - 1.0).floor().max(0.0)) as usize, ((cx + rx + 2.0).ceil() as usize).min(p.w));
    let (y0, y1) = (((cy - ry - 1.0).floor().max(0.0)) as usize, ((cy + ry + 2.0).ceil() as usize).min(p.h));
    for y in y0..y1 {
        for x in x0..x1 {
            let dx = (x as f32 - cx) / rx.max(0.5);
            let dy = (y as f32 - cy) / ry.max(0.5);
            let d = (dx * dx + dy * dy).sqrt();
            let v = ((1.0 - d) * rx.min(ry).max(0.5) + 0.5).clamp(0.0, 1.0);
            let o = &mut p.d[y * p.w + x];
            *o = o.max(v);
        }
    }
}

// ------------------------------------------------------------------ faces

/// One face: where it is (as fractions of the image size, so it fits any resolution of the
/// same photo) and its skin / protected-feature maps over the crop.
#[derive(Clone)]
pub struct Face {
    pub score: f32,
    /// Face box: x, y, w, h (fractions of image width/height).
    pub bbox: [f32; 4],
    /// Eyes, nose tip, mouth corners (fractions).
    pub kps: [[f32; 2]; 5],
    /// Square crop the maps cover: x0, y0 (fractions), side (fraction of image width).
    crop: [f32; 3],
    skin: Arc<Plane>,
    protect: Arc<Plane>,
}

impl std::fmt::Debug for Face {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Face").field("score", &self.score).field("bbox", &self.bbox).finish()
    }
}

struct Det {
    score: f32,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    kps: [[f32; 2]; 5],
}

fn detect(photo: &Photo) -> Result<Vec<Det>, RetouchError> {
    let (w, h) = (photo.width as usize, photo.height as usize);
    let s = 640.0 / w.max(h) as f32;
    // letterboxed BGR 0..255 input
    let mut chans = [Plane::new(w, h, 0.0), Plane::new(w, h, 0.0), Plane::new(w, h, 0.0)];
    for (i, px) in photo.data.iter().enumerate() {
        for c in 0..3 {
            chans[c].d[i] = px[2 - c] * 255.0;
        }
    }
    let mut input = vec![0f32; 3 * 640 * 640];
    for (c, ch) in chans.iter().enumerate() {
        let sm = shrink(ch, s);
        for y in 0..sm.h.min(640) {
            for x in 0..sm.w.min(640) {
                input[c * 640 * 640 + y * 640 + x] = sm.at(x, y);
            }
        }
    }
    let t = Tensor::from_shape(&[1, 3, 640, 640], &input).map_err(|e| RetouchError::Model(e.to_string()))?;
    let out = detector()?.run(tvec!(t.into())).map_err(|e| RetouchError::Model(e.to_string()))?;
    let view = |i: usize| out[i].as_slice::<f32>().map(|s| s.to_vec()).unwrap_or_default();
    let mut faces = Vec::new();
    for (k, st) in [8usize, 16, 32].iter().enumerate() {
        let cols = 640 / st;
        let (cls, obj, bbox, kps) = (view(k), view(3 + k), view(6 + k), view(9 + k));
        for i in 0..cls.len() {
            let sc = (cls[i].clamp(0.0, 1.0) * obj[i].clamp(0.0, 1.0)).sqrt();
            if sc < 0.6 {
                continue;
            }
            let (r, c) = ((i / cols) as f32, (i % cols) as f32);
            let st = *st as f32;
            let b = &bbox[i * 4..i * 4 + 4];
            let cx = (c + b[0]) * st;
            let cy = (r + b[1]) * st;
            let bw = b[2].exp() * st;
            let bh = b[3].exp() * st;
            let kp = &kps[i * 10..i * 10 + 10];
            let mut k5 = [[0f32; 2]; 5];
            for j in 0..5 {
                k5[j] = [(c + kp[2 * j]) * st / s, (r + kp[2 * j + 1]) * st / s];
            }
            faces.push(Det { score: sc, x: (cx - bw / 2.0) / s, y: (cy - bh / 2.0) / s, w: bw / s, h: bh / s, kps: k5 });
        }
    }
    faces.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
    let mut keep: Vec<Det> = Vec::new();
    for f in faces {
        if keep.iter().all(|g| iou(&f, g) < 0.3) {
            keep.push(f);
        }
    }
    // Background faces too small to retouch are left alone.
    let big = keep.iter().map(|f| f.w).fold(0.0, f32::max);
    keep.retain(|f| f.w > 0.2 * big && f.w > 48.0);
    Ok(keep)
}

fn iou(a: &Det, b: &Det) -> f32 {
    let iw = ((a.x + a.w).min(b.x + b.w) - a.x.max(b.x)).max(0.0);
    let ih = ((a.y + a.h).min(b.y + b.h) - a.y.max(b.y)).max(0.0);
    let inter = iw * ih;
    inter / (a.w * a.h + b.w * b.h - inter + 1e-9)
}

/// Square crop around a face (pixels): x0, y0, side.
fn crop_box(d: &Det) -> (f32, f32, f32) {
    let side = d.w.max(d.h) * 2.2;
    let cx = d.x + d.w / 2.0;
    let cy = d.y + d.h / 2.0;
    (cx - side / 2.0, cy - side / 2.0 - 0.05 * side, side)
}

/// Find the faces in a photo and map their skin. Do this once per photo (on the preview-size
/// copy); the result applies to any resolution of the same photo.
pub fn find_faces(photo: &Photo) -> Result<Vec<Face>, RetouchError> {
    let dets = detect(photo)?;
    if dets.is_empty() {
        return Ok(vec![]);
    }
    let parse = parser()?;
    let (w, h) = (photo.width as f32, photo.height as f32);
    const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
    const STD: [f32; 3] = [0.229, 0.224, 0.225];
    let mut out = Vec::new();
    for d in dets {
        let (x0, y0, side) = crop_box(&d);
        // sample the crop at 512×512 (box-average when the crop is bigger)
        let step = side / 512.0;
        let mut input = vec![0f32; 3 * 512 * 512];
        for yy in 0..512 {
            for xx in 0..512 {
                let mut acc = [0f32; 3];
                let mut n = 0.0;
                let sub = step.ceil().clamp(1.0, 4.0) as usize;
                for sy in 0..sub {
                    for sx in 0..sub {
                        let px = x0 + (xx as f32 + (sx as f32 + 0.5) / sub as f32) * step;
                        let py = y0 + (yy as f32 + (sy as f32 + 0.5) / sub as f32) * step;
                        if px >= 0.0 && py >= 0.0 && px < w && py < h {
                            let p = photo.data[py as usize * photo.width as usize + px as usize];
                            for c in 0..3 {
                                acc[c] += p[c];
                            }
                        }
                        n += 1.0;
                    }
                }
                for c in 0..3 {
                    input[c * 512 * 512 + yy * 512 + xx] = (acc[c] / n - MEAN[c]) / STD[c];
                }
            }
        }
        let t = Tensor::from_shape(&[1, 3, 512, 512], &input).map_err(|e| RetouchError::Model(e.to_string()))?;
        let o = parse.run(tvec!(t.into())).map_err(|e| RetouchError::Model(e.to_string()))?;
        let lg = o[0].as_slice::<f32>().map_err(|e| RetouchError::Model(e.to_string()))?;
        let n = 512 * 512;
        let mut skin = Plane::new(512, 512, 0.0);
        let mut prot = Plane::new(512, 512, 0.0);
        for i in 0..n {
            let mut mx = f32::MIN;
            for c in 0..19 {
                mx = mx.max(lg[c * n + i]);
            }
            let mut e = [0f32; 19];
            let mut sum = 0.0;
            for c in 0..19 {
                e[c] = (lg[c * n + i] - mx).exp();
                sum += e[c];
            }
            // 1 skin, 10 nose, 14 neck | 2,3 brows, 4,5 eyes, 6 glasses, 11 mouth, 12,13 lips, 17 hair
            skin.d[i] = (e[1] + e[10] + e[14]) / sum;
            prot.d[i] = (e[2] + e[3] + e[4] + e[5] + e[6] + e[11] + e[12] + e[13] + e[17]) / sum;
        }
        out.push(Face {
            score: d.score,
            bbox: [d.x / w, d.y / h, d.w / w, d.h / h],
            kps: d.kps.map(|k| [k[0] / w, k[1] / h]),
            crop: [x0 / w, y0 / h, side / w],
            skin: Arc::new(skin),
            protect: Arc::new(prot),
        });
    }
    Ok(out)
}

// ------------------------------------------------------------------ retouch

const EPS: f32 = 0.01;
const LUM: [f32; 3] = [0.2126, 0.7152, 0.0722];

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SpotKind {
    Blemish,
    Mole,
    Freckle,
    Highlight,
}

#[derive(Debug, Clone, Copy)]
pub struct Spot {
    pub x: f32,
    pub y: f32,
    pub r: f32,
    pub kind: SpotKind,
    dark: f32,
}

/// Retouch every face in the photo. `faces` come from [`find_faces`] (any resolution).
pub fn apply(photo: &Photo, faces: &[Face], s: &RetouchSettings) -> Photo {
    let mut out = photo.clone();
    if !s.active() {
        return out;
    }
    let (w, h) = (photo.width as usize, photo.height as usize);
    for f in faces {
        let side = (f.crop[2] * w as f32).round() as isize;
        let x0 = (f.crop[0] * w as f32).round() as isize;
        let y0 = (f.crop[1] * h as f32).round() as isize;
        let (sx0, sy0) = (x0.max(0) as usize, y0.max(0) as usize);
        let (sx1, sy1) = (((x0 + side).max(0) as usize).min(w), ((y0 + side).max(0) as usize).min(h));
        if sx1 <= sx0 + 8 || sy1 <= sy0 + 8 {
            continue;
        }
        let (cw, ch) = (sx1 - sx0, sy1 - sy0);
        // maps: 512² over the whole (possibly off-image) crop -> this part, at this resolution
        let k = 512.0 / side as f32;
        let take = |p: &Plane| -> Plane {
            let d = (0..cw * ch)
                .into_par_iter()
                .map(|i| {
                    let gx = (sx0 + i % cw) as f32 - x0 as f32;
                    let gy = (sy0 + i / cw) as f32 - y0 as f32;
                    sample(p, (gx + 0.5) * k - 0.5, (gy + 0.5) * k - 0.5)
                })
                .collect();
            Plane { w: cw, h: ch, d }
        };
        let skin = take(&f.skin);
        let prot = take(&f.protect);
        let face_w = f.bbox[2] * w as f32;
        let nose = [f.kps[2][0] * w as f32 - sx0 as f32, f.kps[2][1] * h as f32 - sy0 as f32];
        let crop: Vec<Rgb> = (0..ch).flat_map(|y| out.data[(sy0 + y) * w + sx0..(sy0 + y) * w + sx1].to_vec()).collect();
        let (new, _) = retouch_face(&crop, cw, ch, &skin, &prot, face_w, nose, s);
        for y in 0..ch {
            out.data[(sy0 + y) * w + sx0..(sy0 + y) * w + sx1].copy_from_slice(&new[y * cw..(y + 1) * cw]);
        }
    }
    out
}

fn retouch_face(crop: &[Rgb], w: usize, h: usize, skin: &Plane, prot: &Plane, fw: f32, nose: [f32; 2], s: &RetouchSettings) -> (Vec<Rgb>, Vec<Spot>) {
    let f = fw.max(16.0);
    let ln: [Plane; 3] = std::array::from_fn(|c| Plane { w, h, d: crop.par_iter().map(|p| (p[c].max(0.0) + EPS).ln()).collect() });
    let yl = Plane { w, h, d: (0..w * h).into_par_iter().map(|i| LUM[0] * ln[0].d[i] + LUM[1] * ln[1].d[i] + LUM[2] * ln[2].d[i]).collect() };

    // ---- where to work: confident skin, away from eyes / brows / lips / hair
    let mut m = skin.map(|v| ((v - 0.5) * 4.0).clamp(0.0, 1.0));
    let away = blur(&prot.map(|v| if v > 0.4 { 1.0 } else { 0.0 }), f / 60.0);
    m = m.zip(&away, |a, b| a * (1.0 - b * 3.0).clamp(0.0, 1.0));
    // Beard / stubble the parser called skin: lots of fine texture but not red (acne is red).
    let fine = yl.zip(&blur(&yl, f / 300.0), |a, b| (a - b).abs());
    let fe = blur(&fine, f / 60.0);
    let ref_e = masked_median(&fe, &m, 0.6) + 1e-4;
    let redl = ln[0].zip(&ln[1], |r, g| r - g);
    let red_ref = masked_median(&redl, &m, 0.6);
    let red_ex = blur(&redl, f / 60.0).map(|v| (v - red_ref).max(0.0));
    let hairy = fe.zip(&red_ex, |e, r| ((e / ref_e - 1.8) / 1.2).clamp(0.0, 1.0) * (1.0 - (r / 0.05).clamp(0.0, 1.0)));
    m = m.zip(&hairy, |a, b| a * (1.0 - b));
    // Nostrils are dark by nature, never blemishes.
    let mut nz = Plane::new(w, h, 0.0);
    fill_ellipse(&mut nz, nose[0], nose[1] + f * 0.06, f * 0.2, f * 0.09);
    m = m.zip(&blur(&nz, f / 60.0), |a, b| a * (1.0 - b));
    let m = blur(&m, f / 200.0);
    if masked_median(&m, &m, 0.5) == 0.0 {
        return (crop.to_vec(), vec![]);
    }

    // ---- spots
    let spots = find_spots(&ln, &m, f);
    let mut heal = Plane::new(w, h, 0.0);
    if s.blemishes > 0.0 {
        for sp in &spots {
            let keep = match sp.kind {
                SpotKind::Highlight => true,
                SpotKind::Mole => s.keep_moles,
                SpotKind::Freckle => s.keep_freckles,
                SpotKind::Blemish => false,
            };
            if keep || sp.dark > 0.6 {
                continue;
            }
            let rr = sp.r * 2.2;
            // only spots fully inside clean skin
            let win = rr * 1.3;
            let (x0, x1) = ((sp.x - win).max(0.0) as usize, ((sp.x + win + 1.0) as usize).min(w));
            let (y0, y1) = ((sp.y - win).max(0.0) as usize, ((sp.y + win + 1.0) as usize).min(h));
            let mut ok = x1 > x0 && y1 > y0;
            'o: for y in y0..y1 {
                for x in x0..x1 {
                    if m.at(x, y) < 0.55 {
                        ok = false;
                        break 'o;
                    }
                }
            }
            if ok {
                fill_ellipse(&mut heal, sp.x, sp.y, rr, rr);
            }
        }
    }
    let heal = blur(&heal, f / 300.0).map(|v| (v * 1.3).clamp(0.0, 1.0) * s.blemishes);

    // ---- heal: surrounding skin colour (spots excluded) + pore texture from nearby skin
    let mut lg2: [Plane; 3] = ln.clone();
    if heal.d.iter().any(|&v| v > 0.0) {
        let wt = heal.zip(&m, |hv, mv| if hv > 0.02 { 0.0 } else { mv + 1e-3 });
        let ok = heal.zip(&m, |hv, mv| if hv > 0.02 { 0.0 } else { mv });
        // pick the direction with the most clean skin to borrow texture from
        let dist = (f / 22.0).round() as isize;
        let mut best = (f32::MIN, (0isize, 0isize));
        for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (-1, -1), (1, -1), (-1, 1)] {
            let (ox, oy) = (dx * dist, dy * dist);
            let mut sc = 0.0;
            for y in (0..h).step_by(2) {
                for x in (0..w).step_by(2) {
                    let hv = heal.at(x, y);
                    if hv > 0.0 {
                        let (sx, sy) = (x as isize + ox, y as isize + oy);
                        if sx >= 0 && sy >= 0 && (sx as usize) < w && (sy as usize) < h {
                            sc += hv * ok.at(sx as usize, sy as usize);
                        }
                    }
                }
            }
            if sc > best.0 {
                best = (sc, (ox, oy));
            }
        }
        let (ox, oy) = best.1;
        for c in 0..3 {
            let base = norm_blur(&ln[c], &wt, f / 70.0);
            let tex = ln[c].zip(&blur(&ln[c], f / 300.0), |a, b| a - b);
            let d: Vec<f32> = (0..w * h)
                .into_par_iter()
                .map(|i| {
                    let hv = heal.d[i];
                    if hv <= 0.0 {
                        return ln[c].d[i];
                    }
                    let (x, y) = ((i % w) as isize, (i / w) as isize);
                    let (sx, sy) = ((x + ox).clamp(0, w as isize - 1) as usize, (y + oy).clamp(0, h as isize - 1) as usize);
                    let filled = base.d[i] + tex.d[sy * w + sx];
                    ln[c].d[i] * (1.0 - hv) + filled * hv
                })
                .collect();
            lg2[c] = Plane { w, h, d };
        }
    }

    // ---- smoothing: frequency separation in log space; pores (fine band) always kept
    let y2 = Plane { w, h, d: (0..w * h).into_par_iter().map(|i| LUM[0] * lg2[0].d[i] + LUM[1] * lg2[1].d[i] + LUM[2] * lg2[2].d[i]).collect() };
    let c2: [Plane; 3] = std::array::from_fn(|c| lg2[c].zip(&y2, |a, b| a - b));
    let fine_y = y2.zip(&blur(&y2, f / 350.0), |a, b| a - b);
    let wm = m.map(|v| v + 1e-3);
    let low_y = norm_blur(&y2, &wm, f / 45.0);
    let mid_y: Plane = Plane { w, h, d: (0..w * h).into_par_iter().map(|i| y2.d[i] - fine_y.d[i] - low_y.d[i]).collect() };
    let tau = 2.5 * masked_median(&mid_y.map(f32::abs), &m, 0.5) + 1e-4;
    // only red excess is evened, towards the face's own skin colour (never greys the skin)
    let red = c2[0].zip(&c2[1], |a, b| a - b);
    let rref = masked_median(&red, &m, 0.6);
    let excess = norm_blur(&red, &wm, f / 120.0).map(|v| (v - rref).max(0.0));
    let cref: [f32; 3] = std::array::from_fn(|c| masked_median(&c2[c], &m, 0.6));

    let out: Vec<Rgb> = (0..w * h)
        .into_par_iter()
        .map(|i| {
            let mv = m.d[i];
            if mv <= 0.0 && heal.d[i] <= 0.0 {
                return crop[i];
            }
            let mid = mid_y.d[i];
            let att = s.smooth * (-(mid / tau).powi(2)).exp();
            let mid2 = mid * (1.0 - att * mv);
            let mut yn = fine_y.d[i] + mid2 + low_y.d[i];
            let hi = (mid2 + fine_y.d[i] - 0.08).max(0.0);
            yn -= hi * s.shine * mv;
            let wv = (excess.d[i] / 0.12).clamp(0.0, 1.0) * s.even * mv;
            let mut o = [0f32; 3];
            for c in 0..3 {
                let cc = c2[c].d[i] + (cref[c] - c2[c].d[i]) * wv;
                o[c] = ((yn + cc).exp() - EPS).clamp(0.0, 1.0);
            }
            o
        })
        .collect();
    (out, spots)
}

/// Scale-space blob detection on a "spottiness" signal (darker and/or redder than the skin
/// around). Runs on a copy where the face is ~420 px wide, so it costs the same at any size.
fn find_spots(ln: &[Plane; 3], m: &Plane, f: f32) -> Vec<Spot> {
    let k = (420.0 / f).min(1.0);
    let small: [Plane; 3] = std::array::from_fn(|c| shrink(&ln[c], k));
    let ms = shrink(m, k);
    let (w, h) = (ms.w, ms.h);
    let fs = f * k;
    let y = Plane { w, h, d: (0..w * h).map(|i| LUM[0] * small[0].d[i] + LUM[1] * small[1].d[i] + LUM[2] * small[2].d[i]).collect() };
    let red = small[0].zip(&small[1], |a, b| a - b);
    let sig = y.zip(&red, |a, b| -a + 1.2 * b);
    let wm = ms.map(|v| v + 1e-3);
    let sigmas: Vec<f32> = [260.0, 180.0, 125.0, 90.0, 65.0, 48.0].iter().map(|d| fs / d).collect();
    let resp: Vec<Plane> = sigmas
        .iter()
        .map(|&sg| {
            let d = blur(&sig, sg).zip(&norm_blur(&sig, &wm, sg * 2.2), |a, b| a - b);
            let mad = masked_median(&d.map(f32::abs), &ms, 0.6) + 1e-5;
            d.map(|v| v / mad)
        })
        .collect();
    let mut spots = Vec::new();
    for (si, r) in resp.iter().enumerate() {
        for yy in 1..h.saturating_sub(1) {
            for xx in 1..w.saturating_sub(1) {
                let v = r.at(xx, yy);
                if v <= 5.5 || ms.at(xx, yy) <= 0.7 {
                    continue;
                }
                let mut is_max = true;
                'n: for sj in si.saturating_sub(1)..(si + 2).min(resp.len()) {
                    for dy in 0..3 {
                        for dx in 0..3 {
                            if resp[sj].at(xx + dx - 1, yy + dy - 1) > v {
                                is_max = false;
                                break 'n;
                            }
                        }
                    }
                }
                if !is_max {
                    continue;
                }
                let rad = sigmas[si] * 1.6;
                let rr = (rad * 2.5).max(2.0);
                let (x0, x1) = ((xx as f32 - rr).max(0.0) as usize, ((xx as f32 + rr + 1.0) as usize).min(w));
                let (y0, y1) = ((yy as f32 - rr).max(0.0) as usize, ((yy as f32 + rr + 1.0) as usize).min(h));
                let (mut iy, mut ir, mut ni) = (0.0, 0.0, 0.0);
                let (mut ry, mut rrd) = (Vec::new(), Vec::new());
                for py in y0..y1 {
                    for px in x0..x1 {
                        let d = ((px as f32 - xx as f32).powi(2) + (py as f32 - yy as f32).powi(2)).sqrt();
                        if d <= rad {
                            iy += y.at(px, py);
                            ir += red.at(px, py);
                            ni += 1.0;
                        } else if d > rad * 1.6 && d <= rr {
                            ry.push(y.at(px, py));
                            rrd.push(red.at(px, py));
                        }
                    }
                }
                if ni < 1.0 || ry.len() < 4 {
                    continue;
                }
                let med = |v: &mut Vec<f32>| {
                    let k = v.len() / 2;
                    *v.select_nth_unstable_by(k, |a, b| a.partial_cmp(b).unwrap()).1
                };
                let dark = med(&mut ry) - iy / ni; // > 0 = darker than around
                let redder = ir / ni - med(&mut rrd); // > 0 = redder
                let kind = if dark > 0.30 && redder < 0.04 && rad > fs / 160.0 {
                    SpotKind::Mole
                } else if redder < 0.025 && dark < 0.18 {
                    SpotKind::Freckle
                } else if dark < -0.05 {
                    SpotKind::Highlight
                } else {
                    SpotKind::Blemish
                };
                spots.push(Spot { x: xx as f32 / k, y: yy as f32 / k, r: rad / k, kind, dark });
            }
        }
    }
    // a spot inside a bigger one is the same spot
    spots.sort_by(|a, b| b.r.partial_cmp(&a.r).unwrap());
    let mut out: Vec<Spot> = Vec::new();
    for s in spots {
        if out.iter().all(|o| ((s.x - o.x).powi(2) + (s.y - o.y).powi(2)).sqrt() >= o.r) {
            out.push(s);
        }
    }
    out
}

/// Helper for tools: retouch a file in one go (finds faces on a preview-size copy).
pub fn faces_for(photo: &Photo) -> Result<Vec<Face>, RetouchError> {
    let max = photo.width.max(photo.height);
    if max > crate::ANALYSIS_SIZE {
        let small = crate::decode::shrink_photo(photo, crate::ANALYSIS_SIZE);
        find_faces(&small)
    } else {
        find_faces(photo)
    }
}

pub fn model_file_in(dir: &Path) -> PathBuf {
    dir.join(PARSER_FILE)
}
