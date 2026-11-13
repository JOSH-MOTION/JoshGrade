//! Spectral-peak "landmark" fingerprints (the Shazam / audfprint idea, as used for
//! multi-camera sync by Bryan, Smaragdis & Mysore 2012): pick the strongest peaks in the
//! spectrogram, pair nearby peaks, and hash each pair (frequency, frequency gap, time gap).
//! Two recordings of the same moment share many hashes at one consistent time difference,
//! even with different microphones, distance, reverb, levels and noise — and even if they
//! overlap only for a few seconds. Matching counts shared hashes per time difference.

use rustfft::{num_complex::Complex32, FftPlanner};

pub const SR: u32 = 8000;
const N: usize = 256; // 32 ms window
pub const HOP: usize = 128; // 16 ms
const BINS: usize = N / 2;
const MAX_PER_FRAME: usize = 5;
const FAN_OUT: usize = 5;
const MAX_DT: u32 = 40; // frames (~0.64 s)
const MAX_DF: i32 = 31;

/// Seconds per landmark frame.
pub fn frame_secs() -> f64 {
    HOP as f64 / SR as f64
}

/// Streaming peak picker. Feed it samples at `SR`, then call `finish`.
pub struct Builder {
    fft: std::sync::Arc<dyn rustfft::Fft<f32>>,
    window: Vec<f32>,
    buf: Vec<f32>,
    /// slowly-updated average level per frequency (whitening: different mics/EQ look alike)
    avg: Vec<f32>,
    /// last 5 whitened spectra (ring), for the 2-D local-maximum test
    ring: Vec<Vec<f32>>,
    frame: u32,
    peaks: Vec<(u32, u16)>,
}

impl Builder {
    pub fn new() -> Builder {
        let fft = FftPlanner::<f32>::new().plan_fft_forward(N);
        let window = (0..N).map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / N as f32).cos()).collect();
        Builder { fft, window, buf: Vec::with_capacity(N * 2), avg: vec![f32::NAN; BINS], ring: Vec::new(), frame: 0, peaks: Vec::new() }
    }

    pub fn push(&mut self, samples: &[f32]) {
        for &x in samples {
            self.buf.push(x);
            if self.buf.len() >= N {
                self.frame_done();
                self.buf.drain(..HOP);
            }
        }
    }

    fn frame_done(&mut self) {
        let mut c: Vec<Complex32> = (0..N).map(|i| Complex32::new(self.buf[i] * self.window[i], 0.0)).collect();
        self.fft.process(&mut c);
        // log magnitude; whitened by a slow per-frequency average (so a bassy far camera and a
        // bright close one show the same peaks), and only peaks well above this frame's own
        // floor count (noise makes many small peaks).
        let mut raw = vec![0f32; BINS];
        for k in 1..BINS {
            raw[k] = (c[k].norm_sqr() + 1e-10).ln() * 0.5;
        }
        let mut s = raw[1..].to_vec();
        s.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let floor = s[s.len() / 2];
        let mut w = vec![f32::MIN; BINS];
        for k in 1..BINS {
            let v = raw[k];
            if self.avg[k].is_nan() {
                self.avg[k] = v;
            }
            if v >= floor + 2.0 {
                w[k] = v - self.avg[k] + (v - floor) * 0.5;
            }
            self.avg[k] += 0.01 * (v - self.avg[k]);
        }
        self.ring.push(w);
        if self.ring.len() > 5 {
            self.ring.remove(0);
        }
        if self.ring.len() == 5 {
            // frame in the middle of the ring
            let t = self.frame - 2;
            let mid = &self.ring[2];
            let mut cand: Vec<(f32, usize)> = Vec::new();
            for k in 3..BINS - 3 {
                let v = mid[k];
                if v == f32::MIN {
                    continue;
                }
                let mut is_max = true;
                'n: for f in &self.ring {
                    for kk in k - 3..=k + 3 {
                        if f[kk] > v {
                            is_max = false;
                            break 'n;
                        }
                    }
                }
                if is_max {
                    cand.push((v, k));
                }
            }
            cand.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
            for &(_, k) in cand.iter().take(MAX_PER_FRAME) {
                self.peaks.push((t, k as u16));
            }
        }
        self.frame += 1;
    }

    /// Landmarks as (hash, frame), sorted by hash.
    pub fn finish(self) -> Vec<(u32, u32)> {
        let p = self.peaks;
        let mut out = Vec::with_capacity(p.len() * FAN_OUT);
        for i in 0..p.len() {
            let (t1, k1) = p[i];
            let mut n = 0;
            for &(t2, k2) in &p[i + 1..] {
                if t2 <= t1 {
                    continue;
                }
                let dt = t2 - t1;
                if dt > MAX_DT {
                    break;
                }
                let df = k2 as i32 - k1 as i32;
                if df.abs() > MAX_DF {
                    continue;
                }
                let h = ((k1 as u32 & 127) << 13) | (((df + 32) as u32 & 63) << 7) | (dt & 127);
                out.push((h, t1));
                n += 1;
                if n == FAN_OUT {
                    break;
                }
            }
        }
        out.sort_unstable();
        out
    }
}

/// Best time difference between two landmark sets.
#[derive(Debug, Clone, Copy)]
pub struct Hit {
    /// Frame of `a` where `b` starts (b's frame 0 sits at this frame of a).
    pub lag: i64,
    /// Shared landmarks at that difference (±1 frame).
    pub count: u32,
    /// The best count at any clearly different time difference.
    pub second: u32,
}

impl Hit {
    /// Clearly more than chance: many shared landmarks, standing well above any other position.
    pub fn strong(&self) -> bool {
        self.count >= 8 && self.count as f32 >= 2.5 * self.second.max(2) as f32
    }
    pub fn offset_secs(&self) -> f64 {
        self.lag as f64 * frame_secs()
    }
}

/// Count shared landmarks per time difference (b relative to a).
pub fn best_lag(a: &[(u32, u32)], b: &[(u32, u32)]) -> Option<Hit> {
    const MAX_RUN: usize = 24; // very common hashes say nothing
    let mut votes: Vec<i32> = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        let (ha, hb) = (a[i].0, b[j].0);
        if ha < hb {
            i += 1;
        } else if hb < ha {
            j += 1;
        } else {
            let i2 = i + a[i..].iter().take_while(|x| x.0 == ha).count();
            let j2 = j + b[j..].iter().take_while(|x| x.0 == ha).count();
            if i2 - i <= MAX_RUN && j2 - j <= MAX_RUN {
                for x in &a[i..i2] {
                    for y in &b[j..j2] {
                        votes.push(x.1 as i32 - y.1 as i32);
                    }
                }
            }
            i = i2;
            j = j2;
        }
    }
    if votes.is_empty() {
        return None;
    }
    votes.sort_unstable();
    // counts per lag
    let mut lags: Vec<(i32, u32)> = Vec::new();
    for v in votes {
        match lags.last_mut() {
            Some(l) if l.0 == v => l.1 += 1,
            _ => lags.push((v, 1)),
        }
    }
    let at = |lag: i32| lags.binary_search_by_key(&lag, |l| l.0).map(|i| lags[i].1).unwrap_or(0);
    let (mut best_lag, mut best) = (0, 0u32);
    for &(l, _) in &lags {
        let c = at(l - 1) + at(l) + at(l + 1);
        if c > best {
            best = c;
            best_lag = l;
        }
    }
    let mut second = 0u32;
    for &(l, _) in &lags {
        if (l - best_lag).abs() > 4 {
            second = second.max(at(l - 1) + at(l) + at(l + 1));
        }
    }
    Some(Hit { lag: best_lag as i64, count: best, second })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn finds_offset_through_noise_and_eq() {
        // a "voice": random tone bursts; b = a 3 s later, filtered + noisy + quieter
        let mut rng = 12345u64;
        let mut rnd = || {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            (rng % 10_000) as f32 / 10_000.0
        };
        let sr = SR as usize;
        let mut a = vec![0f32; sr * 30];
        let mut t = 0;
        while t < a.len() - sr {
            let f = 200.0 + rnd() * 2500.0;
            let len = (0.05 + rnd() * 0.2) as f32;
            let n = (len * sr as f32) as usize;
            for i in 0..n {
                a[t + i] += (2.0 * std::f32::consts::PI * f * i as f32 / sr as f32).sin() * 0.3;
            }
            t += n + (rnd() * 0.2 * sr as f32) as usize;
        }
        let shift = 3 * sr;
        let mut b: Vec<f32> = a[shift..shift + 12 * sr].iter().map(|v| v * 0.2).collect();
        let mut lp = 0f32;
        for v in b.iter_mut() {
            lp = 0.7 * lp + 0.3 * *v;
            *v = lp + (rnd() - 0.5) * 0.05;
        }
        let fp = |x: &[f32]| {
            let mut bld = Builder::new();
            bld.push(x);
            bld.finish()
        };
        let h = best_lag(&fp(&a), &fp(&b)).unwrap();
        assert!(h.strong(), "{h:?}");
        assert!((h.offset_secs() - 3.0).abs() < 0.04, "{h:?}");
    }
}
