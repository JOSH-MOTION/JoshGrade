use grade_core::*;
use std::path::Path;
use std::time::Instant;
fn main() {
    let p = std::env::args().nth(1).unwrap();
    let path = Path::new(&p);
    let t = Instant::now();
    let (pp, fd) = decode::load_preview(path, ANALYSIS_SIZE).unwrap();
    println!("load_preview {:?} {}x{} full {:?}", t.elapsed(), pp.width, pp.height, fd);
    let t = Instant::now();
    let (mut img, o) = decode::load_dynamic(path).unwrap();
    img.apply_orientation(o);
    println!("decode full {:?} {}x{}", t.elapsed(), img.width(), img.height());
    let t = Instant::now();
    let photo = decode::to_photo(img, Some(ANALYSIS_SIZE), decode::is_raw(path));
    println!("to_photo {:?} {}x{}", t.elapsed(), photo.width, photo.height);
    if pp.width == photo.width && pp.height == photo.height {
        let d: f32 = pp.data.iter().zip(&photo.data).map(|(a, b)| (0..3).map(|k| (a[k] - b[k]).abs()).sum::<f32>()).sum::<f32>() / (pp.data.len() * 3) as f32;
        println!("mean abs diff preview-path vs full-path: {d:.4}");
    } else {
        println!("sizes differ {}x{} vs {}x{}", pp.width, pp.height, photo.width, photo.height);
    }
    let t = Instant::now();
    let a = analyze(&photo);
    println!("analyze {:?}", t.elapsed());
    let mut s: Settings = serde_json::from_str(r#"{"auto":true,"look":"natural","strength":1.0}"#).unwrap();
    s.adjust.exposure = 10.0;
    for i in 0..3 {
        let t = Instant::now();
        let g = grade_with(&a, &s).unwrap();
        let t1 = t.elapsed();
        let out = g.apply(&photo);
        let t2 = t.elapsed();
        let out2 = detail::apply(&out, &s.detail, 0.25);
        let t3 = t.elapsed();
        let f = detail::Frame::of_crop(&s.crop, out2.width as f32, out2.height as f32);
        let out3 = detail::finish_frame(&out2, &s.detail, &f, 0.25);
        let t4 = t.elapsed();
        let j = encode::jpeg_bytes(&out3, 88).unwrap();
        let t5 = t.elapsed();
        let sc = scopes::compute(&out3);
        let t6 = t.elapsed();
        let r = report::measure(&out3);
        let t7 = t.elapsed();
        println!("run{i}: grade_with {:?} apply {:?} detail {:?} finish {:?} jpeg {:?} ({}KB) scopes {:?} measure {:?}", t1, t2 - t1, t3 - t2, t4 - t3, t5 - t4, j.len() / 1024, t6 - t5, t7 - t6);
        let _ = (sc, r);
    }
    {
        let g = grade_with(&a, &s).unwrap();
        let t = Instant::now();
        let ex = g.apply_exact(&photo);
        let te = t.elapsed();
        let t = Instant::now();
        let lu = g.apply(&photo);
        let tl = t.elapsed();
        let mut mx = 0f32;
        let mut mean = 0f32;
        for (a, b) in ex.data.iter().zip(&lu.data) {
            for k in 0..3 {
                let d = (a[k] - b[k]).abs();
                mx = mx.max(d);
                mean += d;
            }
        }
        println!("exact {:?} lut {:?} mean diff {:.5} max diff {:.4} (in 8-bit: mean {:.2} max {:.1})", te, tl, mean / (ex.data.len() * 3) as f32, mx, mean / (ex.data.len() * 3) as f32 * 255.0, mx * 255.0);
    }
    s.detail.clarity = 30.0;
    s.detail.noise = 30.0;
    s.detail.sharpen = 40.0;
    let g = grade_with(&a, &s).unwrap();
    let out = g.apply(&photo);
    let t = Instant::now();
    let _ = detail::apply(&out, &s.detail, 0.25);
    println!("detail with clarity+NR+sharpen {:?}", t.elapsed());
}
#[allow(dead_code)]
fn _x() {}
