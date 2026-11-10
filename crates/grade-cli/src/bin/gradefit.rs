//! Development tool: learn how professionals grade.
//!
//!   gradefit fit PAIRS_DIR OUT.jsonl [--aug]
//!
//! For every `NNNNN_in.png` / `NNNNN_ex.png` pair (original / expert edit) it finds the
//! JoshGrade correction that best reproduces the expert, and writes the photo's features,
//! that correction and how close each method gets (mean ΔE, lower is better).

use grade_core::analyze::{analyze, AutoParams};
use grade_core::color::Rgb;
use grade_core::learn::{self, lab, Correction, RANGES};
use grade_core::{find_look, Adjust, Grade, Photo, Settings};
use rayon::prelude::*;
use std::io::Write;
use std::path::Path;

fn load(p: &Path) -> Photo {
    let img = image::open(p).expect("open").to_rgb32f();
    let (w, h) = img.dimensions();
    Photo { width: w, height: h, data: img.into_raw().chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect(), from_raw: false }
}

pub fn render(img: &Photo, c: &Correction, base: &AutoParams) -> Photo {
    let (auto, sat) = c.to_auto(base);
    let look = find_look("normal").unwrap();
    let adj = Adjust { saturation: sat * 100.0, ..Default::default() };
    Grade::with_adjust(&auto, &look.params, &adj, 1.0).apply(img)
}

fn delta_e(a: &Photo, b_lab: &[[f32; 3]]) -> f32 {
    a.data.iter().zip(b_lab).map(|(p, q)| {
        let l = lab(*p);
        ((l[0] - q[0]).powi(2) + (l[1] - q[1]).powi(2) + (l[2] - q[2]).powi(2)).sqrt()
    }).sum::<f32>() / a.data.len() as f32
}

/// Nelder–Mead in normalised parameter space.
fn fit(img: &Photo, target: &[[f32; 3]], base: &AutoParams, starts: &[Correction]) -> (Correction, f32) {
    let span: Vec<f32> = RANGES.iter().map(|r| r.1 - r.0).collect();
    let to_c = |x: &[f32]| {
        let mut c = [0f32; 10];
        for i in 0..10 {
            c[i] = RANGES[i].0 + x[i] * span[i];
        }
        Correction(c).clamped()
    };
    let to_x = |c: &Correction| -> Vec<f32> { (0..10).map(|i| (c.0[i] - RANGES[i].0) / span[i]).collect() };
    let loss = |x: &[f32]| delta_e(&render(img, &to_c(x), base), target);
    let mut best = (Correction::identity(), f32::MAX);
    for s in starts {
        let x0 = to_x(s);
        let n = 10;
        let mut simplex: Vec<(Vec<f32>, f32)> = vec![(x0.clone(), loss(&x0))];
        for i in 0..n {
            let mut x = x0.clone();
            x[i] = if x[i] > 0.5 { x[i] - 0.08 } else { x[i] + 0.08 };
            let l = loss(&x);
            simplex.push((x, l));
        }
        for _ in 0..320 {
            simplex.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
            if (simplex[n].1 - simplex[0].1).abs() < 1e-4 {
                break;
            }
            let cen: Vec<f32> = (0..n).map(|j| simplex[..n].iter().map(|s| s.0[j]).sum::<f32>() / n as f32).collect();
            let pt = |t: f32| -> Vec<f32> { (0..n).map(|j| (cen[j] + t * (simplex[n].0[j] - cen[j])).clamp(-0.05, 1.05)).collect() };
            let xr = pt(-1.0);
            let lr = loss(&xr);
            if lr < simplex[0].1 {
                let xe = pt(-2.0);
                let le = loss(&xe);
                simplex[n] = if le < lr { (xe, le) } else { (xr, lr) };
            } else if lr < simplex[n - 1].1 {
                simplex[n] = (xr, lr);
            } else {
                let xc = pt(0.5);
                let lc = loss(&xc);
                if lc < simplex[n].1 {
                    simplex[n] = (xc, lc);
                } else {
                    let b0 = simplex[0].0.clone();
                    for s in simplex.iter_mut().skip(1) {
                        s.0 = (0..n).map(|j| b0[j] + 0.5 * (s.0[j] - b0[j])).collect();
                        s.1 = loss(&s.0);
                    }
                }
            }
        }
        simplex.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        if simplex[0].1 < best.1 {
            best = (to_c(&simplex[0].0), simplex[0].1);
        }
    }
    best
}

/// Small deterministic random numbers for augmentation.
fn rnd(seed: u64, k: u64) -> f32 {
    let mut x = seed.wrapping_mul(6364136223846793005).wrapping_add(k.wrapping_mul(1442695040888963407));
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51afd7ed558ccd);
    x ^= x >> 33;
    (x % 1_000_000) as f32 / 1_000_000.0 * 2.0 - 1.0
}

fn degrade(ex: &Photo, seed: u64) -> Photo {
    // A badly shot / badly processed version of a well-graded photo.
    let c = Correction([
        rnd(seed, 1) * 1.1,
        rnd(seed, 2) * 0.22,
        rnd(seed, 3) * 0.12,
        (rnd(seed, 4) * 0.06).max(-0.04),
        1.0 + rnd(seed, 5) * 0.12,
        rnd(seed, 6) * 0.2,
        rnd(seed, 7) * 0.3,
        0.88 + rnd(seed, 8) * 0.06,
        rnd(seed, 9) * 0.3,
        rnd(seed, 10) * 0.3,
    ])
    .clamped();
    render(ex, &c, &AutoParams::identity())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 || args[1] != "fit" {
        eprintln!("gradefit fit PAIRS_DIR OUT.jsonl [--aug]");
        std::process::exit(2);
    }
    let dir = Path::new(&args[2]);
    let aug = args.iter().any(|a| a == "--aug");
    let mut ids: Vec<String> = std::fs::read_dir(dir).unwrap().filter_map(|e| {
        let n = e.ok()?.file_name().to_string_lossy().into_owned();
        n.strip_suffix("_in.png").map(String::from)
    }).collect();
    ids.sort();
    let out = std::sync::Mutex::new(std::fs::File::create(&args[3]).unwrap());
    let done = std::sync::atomic::AtomicUsize::new(0);
    let total = ids.len();
    ids.par_iter().for_each(|id| {
        let inp = load(&dir.join(format!("{id}_in.png")));
        let ex = load(&dir.join(format!("{id}_ex.png")));
        let mut cases: Vec<(String, Photo)> = vec![("fivek".into(), inp)];
        if aug {
            let seed = id.parse::<u64>().unwrap_or(1);
            cases.push(("good".into(), ex.clone()));
            cases.push(("bad".into(), degrade(&ex, seed)));
        }
        let ex_small = grade_core::decode::shrink_photo(&ex, 170);
        let target: Vec<[f32; 3]> = ex_small.data.iter().map(|p: &Rgb| lab(*p)).collect();
        for (kind, img) in cases {
            let an = analyze(&img);
            let feats = learn::features(&img, &an.params);
            let small = grade_core::decode::shrink_photo(&img, 170);
            let rules = Correction::from_auto(&an.params);
            let l_id = delta_e(&small, &target);
            let l_rules = delta_e(&render(&small, &rules, &an.params), &target);
            let cur = |look: &str| {
                let s = Settings { look: look.into(), ..Default::default() };
                let g = grade_core::grade_with(&an, &s).unwrap();
                delta_e(&g.apply(&small), &target)
            };
            let (l_nat, l_norm) = (cur("natural"), cur("normal"));
            // An already well-graded photo should be left alone: its answer is "no change".
            let (c, l_fit) = if kind == "good" { (Correction::identity(), l_id) } else { fit(&small, &target, &an.params, &[rules, Correction::identity()]) };
            let line = serde_json::json!({
                "id": id, "kind": kind, "features": feats, "fit": c.0,
                "loss": {"identity": l_id, "rules": l_rules, "natural": l_nat, "normal": l_norm, "fit": l_fit}
            });
            writeln!(out.lock().unwrap(), "{line}").unwrap();
        }
        let d = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        if d % 20 == 0 {
            eprintln!("{d}/{total}");
        }
    });
}
