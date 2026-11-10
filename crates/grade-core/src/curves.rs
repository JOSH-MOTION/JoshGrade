//! Point curves, like Lightroom's tone curve: a master (RGB) curve and one per channel.
//! Points are (input, output) on 0..1 display values; between them a monotone cubic
//! (Fritsch–Carlson) so the curve never overshoots or reverses.

use crate::color::Rgb;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Curves {
    pub rgb: Vec<[f32; 2]>,
    pub r: Vec<[f32; 2]>,
    pub g: Vec<[f32; 2]>,
    pub b: Vec<[f32; 2]>,
}

const N: usize = 1024;

#[derive(Debug, Clone)]
pub struct Tables {
    master: Option<Vec<f32>>,
    ch: [Option<Vec<f32>>; 3],
}

fn is_identity(p: &[[f32; 2]]) -> bool {
    p.len() < 2 || p.iter().all(|q| (q[0] - q[1]).abs() < 1e-4)
}

/// Sample a monotone cubic through the points into a table of N values on 0..1.
pub fn table(points: &[[f32; 2]]) -> Vec<f32> {
    let mut p: Vec<[f32; 2]> = points.iter().map(|q| [q[0].clamp(0.0, 1.0), q[1].clamp(0.0, 1.0)]).collect();
    p.sort_by(|a, b| a[0].partial_cmp(&b[0]).unwrap());
    p.dedup_by(|a, b| (a[0] - b[0]).abs() < 1e-4);
    if p.is_empty() {
        return (0..N).map(|i| i as f32 / (N - 1) as f32).collect();
    }
    if p.len() == 1 {
        return vec![p[0][1]; N];
    }
    let n = p.len();
    let d: Vec<f32> = (0..n - 1).map(|i| (p[i + 1][1] - p[i][1]) / (p[i + 1][0] - p[i][0]).max(1e-6)).collect();
    let mut m = vec![0f32; n];
    m[0] = d[0];
    m[n - 1] = d[n - 2];
    for i in 1..n - 1 {
        m[i] = if d[i - 1] * d[i] <= 0.0 { 0.0 } else { (d[i - 1] + d[i]) / 2.0 };
    }
    for i in 0..n - 1 {
        if d[i].abs() < 1e-9 {
            m[i] = 0.0;
            m[i + 1] = 0.0;
        } else {
            let (a, b) = (m[i] / d[i], m[i + 1] / d[i]);
            let s = a * a + b * b;
            if s > 9.0 {
                let t = 3.0 / s.sqrt();
                m[i] = t * a * d[i];
                m[i + 1] = t * b * d[i];
            }
        }
    }
    (0..N)
        .map(|k| {
            let x = k as f32 / (N - 1) as f32;
            if x <= p[0][0] {
                return p[0][1];
            }
            if x >= p[n - 1][0] {
                return p[n - 1][1];
            }
            let i = p.windows(2).position(|w| x >= w[0][0] && x <= w[1][0]).unwrap_or(0);
            let h = (p[i + 1][0] - p[i][0]).max(1e-6);
            let t = (x - p[i][0]) / h;
            let (t2, t3) = (t * t, t * t * t);
            let y = (2.0 * t3 - 3.0 * t2 + 1.0) * p[i][1] + (t3 - 2.0 * t2 + t) * h * m[i] + (-2.0 * t3 + 3.0 * t2) * p[i + 1][1] + (t3 - t2) * h * m[i + 1];
            y.clamp(0.0, 1.0)
        })
        .collect()
}

impl Curves {
    pub fn tables(&self) -> Option<Tables> {
        let mk = |p: &Vec<[f32; 2]>| if is_identity(p) { None } else { Some(table(p)) };
        let t = Tables { master: mk(&self.rgb), ch: [mk(&self.r), mk(&self.g), mk(&self.b)] };
        (t.master.is_some() || t.ch.iter().any(|c| c.is_some())).then_some(t)
    }
}

#[inline]
fn look(t: &[f32], v: f32) -> f32 {
    let f = v.clamp(0.0, 1.0) * (N - 1) as f32;
    let i = (f as usize).min(N - 2);
    let a = f - i as f32;
    t[i] * (1.0 - a) + t[i + 1] * a
}

impl Tables {
    #[inline]
    pub fn apply(&self, c: Rgb) -> Rgb {
        let mut c = c;
        if let Some(m) = &self.master {
            c = c.map(|v| look(m, v));
        }
        for k in 0..3 {
            if let Some(t) = &self.ch[k] {
                c[k] = look(t, c[k]);
            }
        }
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn straight_and_monotone() {
        let t = table(&[[0.0, 0.0], [1.0, 1.0]]);
        assert!((t[512] - 512.0 / 1023.0).abs() < 1e-3);
        let s = table(&[[0.0, 0.0], [0.25, 0.18], [0.75, 0.85], [1.0, 1.0]]);
        assert!(s.windows(2).all(|w| w[1] >= w[0] - 1e-6));
        assert!(s[256] < 0.25 && s[768] > 0.75);
    }
}
