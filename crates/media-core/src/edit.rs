//! Simple video editing: keep parts of clips (trim, cut out, remove silences), join clips
//! into one video, and grade it with a LUT — all in one ffmpeg pass with progress.

use crate::ffmpeg::{self, MediaInfo, Tools};
use crate::Error;
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Segment {
    pub start: f64,
    pub end: f64,
}

impl Segment {
    pub fn len(&self) -> f64 {
        (self.end - self.start).max(0.0)
    }
}

/// One clip on the edit: the file and the parts of it to keep, in order.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditClip {
    pub path: String,
    #[serde(default)]
    pub info: Option<MediaInfo>,
    /// Parts to keep (seconds). Empty = the whole clip.
    #[serde(default)]
    pub keep: Vec<Segment>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ExportOptions {
    /// .cube LUT to grade the picture with.
    pub lut: Option<String>,
    /// Output size; None = the first clip's size.
    pub width: Option<u32>,
    pub height: Option<u32>,
}

/// Normalise keep-lists: clamp to the clip, sort, merge overlaps, drop slivers.
pub fn clean(keep: &[Segment], duration: f64) -> Vec<Segment> {
    let mut v: Vec<Segment> = keep
        .iter()
        .map(|s| Segment { start: s.start.clamp(0.0, duration), end: s.end.clamp(0.0, duration) })
        .filter(|s| s.len() > 0.04)
        .collect();
    v.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap());
    let mut out: Vec<Segment> = Vec::new();
    for s in v {
        match out.last_mut() {
            Some(l) if s.start <= l.end + 0.02 => l.end = l.end.max(s.end),
            _ => out.push(s),
        }
    }
    if out.is_empty() && keep.is_empty() {
        out.push(Segment { start: 0.0, end: duration });
    }
    out
}

/// Hardware H.264 on Macs (fast, cool laptop), x264 elsewhere.
fn video_codec(tools: &Tools, w: u32, h: u32, fps: f64) -> Vec<String> {
    let has_vt = std::process::Command::new(&tools.ffmpeg)
        .args(["-hide_banner", "-encoders"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("h264_videotoolbox"))
        .unwrap_or(false);
    if has_vt {
        // ~0.2 bits per pixel per frame: visually clean for edited footage.
        let kbps = ((w as f64 * h as f64 * fps.max(24.0) * 0.2) / 1000.0).clamp(4_000.0, 80_000.0) as u64;
        vec!["-c:v".into(), "h264_videotoolbox".into(), "-b:v".into(), format!("{kbps}k"), "-allow_sw".into(), "1".into(), "-profile:v".into(), "high".into()]
    } else {
        vec!["-c:v".into(), "libx264".into(), "-preset".into(), "veryfast".into(), "-crf".into(), "18".into()]
    }
}

/// Escape a path for use inside an ffmpeg filter argument.
pub fn filter_path(p: &str) -> String {
    p.replace('\\', "/").replace(':', "\\:").replace('\'', "\\'")
}

/// Export the edit as one MP4. `progress(fraction)` is called as it renders.
pub fn export(
    tools: &Tools,
    clips: &[EditClip],
    opts: &ExportOptions,
    out: &Path,
    cancel: &AtomicBool,
    mut progress: impl FnMut(f32),
) -> Result<PathBuf, Error> {
    let mut infos = Vec::new();
    for c in clips {
        let info = match &c.info {
            Some(i) if i.duration > 0.0 => i.clone(),
            _ => ffmpeg::probe(tools, Path::new(&c.path))?,
        };
        infos.push(info);
    }
    let first = infos.iter().find(|i| i.has_video).ok_or_else(|| Error::Other("Add at least one video.".into()))?;
    let (w, h) = (opts.width.unwrap_or(first.width).max(16) / 2 * 2, opts.height.unwrap_or(first.height).max(16) / 2 * 2);
    let fps = if first.fps() > 1.0 { first.fps() } else { 30.0 };
    let fps_str = if first.fps_num > 0 && first.fps_den > 0 { format!("{}/{}", first.fps_num, first.fps_den) } else { "30".into() };

    let mut graph = String::new();
    let mut parts: Vec<(String, String)> = Vec::new(); // (video label, audio label) in order
    let mut total = 0.0;
    for (i, (c, info)) in clips.iter().zip(&infos).enumerate() {
        let segs = clean(&c.keep, info.duration);
        if segs.is_empty() {
            continue;
        }
        let n = segs.len();
        if info.has_video {
            graph.push_str(&format!("[{i}:v:0]split={n}"));
            for k in 0..n {
                graph.push_str(&format!("[sv{i}_{k}]"));
            }
            graph.push(';');
        }
        if info.has_audio {
            graph.push_str(&format!("[{i}:a:0]asplit={n}"));
            for k in 0..n {
                graph.push_str(&format!("[sa{i}_{k}]"));
            }
            graph.push(';');
        }
        for (k, s) in segs.iter().enumerate() {
            let (v, a) = (format!("v{i}_{k}"), format!("a{i}_{k}"));
            if info.has_video {
                graph.push_str(&format!(
                    "[sv{i}_{k}]trim=start={:.4}:end={:.4},setpts=PTS-STARTPTS,scale={w}:{h}:force_original_aspect_ratio=decrease,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2,setsar=1,fps={fps_str},format=yuv420p[{v}];",
                    s.start, s.end
                ));
            } else {
                graph.push_str(&format!("color=c=black:s={w}x{h}:r={fps_str}:d={:.4},format=yuv420p[{v}];", s.len()));
            }
            if info.has_audio {
                // tiny fades so cuts never click
                let fade = (s.len() / 4.0).min(0.012);
                graph.push_str(&format!(
                    "[sa{i}_{k}]atrim=start={:.4}:end={:.4},asetpts=PTS-STARTPTS,aresample=48000,aformat=channel_layouts=stereo,afade=t=in:d={fade:.4},afade=t=out:st={:.4}:d={fade:.4}[{a}];",
                    s.start, s.end, (s.len() - fade).max(0.0)
                ));
            } else {
                graph.push_str(&format!("anullsrc=r=48000:cl=stereo,atrim=duration={:.4}[{a}];", s.len()));
            }
            parts.push((v, a));
            total += s.len();
        }
    }
    if parts.is_empty() {
        return Err(Error::Other("Nothing left to export: every part was cut.".into()));
    }
    for (v, a) in &parts {
        graph.push_str(&format!("[{v}][{a}]"));
    }
    graph.push_str(&format!("concat=n={}:v=1:a=1[cv][ca]", parts.len()));
    let vout = if let Some(l) = &opts.lut {
        graph.push_str(&format!(";[cv]lut3d=file='{}':interp=tetrahedral,format=yuv420p[gv]", filter_path(l)));
        "[gv]"
    } else {
        "[cv]"
    };
    // Long filter graphs go in a file (command lines have a length limit).
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let tmp = std::env::temp_dir().join(format!("joshgrade-edit-{}-{}.txt", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    std::fs::write(&tmp, &graph)?;
    if let Some(d) = out.parent() {
        std::fs::create_dir_all(d)?;
    }
    let mut cmd = tools.ffmpeg_cmd();
    cmd.arg("-y");
    for c in clips {
        cmd.arg("-i").arg(&c.path);
    }
    cmd.arg(if script_flag_new(tools) { "-/filter_complex" } else { "-filter_complex_script" }).arg(&tmp);
    cmd.args(["-map", vout, "-map", "[ca]"]);
    cmd.args(video_codec(tools, w, h, fps));
    cmd.args(["-pix_fmt", "yuv420p", "-c:a", "aac", "-b:a", "256k", "-movflags", "+faststart"]);
    cmd.arg(out);
    let r = run_with_progress(cmd, total, cancel, &mut progress);
    let _ = std::fs::remove_file(&tmp);
    if r.is_err() {
        let _ = std::fs::remove_file(out);
    }
    r.map(|_| out.to_path_buf())
}

/// Run ffmpeg, reporting progress (0..1) from its `-progress` output; cancellable.
pub fn run_with_progress(mut cmd: std::process::Command, total: f64, cancel: &AtomicBool, progress: &mut dyn FnMut(f32)) -> Result<(), Error> {
    cmd.args(["-progress", "pipe:1", "-nostats"]);
    let mut child = cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    let stderr = child.stderr.take().unwrap();
    let err_thread = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = BufReader::new(stderr).read_to_string(&mut s);
        s
    });
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::Cancelled);
        }
        match rx.recv_timeout(std::time::Duration::from_millis(200)) {
            Ok(line) => {
                if let Some(v) = line.strip_prefix("out_time_us=").or_else(|| line.strip_prefix("out_time_ms=")) {
                    if let Ok(us) = v.trim().parse::<f64>() {
                        progress(((us / 1e6) / total.max(0.01)).clamp(0.0, 1.0) as f32);
                    }
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let status = child.wait()?;
    let err = err_thread.join().unwrap_or_default();
    if status.success() {
        progress(1.0);
        Ok(())
    } else {
        let last = err.lines().filter(|l| !l.trim().is_empty()).last().unwrap_or("unknown error").to_string();
        Err(Error::Other(format!("Export failed: {last}")))
    }
}

/// Find pauses: stretches quieter than `threshold_db` for at least `min_len` seconds.
pub fn find_silences(tools: &Tools, input: &Path, threshold_db: f64, min_len: f64, cancel: &AtomicBool) -> Result<Vec<Segment>, Error> {
    let mut cmd = std::process::Command::new(&tools.ffmpeg);
    cmd.args(["-hide_banner", "-nostdin", "-i"]).arg(input);
    cmd.args(["-vn", "-sn", "-dn", "-af", &format!("silencedetect=noise={threshold_db}dB:d={min_len}"), "-f", "null", "-"]);
    let mut child = cmd.stdout(Stdio::null()).stderr(Stdio::piped()).spawn()?;
    let stderr = child.stderr.take().unwrap();
    let mut out = Vec::new();
    let mut start: Option<f64> = None;
    for line in BufReader::new(stderr).lines().map_while(Result::ok) {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            return Err(Error::Cancelled);
        }
        if let Some(i) = line.find("silence_start: ") {
            start = line[i + 15..].split_whitespace().next().and_then(|v| v.parse().ok());
        } else if let Some(i) = line.find("silence_end: ") {
            let end: Option<f64> = line[i + 13..].split_whitespace().next().and_then(|v| v.parse().ok());
            if let (Some(s), Some(e)) = (start.take(), end) {
                out.push(Segment { start: s.max(0.0), end: e });
            }
        }
    }
    let _ = child.wait();
    if let Some(s) = start {
        // silence running to the end of the file
        if let Ok(info) = ffmpeg::probe(tools, input) {
            out.push(Segment { start: s, end: info.duration });
        }
    }
    Ok(out)
}

/// The parts to keep once the pauses are taken out. `pad` seconds of each pause stay on
/// both sides so speech never sounds clipped and breaths feel natural.
pub fn keep_without(silences: &[Segment], duration: f64, pad: f64) -> Vec<Segment> {
    let mut keep = Vec::new();
    let mut t = 0.0;
    for s in silences {
        let (a, b) = (s.start + pad, s.end - pad);
        if b - a < 0.1 {
            continue; // too short to bother cutting once padded
        }
        if a > t {
            keep.push(Segment { start: t, end: a });
        }
        t = b;
    }
    if t < duration {
        keep.push(Segment { start: t, end: duration });
    }
    clean(&keep, duration)
}

/// Grab one frame as a small RGB image (for previews and for analysing the grade).
pub fn frame_rgb(tools: &Tools, input: &Path, t: f64, max_side: u32) -> Result<(u32, u32, Vec<u8>), Error> {
    let info = ffmpeg::probe(tools, input)?;
    let s = max_side as f64 / info.width.max(info.height).max(1) as f64;
    let (w, h) = (((info.width as f64 * s).round() as u32).max(2) / 2 * 2, ((info.height as f64 * s).round() as u32).max(2) / 2 * 2);
    let out = tools
        .ffmpeg_cmd()
        .args(["-ss", &format!("{:.3}", t.max(0.0))])
        .arg("-i")
        .arg(input)
        .args(["-frames:v", "1", "-vf", &format!("scale={w}:{h}"), "-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
        .output()?;
    if out.stdout.len() < (w * h * 3) as usize {
        return Err(Error::Other(format!("couldn't read a frame from {}", ffmpeg::file_name(input))));
    }
    Ok((w, h, out.stdout[..(w * h * 3) as usize].to_vec()))
}

/// ffmpeg 7.1+ reads option values from files with `-/option`; older ones use `-filter_complex_script`.
fn script_flag_new(tools: &Tools) -> bool {
    let out = std::process::Command::new(&tools.ffmpeg).arg("-version").output();
    let v = out.map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
    // "ffmpeg version 7.1.1 ..." / "ffmpeg version N-118000-g..." (git builds are new)
    let ver = v.split_whitespace().nth(2).unwrap_or("");
    if ver.starts_with('N') {
        return true;
    }
    let mut it = ver.split(['.', '-']).map(|x| x.parse::<u32>().unwrap_or(0));
    let (maj, min) = (it.next().unwrap_or(0), it.next().unwrap_or(0));
    maj > 7 || (maj == 7 && min >= 1)
}
