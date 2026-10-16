//! Export a grade as an industry-standard .cube 3D LUT.
//! Photoshop: Layer > New Adjustment Layer > Color Lookup > 3DLUT File > Load 3D LUT…

use crate::grade::Grade;
use std::fmt::Write as _;

pub fn cube_string(grade: &Grade, size: usize, title: &str) -> String {
    let size = size.clamp(2, 65);
    let mut s = String::with_capacity(size * size * size * 28 + 200);
    let clean: String = title.chars().filter(|c| c.is_ascii() && *c != '"' && !c.is_control()).collect();
    let _ = writeln!(s, "TITLE \"{}\"", clean);
    let _ = writeln!(s, "# Made with JoshGrade - offline automatic colour grading");
    let _ = writeln!(s, "LUT_3D_SIZE {}", size);
    let _ = writeln!(s, "DOMAIN_MIN 0.0 0.0 0.0");
    let _ = writeln!(s, "DOMAIN_MAX 1.0 1.0 1.0");
    let d = (size - 1) as f32;
    // .cube ordering: red changes fastest, then green, then blue.
    for b in 0..size {
        for g in 0..size {
            for r in 0..size {
                let o = grade.apply_px([r as f32 / d, g as f32 / d, b as f32 / d]);
                let _ = writeln!(s, "{:.6} {:.6} {:.6}", o[0], o[1], o[2]);
            }
        }
    }
    s
}
