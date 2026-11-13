//! Visual check of export options and the focus mask: exportcheck <photo> <outdir>
use grade_core::output::{ExportOptions, Sharpen, Watermark};
use std::path::Path;
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let (input, out) = (Path::new(&a[1]), Path::new(&a[2]));
    let s = grade_core::Settings::default();
    let o = ExportOptions { long_edge: 1200, quality: 90, sharpen: Sharpen::Screen, watermark: Watermark { text: "© Josh Doe Photography".into(), ..Default::default() }, ..Default::default() };
    let t = std::time::Instant::now();
    grade_core::save_graded_with(input, &out.join("export.jpg"), &s, grade_core::OutputFormat::Jpeg, &o, false).unwrap();
    println!("export {:?}", t.elapsed());
    let (p, _) = grade_core::decode::load_preview(input, 1600).unwrap();
    let m = grade_core::output::focus_mask(&p);
    let img = image::RgbImage::from_fn(p.width, p.height, |x, y| {
        let i = (y * p.width + x) as usize;
        let c = p.data[i];
        let a = m[i] * 0.8;
        image::Rgb([((c[0] * (1.0 - a) + 0.27 * a) * 255.0) as u8, ((c[1] * (1.0 - a) + a) * 255.0) as u8, ((c[2] * (1.0 - a) + 0.35 * a) * 255.0) as u8])
    });
    img.save(out.join("focus.jpg")).unwrap();
}
