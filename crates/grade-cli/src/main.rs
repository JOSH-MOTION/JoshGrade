//! Command-line front end, handy for testing and for big batches:
//!   joshgrade-cli [--look cinematic] [--strength 0.8] [--out DIR] [--format jpeg|tiff|png] [--lut] PHOTOS_OR_FOLDERS...
use grade_core::*;
use std::path::PathBuf;

fn main() {
    let mut settings = Settings::default();
    let mut format = OutputFormat::Jpeg;
    let mut write_lut = false;
    let mut out: Option<PathBuf> = None;
    let mut inputs = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--look" => settings.look = args.next().unwrap_or_default(),
            "--strength" => settings.strength = args.next().and_then(|s| s.parse().ok()).unwrap_or(1.0),
            "--out" => out = args.next().map(PathBuf::from),
            "--format" => {
                format = match args.next().as_deref() {
                    Some("tiff") | Some("tif") => OutputFormat::Tiff,
                    Some("png") => OutputFormat::Png,
                    _ => OutputFormat::Jpeg,
                }
            }
            "--lut" => write_lut = true,
            "--looks" => {
                for l in all_looks() {
                    println!("{:10} {}", l.id, l.description);
                }
                return;
            }
            "-h" | "--help" => {
                println!("joshgrade-cli [--look ID] [--strength 0..2] [--out DIR] [--format jpeg|tiff|png] [--lut] PHOTOS_OR_FOLDERS...\njoshgrade-cli --looks   (list looks)");
                return;
            }
            _ => inputs.push(PathBuf::from(a)),
        }
    }
    let (files, skipped) = collect_inputs(&inputs);
    for s in skipped {
        eprintln!("- skipped {} (not a supported photo)", s.display());
    }
    if files.is_empty() {
        eprintln!("No supported photos given. Try --help.");
        std::process::exit(1);
    }
    for f in files {
        let dir = out.clone().unwrap_or_else(|| f.parent().unwrap_or(std::path::Path::new(".")).join("Graded"));
        let dest = unique_output(&f, &dir, format);
        let t = std::time::Instant::now();
        match save_graded(&f, &dest, &settings, format, 95, write_lut) {
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
