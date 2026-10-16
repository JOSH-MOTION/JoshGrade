//! Command-line front end, handy for testing and for batch work:
//!   joshgrade-cli [--look cinematic] [--strength 0.8] [--out DIR] [--format jpeg|tiff|png] [--lut] PHOTOS_OR_FOLDERS...
use grade_core::*;
use std::path::PathBuf;

fn main() {
    let mut job = Job::default();
    let mut out: Option<PathBuf> = None;
    let mut inputs = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--look" => job.look = args.next().unwrap_or_default(),
            "--strength" => job.strength = args.next().and_then(|s| s.parse().ok()).unwrap_or(1.0),
            "--out" => out = args.next().map(PathBuf::from),
            "--format" => {
                job.format = match args.next().as_deref() {
                    Some("tiff") | Some("tif") => OutputFormat::Tiff,
                    Some("png") => OutputFormat::Png,
                    _ => OutputFormat::Jpeg,
                }
            }
            "--lut" => job.write_lut = true,
            "--looks" => {
                for l in all_looks() {
                    println!("{:10} {}", l.id, l.description);
                }
                return;
            }
            "-h" | "--help" => {
                println!("joshgrade-cli [--look ID] [--strength 0..1.5] [--out DIR] [--format jpeg|tiff|png] [--lut] PHOTOS_OR_FOLDERS...\njoshgrade-cli --looks   (list looks)");
                return;
            }
            _ => inputs.push(PathBuf::from(a)),
        }
    }
    let files = collect_inputs(&inputs);
    if files.is_empty() {
        eprintln!("No supported photos given. Try --help.");
        std::process::exit(1);
    }
    for f in files {
        let dir = out.clone().unwrap_or_else(|| f.parent().unwrap_or(std::path::Path::new(".")).join("Graded"));
        let t = std::time::Instant::now();
        match process_file(&f, &dir, &job) {
            Ok(r) => {
                println!("✓ {} -> {} ({:.1}s)", f.display(), r.output.display(), t.elapsed().as_secs_f32());
                for n in r.notes {
                    println!("    • {}", n);
                }
            }
            Err(e) => eprintln!("✗ {}: {}", f.display(), e),
        }
    }
}
