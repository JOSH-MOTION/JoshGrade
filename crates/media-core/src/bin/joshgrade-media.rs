//! Test/batch tool:
//!   joshgrade-media sync --ref REC.wav CLIP1.mp4 CLIP2.mov [--out DIR] [--xml timeline.xml]
//!   joshgrade-media captions MODEL.bin VIDEO [--lang en] [--burn bold] [--out DIR]
use media_core::*;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let tools = ffmpeg::Tools::locate().expect("ffmpeg");
    let cancel = AtomicBool::new(false);
    let flag = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    match args.first().map(|s| s.as_str()) {
        // joshgrade-media edit OUT.mp4 CLIP[@s-e,s-e] ... [--nosilence] [--lut F.cube]
        Some("edit") => {
            let out = PathBuf::from(&args[1]);
            let mut clips = vec![];
            for a in args[2..].iter().filter(|a| !a.starts_with("--") && !a.ends_with(".cube")) {
                let (p, segs) = a.split_once('@').unwrap_or((a.as_str(), ""));
                let mut keep: Vec<edit::Segment> = segs.split(',').filter(|x| !x.is_empty()).map(|r| {
                    let (x, y) = r.split_once('-').unwrap();
                    edit::Segment { start: x.parse().unwrap(), end: y.parse().unwrap() }
                }).collect();
                let info = ffmpeg::probe(&tools, std::path::Path::new(p)).unwrap();
                if args.iter().any(|a| a == "--nosilence") {
                    let sil = edit::find_silences(&tools, std::path::Path::new(p), -35.0, 0.5, &cancel).unwrap();
                    println!("{p}: {} pauses", sil.len());
                    keep = edit::keep_without(&sil, info.duration, 0.12);
                    println!("  keep {:?}", keep.iter().map(|s| (s.start * 100.0).round() / 100.0..(s.end * 100.0).round() / 100.0).collect::<Vec<_>>());
                }
                clips.push(edit::EditClip { path: p.to_string(), info: Some(info), keep });
            }
            let opts = edit::ExportOptions { lut: flag("--lut"), ..Default::default() };
            let t = std::time::Instant::now();
            let mut last = 0.0;
            edit::export(&tools, &clips, &opts, &out, &cancel, |p| { if p - last > 0.25 { last = p; eprintln!("  {:.0}%", p * 100.0); } }).unwrap();
            println!("wrote {} in {:.1}s", out.display(), t.elapsed().as_secs_f64());
        }
        Some("sync") => {
            let mut refs = vec![];
            let mut clips = vec![];
            let mut i = 1;
            while i < args.len() {
                match args[i].as_str() {
                    "--ref" => { refs.push(PathBuf::from(&args[i + 1])); i += 2; }
                    "--out" | "--xml" | "--json" => i += 2,
                    a => { clips.push(PathBuf::from(a)); i += 1; }
                }
            }
            let t = std::time::Instant::now();
            let rep = sync::sync(&tools, &clips, &refs, &cancel, &mut |p| eprintln!("  {} ({}/{})", p.step, p.done, p.total)).unwrap();
            for m in &rep.matches {
                println!("{:40} offset {:+10.4}s  drift {:+7.1} ppm  score {:.3}  {}", ffmpeg::file_name(std::path::Path::new(&m.clip)), m.offset, m.drift_ppm, m.score, m.confidence);
            }
            eprintln!("sync took {:.1}s", t.elapsed().as_secs_f32());
            if let Some(j) = flag("--json") {
                std::fs::write(&j, serde_json::to_string(&rep).unwrap()).unwrap();
            }
            if let Some(out) = flag("--out") {
                for m in rep.matches.iter().filter(|m| m.reference.is_some() && Some(&m.clip) != m.reference.as_ref()) {
                    let p = export::render_synced(&tools, m, std::path::Path::new(&out), &export::RenderOptions { keep_camera_audio: true, fix_drift: true }).unwrap();
                    println!("wrote {}", p.display());
                }
            }
            if let Some(x) = flag("--xml") {
                std::fs::write(&x, export::timeline_xml(&rep.matches, &rep.references)).unwrap();
                println!("wrote {x}");
            }
        }
        Some("captions") => {
            let ctx = captions::load_model(std::path::Path::new(&args[1])).unwrap();
            let video = PathBuf::from(&args[2]);
            let lang = flag("--lang").unwrap_or("auto".into());
            let t = std::time::Instant::now();
            let tr = captions::transcribe(&tools, &ctx, &video, &captions::TranscribeOptions { language: lang, translate: false }, &cancel, &mut |p| eprintln!("  {:.0}% {}", p.percent, p.latest)).unwrap();
            eprintln!("transcribed {:.1}s of audio in {:.1}s, language {}", tr.duration, t.elapsed().as_secs_f32(), tr.language);
            if let Some(j) = flag("--json") {
                std::fs::write(&j, serde_json::to_string(&tr).unwrap()).unwrap();
            }
            let cues = captions::build_cues(&tr.words, &captions::CueStyle { max_chars: 42, max_lines: 2, max_words: 0, uppercase: false });
            print!("{}", captions::to_srt(&cues));
            if let Some(j) = flag("--json") {
                std::fs::write(format!("{j}.cues.json"), serde_json::to_string(&cues).unwrap()).unwrap();
            }
            let social = captions::build_cues(&tr.words, &captions::CueStyle { max_chars: 20, max_lines: 1, max_words: 3, uppercase: true });
            eprintln!("social style: {} captions, first: {:?}", social.len(), social.first().map(|c| &c.text));
            if let (Some(tpls), Some(out)) = (flag("--templates"), flag("--out")) {
                let fonts = flag("--fonts").map(PathBuf::from);
                for id in tpls.split(',') {
                    let t = templates::template(id).expect("template");
                    let info = ffmpeg::probe(&tools, &video).unwrap();
                    let cues = captions::build_cues(&tr.words, &templates::layout(t, info.width, info.height, 1.0));
                    if let Some(a) = flag("--ass") {
                        std::fs::write(format!("{a}.{id}.ass"), templates::to_ass(&cues, t, &templates::RenderOpts { width: 1080, height: 1920, scale: 1.0, position: String::new() })).unwrap();
                    }
                    let p = captions::burn_template(&tools, &video, &cues, &captions::TemplateStyle { template: id.into(), scale: 1.0, position: String::new() }, fonts.as_deref(), std::path::Path::new(&out)).unwrap();
                    println!("{id}: {}", p.display());
                }
            }
            if let (Some(preset), Some(out)) = (flag("--burn"), flag("--out")) {
                let style = captions::BurnStyle { preset: preset.clone(), size: 5.0, position: "bottom".into() };
                let use_cues = if preset == "bold" { &social } else { &cues };
                let p = captions::burn_captions(&tools, &video, use_cues, &style, std::path::Path::new(&out)).unwrap();
                println!("burned {}", p.display());
                let p = captions::add_caption_track(&tools, &video, &cues, &tr.language, std::path::Path::new(&out)).unwrap();
                println!("soft track {}", p.display());
            }
        }
        Some("templates") => println!("{}", serde_json::to_string(&templates::TEMPLATES.to_vec()).unwrap()),
        _ => eprintln!("usage: joshgrade-media sync|captions|templates ..."),
    }
}
