use std::time::Instant;
fn main() {
    use rawler::imgop::develop::{ProcessingStep, RawDevelop};
    let p = std::env::args().nth(1).unwrap();
    let t = Instant::now();
    let raw = rawler::decode_file(&p).unwrap();
    println!("decode_file {:?}", t.elapsed());
    for (name, steps) in [
        ("full", vec![ProcessingStep::Rescale, ProcessingStep::Demosaic, ProcessingStep::CropActiveArea, ProcessingStep::WhiteBalance, ProcessingStep::Calibrate, ProcessingStep::CropDefault]),
    ] {
        let t = Instant::now();
        let d = RawDevelop::new_with(&steps);
        let i = d.develop_intermediate(&raw).unwrap();
        let (w, h) = match &i { rawler::imgop::develop::Intermediate::ThreeColor(p) => (p.width, p.height), _ => (0, 0) };
        println!("{name} {:?} {w}x{h}", t.elapsed());
    }
}
