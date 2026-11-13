//! PluralEyes-style automatic sync: find where each camera clip's sound sits inside the
//! separately recorded audio, to the sample, and measure clock drift on long clips.
//!
//! 1. Coarse: both recordings are reduced to a loudness "fingerprint" (100 per second) and
//!    slid past each other with an FFT cross-correlation. Works across very different
//!    microphones because it compares *when* things get loud, not what the sound looks like.
//! 2. Fine: around the best match, the raw audio is aligned with GCC-PHAT, which is precise
//!    to a fraction of a millisecond and robust to echo and mic colour.
//! 3. Drift: on long clips the fine step runs again near the end; the difference between the
//!    two offsets is the clock drift between the camera and the recorder.

use crate::ffmpeg::{self, MediaInfo, Tools};
use crate::Error;
use rustfft::{num_complex::Complex32, FftPlanner};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

const FEAT_SR: u32 = 8000;
const HOP: usize = 80; // 10 ms
const WIN: usize = 160; // 20 ms
pub const FEAT_RATE: f64 = FEAT_SR as f64 / HOP as f64; // 100 frames / second
const FINE_SR: u32 = 16000;

/// Loudness fingerprint of one recording.
#[derive(Debug, Clone)]
pub struct Fingerprint {
    pub path: PathBuf,
    pub info: MediaInfo,
    /// Normalised features, FEAT_RATE per second.
    pub feat: Vec<f32>,
    /// Raw log-energy per frame (for choosing where to refine).
    pub energy: Vec<f32>,
}

pub fn fingerprint(tools: &Tools, path: &Path, cancel: &AtomicBool) -> Result<Fingerprint, Error> {
    let info = ffmpeg::probe(tools, path)?;
    if !info.has_audio {
        return Err(Error::Other(format!("{} has no sound to sync with", ffmpeg::file_name(path))));
    }
    // One-pole high-pass (~150 Hz) removes wind / handling rumble that differs between mics.
    let a = (-2.0 * std::f32::consts::PI * 150.0 / FEAT_SR as f32).exp();
    let (mut px, mut py) = (0f32, 0f32);
    let mut buf: Vec<f32> = Vec::with_capacity(WIN * 4);
    let mut energy: Vec<f32> = Vec::with_capacity((info.duration * FEAT_RATE) as usize + 16);
    let mut aborted = false;
    ffmpeg::stream_audio(tools, path, FEAT_SR, None, None, |chunk| {
        if aborted || cancel.load(Ordering::Relaxed) {
            aborted = true;
            return;
        }
        for &x in chunk {
            let y = a * (py + x - px);
            px = x;
            py = y;
            buf.push(y);
            if buf.len() >= WIN {
                let e = buf[..WIN].iter().map(|v| v * v).sum::<f32>() / WIN as f32;
                energy.push((e + 1e-9).ln());
                buf.drain(..HOP);
            }
        }
    })?;
    if aborted {
        return Err(Error::Cancelled);
    }
    if energy.len() < 50 {
        return Err(Error::Other(format!("{} is too short to sync (needs at least half a second of sound)", ffmpeg::file_name(path))));
    }
    Ok(Fingerprint { path: path.to_path_buf(), info, feat: features(&energy), energy })
}

/// Local-contrast log energy (what's louder than the last half second), blended with onsets,
/// then normalised. Silence floors are clamped so background hiss doesn't create false matches.
fn features(energy: &[f32]) -> Vec<f32> {
    let n = energy.len();
    let mut sorted: Vec<f32> = energy.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let floor = sorted[n / 10] + 1.0; // a bit above the quietest 10% = room tone
    let e: Vec<f32> = energy.iter().map(|v| v.max(floor)).collect();
    // moving average over ±25 frames via prefix sums
    let mut pre = vec![0f64; n + 1];
    for i in 0..n {
        pre[i + 1] = pre[i] + e[i] as f64;
    }
    let mut f: Vec<f32> = (0..n)
        .map(|i| {
            let a = i.saturating_sub(25);
            let b = (i + 26).min(n);
            let mean = ((pre[b] - pre[a]) / (b - a) as f64) as f32;
            let local = (e[i] - mean).clamp(-4.0, 4.0);
            let onset = if i > 0 { (e[i] - e[i - 1]).max(0.0).min(4.0) } else { 0.0 };
            local + 0.7 * onset
        })
        .collect();
    let mean = f.iter().sum::<f32>() / n as f32;
    let var = f.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / n as f32;
    let sd = var.sqrt().max(1e-6);
    for v in f.iter_mut() {
        *v = (*v - mean) / sd;
    }
    f
}

#[derive(Debug, Clone, Serialize)]
pub struct Coarse {
    /// Recorder frame where the clip starts (may be negative).
    pub lag: i64,
    pub score: f32,
    pub second: f32,
    pub overlap_frames: usize,
}

/// Slide `clip` along `reference` and find the best normalised correlation.
pub fn coarse_match(reference: &[f32], clip: &[f32]) -> Option<Coarse> {
    let (n, m) = (reference.len(), clip.len());
    if n == 0 || m == 0 {
        return None;
    }
    let min_ov = m.min(((m as f64 * 0.5) as usize).max(500)).max(50);
    let size = (n + m).next_power_of_two();
    let mut planner = FftPlanner::<f32>::new();
    let fwd = planner.plan_fft_forward(size);
    let inv = planner.plan_fft_inverse(size);
    let mut r: Vec<Complex32> = (0..size).map(|i| Complex32::new(if i < n { reference[i] } else { 0.0 }, 0.0)).collect();
    let mut c: Vec<Complex32> = (0..size).map(|i| Complex32::new(if i < m { clip[i] } else { 0.0 }, 0.0)).collect();
    fwd.process(&mut r);
    fwd.process(&mut c);
    for i in 0..size {
        r[i] = r[i] * c[i].conj();
    }
    inv.process(&mut r);
    let scale = 1.0 / size as f32;

    let mut pr = vec![0f64; n + 1];
    for i in 0..n {
        pr[i + 1] = pr[i] + (reference[i] * reference[i]) as f64;
    }
    let mut pc = vec![0f64; m + 1];
    for i in 0..m {
        pc[i + 1] = pc[i] + (clip[i] * clip[i]) as f64;
    }

    let lo = -(m as i64 - min_ov as i64);
    let hi = n as i64 - min_ov as i64;
    let mut scores: Vec<(i64, f32, usize)> = Vec::with_capacity((hi - lo + 1).max(0) as usize);
    for k in lo..=hi {
        let a = (-k).max(0) as usize;
        let b = (m as i64).min(n as i64 - k) as usize;
        if b <= a || b - a < min_ov {
            continue;
        }
        let idx = if k >= 0 { k as usize } else { (size as i64 + k) as usize };
        let dot = r[idx].re * scale;
        let ec = pc[b] - pc[a];
        let er = pr[(k + b as i64) as usize] - pr[(k + a as i64) as usize];
        let norm = (ec * er).sqrt() as f32;
        if norm > 0.0 {
            // Small penalty for short overlaps so a lucky 5 s blip can't beat a full-length match.
            let ov = (b - a) as f32 / m as f32;
            scores.push((k, dot / norm * (0.85 + 0.15 * ov), b - a));
        }
    }
    let &(lag, score, overlap_frames) = scores.iter().max_by(|a, b| a.1.partial_cmp(&b.1).unwrap())?;
    let second = scores
        .iter()
        .filter(|s| (s.0 - lag).abs() > 50)
        .map(|s| s.1)
        .fold(f32::MIN, f32::max)
        .max(0.0);
    Some(Coarse { lag, score, second, overlap_frames })
}

/// GCC-PHAT: offset (in samples) of `needle` inside `hay`, with sub-sample precision.
/// Returns (offset, peak sharpness).
fn gcc_phat(hay: &[f32], needle: &[f32], max_lag: usize) -> Option<(f64, f32)> {
    gcc_phat_sr(hay, needle, max_lag, FINE_SR)
}

fn gcc_phat_sr(hay: &[f32], needle: &[f32], max_lag: usize, sr: u32) -> Option<(f64, f32)> {
    if hay.is_empty() || needle.is_empty() {
        return None;
    }
    let size = (hay.len() + needle.len()).next_power_of_two();
    let mut planner = FftPlanner::<f32>::new();
    let fwd = planner.plan_fft_forward(size);
    let inv = planner.plan_fft_inverse(size);
    let mut h: Vec<Complex32> = (0..size).map(|i| Complex32::new(*hay.get(i).unwrap_or(&0.0), 0.0)).collect();
    let mut nd: Vec<Complex32> = (0..size).map(|i| Complex32::new(*needle.get(i).unwrap_or(&0.0), 0.0)).collect();
    fwd.process(&mut h);
    fwd.process(&mut nd);
    let bin_hz = sr as f32 / size as f32;
    for i in 0..size {
        let f = (if i <= size / 2 { i } else { size - i }) as f32 * bin_hz;
        let x = h[i] * nd[i].conj();
        let mag = x.norm();
        // speech band only: ignores rumble and hiss that differ between mics
        h[i] = if (150.0..=(sr as f32 * 0.4).min(6000.0)).contains(&f) && mag > 1e-12 { x / mag } else { Complex32::new(0.0, 0.0) };
    }
    inv.process(&mut h);
    let lim = max_lag.min(size - 1);
    let (mut best, mut bv) = (0usize, f32::MIN);
    for (i, v) in h.iter().enumerate().take(lim + 1) {
        if v.re > bv {
            bv = v.re;
            best = i;
        }
    }
    // sharpness = peak vs. average absolute correlation in the search range
    let avg = h[..=lim].iter().map(|v| v.re.abs()).sum::<f32>() / (lim + 1) as f32;
    let sharp = if avg > 0.0 { bv / avg } else { 0.0 };
    let y0 = if best > 0 { h[best - 1].re } else { bv };
    let y2 = if best < lim { h[best + 1].re } else { bv };
    let denom = y0 - 2.0 * bv + y2;
    let frac = if denom.abs() > 1e-9 { 0.5 * (y0 - y2) / denom } else { 0.0 };
    Some((best as f64 + frac.clamp(-0.5, 0.5) as f64, sharp))
}

/// Slow, very robust fallback for clips the loudness match can't place (e.g. a camera far away
/// in a loud room): whitened waveform correlation of up to 40 s of the clip across the whole
/// recording, block by block. Returns (coarse offset of clip t=0, sharpness).
fn deep_search(tools: &Tools, reference: &Fingerprint, clip: &Fingerprint, cancel: &AtomicBool) -> Option<(f64, f32)> {
    const SR: u32 = 4000;
    let len = clip.info.duration.min(40.0);
    let t_clip = ((clip.info.duration - len) / 2.0).max(0.0);
    let needle = ffmpeg::read_audio(tools, &clip.path, SR, Some(t_clip), Some(len)).ok()?;
    if needle.len() < SR as usize * 2 {
        return None;
    }
    let block = (1usize << 20).max(needle.len() * 4); // ~4 minutes per block, overlapping
    let step = block - needle.len();
    let total = (reference.info.duration * SR as f64) as usize;
    let mut best: Option<(f64, f32)> = None;
    let mut start = 0usize;
    while start < total {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        let hay = ffmpeg::read_audio(tools, &reference.path, SR, Some(start as f64 / SR as f64), Some(block as f64 / SR as f64)).ok()?;
        if hay.len() > needle.len() {
            if let Some((lag, sharp)) = gcc_phat_sr(&hay, &needle, hay.len() - needle.len(), SR) {
                if best.map(|b| sharp > b.1).unwrap_or(true) {
                    best = Some(((start as f64 + lag) / SR as f64 - t_clip, sharp));
                }
            }
        }
        start += step;
    }
    best
}

/// Pick the `len`-second window of the clip with the most going on (speech, claps…).
fn busiest_window(energy: &[f32], from: f64, to: f64, len: f64) -> f64 {
    let w = (len * FEAT_RATE) as usize;
    let a = (from * FEAT_RATE).max(0.0) as usize;
    let b = ((to * FEAT_RATE) as usize).min(energy.len());
    if b <= a + w {
        return from.max(0.0);
    }
    let (mut best, mut bs) = (a, f32::MIN);
    let mut i = a;
    while i + w <= b {
        let seg = &energy[i..i + w];
        let mean = seg.iter().sum::<f32>() / w as f32;
        let var = seg.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>();
        if var > bs {
            bs = var;
            best = i;
        }
        i += 50;
    }
    best as f64 / FEAT_RATE
}

/// Fine-align one window. Returns (recorder time of clip t=0, sharpness).
fn refine_at(tools: &Tools, reference: &Path, clip: &Path, coarse_offset: f64, clip_t: f64, len: f64) -> Option<(f64, f32)> {
    const MARGIN: f64 = 0.15;
    let needle = ffmpeg::read_audio(tools, clip, FINE_SR, Some(clip_t), Some(len)).ok()?;
    let rec_start = coarse_offset + clip_t - MARGIN;
    let pad_front = (-rec_start).max(0.0);
    let mut hay = vec![0f32; (pad_front * FINE_SR as f64) as usize];
    hay.extend(ffmpeg::read_audio(tools, reference, FINE_SR, Some(rec_start.max(0.0)), Some(len + 2.0 * MARGIN - pad_front)).ok()?);
    if hay.len() < FINE_SR as usize / 2 || needle.len() < FINE_SR as usize / 2 {
        return None;
    }
    let (d, sharp) = gcc_phat(&hay, &needle, (2.0 * MARGIN * FINE_SR as f64) as usize)?;
    Some((rec_start + d / FINE_SR as f64 - clip_t, sharp))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Match {
    pub clip: String,
    /// The recording this clip was matched to (None = no reliable match).
    pub reference: Option<String>,
    /// Where the clip starts on the recording's timeline, in seconds (may be negative).
    pub offset: f64,
    /// Camera clock vs. recorder clock, parts per million (+ = recorder runs slow).
    pub drift_ppm: f64,
    /// 0..1 match strength.
    pub score: f32,
    /// "high", "medium", "low" or "none".
    pub confidence: String,
    /// Seconds of the clip that overlap the recording.
    pub overlap: f64,
    pub clip_info: MediaInfo,
    /// Loudness envelope of the clip, 10 per second, 0..1 (for drawing).
    pub envelope: Vec<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reference {
    pub path: String,
    pub info: MediaInfo,
    pub envelope: Vec<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncReport {
    pub references: Vec<Reference>,
    pub matches: Vec<Match>,
    /// Where each reference sits on the one shared timeline (seconds), same order as `references`.
    #[serde(default)]
    pub positions: Vec<f64>,
}

/// 10 Hz 0..1 loudness envelope for drawing waveforms.
pub fn envelope(energy: &[f32]) -> Vec<f32> {
    let mut v: Vec<f32> = energy.chunks(10).map(|c| c.iter().cloned().fold(f32::MIN, f32::max)).collect();
    let mut s = v.clone();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let lo = s[s.len() / 20];
    let hi = s[s.len() - 1 - s.len() / 200].max(lo + 1.0);
    for x in v.iter_mut() {
        *x = ((*x - lo) / (hi - lo)).clamp(0.0, 1.0);
    }
    v
}

#[derive(Debug, Clone, Serialize)]
pub struct Progress {
    pub step: String,
    pub done: usize,
    pub total: usize,
}

/// Sync every clip against the reference recordings. With no references, the longest clip
/// becomes the reference (camera-to-camera sync).
pub fn sync(
    tools: &Tools,
    clips: &[PathBuf],
    references: &[PathBuf],
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(Progress),
) -> Result<SyncReport, Error> {
    // Full paths, so exported timelines can always find the media.
    let abs = |p: &PathBuf| std::fs::canonicalize(p).unwrap_or_else(|_| p.clone());
    let clips: Vec<PathBuf> = clips.iter().map(abs).collect();
    let references: Vec<PathBuf> = references.iter().map(abs).collect();
    let (clips, references) = (&clips[..], &references[..]);
    let total = clips.len() + references.len() + clips.len();
    let mut done = 0usize;
    let step = |p: &mut dyn FnMut(Progress), what: String, done: usize| p(Progress { step: what, done, total });

    let mut refs: Vec<Fingerprint> = Vec::new();
    for r in references {
        step(progress, format!("Listening to {}", ffmpeg::file_name(r)), done);
        refs.push(fingerprint(tools, r, cancel)?);
        done += 1;
    }
    let mut prints: Vec<Result<Fingerprint, Error>> = Vec::new();
    for c in clips {
        step(progress, format!("Listening to {}", ffmpeg::file_name(c)), done);
        let fp = fingerprint(tools, c, cancel);
        if let Err(Error::Cancelled) = fp {
            return Err(Error::Cancelled);
        }
        prints.push(fp);
        done += 1;
    }
    if refs.is_empty() {
        // camera-to-camera: longest clip with sound is the reference
        let best = prints
            .iter()
            .enumerate()
            .filter_map(|(i, p)| p.as_ref().ok().map(|p| (i, p.info.duration)))
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        if let Some((i, _)) = best {
            refs.push(prints[i].as_ref().unwrap().clone());
        }
    }

    let mut matches = Vec::new();
    for (ci, fp) in prints.into_iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        step(progress, format!("Matching {}", ffmpeg::file_name(&clips[ci])), done);
        done += 1;
        let fp = match fp {
            Ok(f) => f,
            Err(_) => {
                let info = ffmpeg::probe(tools, &clips[ci]).unwrap_or_default();
                matches.push(Match {
                    clip: clips[ci].to_string_lossy().into_owned(),
                    reference: None, offset: 0.0, drift_ppm: 0.0, score: 0.0, confidence: "none".into(),
                    overlap: 0.0, clip_info: info, envelope: vec![],
                });
                continue;
            }
        };
        // Same file as the reference (camera-to-camera mode): it is the timeline.
        if let Some(r) = refs.iter().find(|r| r.path == fp.path) {
            matches.push(Match {
                clip: fp.path.to_string_lossy().into_owned(),
                reference: Some(r.path.to_string_lossy().into_owned()),
                offset: 0.0, drift_ppm: 0.0, score: 1.0, confidence: "high".into(),
                overlap: fp.info.duration, envelope: envelope(&fp.energy), clip_info: fp.info,
            });
            continue;
        }
        let mut best: Option<(usize, Coarse)> = None;
        for (ri, r) in refs.iter().enumerate() {
            if let Some(c) = coarse_match(&r.feat, &fp.feat) {
                if best.as_ref().map(|b| c.score > b.1.score).unwrap_or(true) {
                    best = Some((ri, c));
                }
            }
        }
        let Some((mut ri, c)) = best else {
            // No match anywhere: keep it in the report so it's shown (and still exported).
            matches.push(Match {
                clip: fp.path.to_string_lossy().into_owned(),
                reference: None, offset: 0.0, drift_ppm: 0.0, score: 0.0, confidence: "none".into(),
                overlap: 0.0, envelope: envelope(&fp.energy), clip_info: fp.info,
            });
            continue;
        };
        let mut coarse_offset = c.lag as f64 / FEAT_RATE;
        let mut c = c;
        // Weak loudness match: try the deep search before giving up. (A strong match that also
        // appears elsewhere — the same song played twice — can't be settled by more searching.)
        if c.score < 0.22 {
            step(progress, format!("Searching harder for {}", ffmpeg::file_name(&clips[ci])), done);
            let mut deep_best: Option<(usize, f64, f32)> = None;
            for (rj, r) in refs.iter().enumerate() {
                if let Some((o, sh)) = deep_search(tools, r, &fp, cancel) {
                    if deep_best.map(|b| sh > b.2).unwrap_or(true) {
                        deep_best = Some((rj, o, sh));
                    }
                }
            }
            if let Some((rj, o, sh)) = deep_best {
                if sh >= 12.0 {
                    ri = rj;
                    coarse_offset = o;
                    // re-score the loudness match at this position for reporting
                    let lag = (o * FEAT_RATE).round() as i64;
                    c = Coarse { lag, score: c.score.max(0.22), second: c.second.min(0.1), overlap_frames: c.overlap_frames };
                }
            }
        }
        let r = &refs[ri];
        let ratio = c.score / c.second.max(0.02);

        // Fine alignment (+ drift on long clips), only inside the overlapping part.
        // Always attempted: its sharpness is the second, independent piece of evidence.
        let dur = fp.info.duration;
        let ov_from = (-coarse_offset).max(0.0);
        let ov_to = dur.min(r.info.duration - coarse_offset);
        let win = 12.0f64.min((ov_to - ov_from).max(0.5));
        let mut offset = coarse_offset;
        let mut drift = 0.0;
        let mut sharp = 0.0f32;
        let span = ov_to - ov_from;
        if span > 0.5 && c.score > 0.08 {
            let t1 = if span > 90.0 {
                busiest_window(&fp.energy, ov_from, ov_from + span * 0.4, win)
            } else {
                busiest_window(&fp.energy, ov_from, ov_to, win)
            };
            if let Some((o1, s1)) = refine_at(tools, &r.path, &fp.path, coarse_offset, t1, win) {
                sharp = s1;
                if s1 > 8.0 {
                    offset = o1;
                    if span > 90.0 {
                        let t2 = busiest_window(&fp.energy, ov_from + span * 0.6, ov_to, win);
                        if let Some((o2, s2)) = refine_at(tools, &r.path, &fp.path, o1, t2, win) {
                            if s2 > 8.0 && t2 - t1 > 30.0 {
                                let d = (o2 - o1) / (t2 - t1);
                                // Real clock drift is tiny (< 300 ppm); anything bigger is a bad read.
                                if d.abs() < 300e-6 {
                                    drift = d;
                                    offset = o1 - d * t1;
                                }
                            }
                        }
                    }
                }
            }
        }
        // Two independent checks: the loudness pattern lines up (score, and stands out from
        // other positions = ratio) and the raw waveforms lock together (sharpness).
        // Unrelated audio scores ~0.15 with sharpness ~3–6; real matches 0.3+ and 15+.
        let confidence = if c.score >= 0.35 && sharp >= 15.0 && ratio >= 1.1 {
            "high"
        } else if c.score >= 0.22 && sharp >= 10.0 {
            "medium"
        } else if c.score >= 0.15 && sharp >= 12.0 {
            "low"
        } else {
            "none"
        };
        if std::env::var_os("JOSHGRADE_DEBUG").is_some() {
            eprintln!("  [debug] {} score {:.3} second {:.3} ratio {:.2} sharp {:.1}", ffmpeg::file_name(&fp.path), c.score, c.second, ratio, sharp);
        }
        matches.push(Match {
            clip: fp.path.to_string_lossy().into_owned(),
            reference: if confidence == "none" { None } else { Some(r.path.to_string_lossy().into_owned()) },
            offset,
            drift_ppm: drift * 1e6,
            score: c.score,
            confidence: confidence.to_string(),
            overlap: (c.overlap_frames as f64 / FEAT_RATE).min(dur),
            envelope: envelope(&fp.energy),
            clip_info: fp.info,
        });
    }
    progress(Progress { step: "Done".into(), done: total, total });
    let references: Vec<Reference> = refs.iter().map(|r| Reference { path: r.path.to_string_lossy().into_owned(), info: r.info.clone(), envelope: envelope(&r.energy) }).collect();
    let positions = crate::export::reference_layout(&matches, &references);
    Ok(SyncReport { references, matches, positions })
}
