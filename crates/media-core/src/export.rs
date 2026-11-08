//! Exports for synced clips: new video files with the recorder's sound laid in, and an XML
//! timeline that Premiere Pro and DaVinci Resolve import (File › Import).

use crate::ffmpeg::{self, MediaInfo, Tools};
use crate::sync::Match;
use crate::Error;
use serde::Deserialize;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize)]
pub struct RenderOptions {
    /// Keep the camera's own sound as a second audio track.
    pub keep_camera_audio: bool,
    /// Correct clock drift by resampling the recorder audio.
    pub fix_drift: bool,
}

/// Write `clip` with the matched recorder audio. Video is copied untouched (no quality loss).
pub fn render_synced(tools: &Tools, m: &Match, out_dir: &Path, opts: &RenderOptions) -> Result<PathBuf, Error> {
    let reference = m.reference.as_ref().ok_or_else(|| Error::Other("this clip has no match".into()))?;
    let clip = Path::new(&m.clip);
    let ext = clip.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).unwrap_or_default();
    // MP4 can't hold uncompressed audio; everything else becomes .mov with 24-bit PCM (what editors want).
    let (out_ext, acodec): (&str, &[&str]) = if ext == "mp4" || ext == "m4v" { ("mp4", &["-c:a", "aac", "-b:a", "320k"]) } else { ("mov", &["-c:a", "pcm_s24le"]) };
    let stem = clip.file_stem().and_then(|s| s.to_str()).unwrap_or("clip");
    std::fs::create_dir_all(out_dir)?;
    let mut out = out_dir.join(format!("{stem}_synced.{out_ext}"));
    let mut n = 2;
    while out.exists() {
        out = out_dir.join(format!("{stem}_synced_{n}.{out_ext}"));
        n += 1;
    }

    let dur = m.clip_info.duration;
    let drift = if opts.fix_drift { m.drift_ppm / 1e6 } else { 0.0 };
    let mut filter = String::new();
    if m.offset < 0.0 {
        // clip starts before the recorder was rolling: pad the front with silence
        let _ = write!(filter, "adelay=delays={:.0}:all=1,", -m.offset * 1000.0);
    }
    if drift.abs() > 1e-6 {
        // squeeze/stretch by the drift (inaudible pitch change of a few hundredths of a cent)
        let rate = 48000.0 * (1.0 + drift);
        let _ = write!(filter, "aresample=48000,asetrate={rate:.3},aresample=48000,");
    }
    let _ = write!(filter, "apad,atrim=0:{dur:.6},asetpts=N/SR/TB");

    let mut cmd = tools.ffmpeg_cmd();
    cmd.arg("-i").arg(clip);
    if m.offset > 0.0 {
        cmd.args(["-ss", &format!("{:.6}", m.offset)]);
    }
    cmd.arg("-i").arg(reference);
    cmd.args(["-filter_complex", &format!("[1:a:0]{filter}[rec]")]);
    cmd.args(["-map", "0:v:0", "-map", "[rec]"]);
    if opts.keep_camera_audio && m.clip_info.has_audio {
        cmd.args(["-map", "0:a:0"]);
    }
    cmd.args(["-map_metadata", "0", "-c:v", "copy"]);
    cmd.args(acodec);
    cmd.args(["-metadata:s:a:0", "title=Recorder (synced)"]);
    if opts.keep_camera_audio {
        cmd.args(["-metadata:s:a:1", "title=Camera"]);
    }
    if out_ext == "mp4" {
        cmd.args(["-movflags", "+faststart"]);
    }
    cmd.args(["-t", &format!("{dur:.6}")]);
    cmd.arg(&out);
    ffmpeg::run(cmd, &format!("Saving {}", ffmpeg::file_name(&out)))?;
    Ok(out)
}

/// Premiere / Resolve frame rate: integer timebase + NTSC flag.
fn timebase(info: &MediaInfo) -> (u32, bool) {
    let fps = info.fps();
    if fps <= 0.0 {
        return (25, false);
    }
    let rounded = fps.round();
    let ntsc = (fps - rounded).abs() > 0.01 && (rounded / fps - 1.001).abs() < 0.0005;
    (rounded as u32, ntsc)
}

fn path_url(p: &str) -> String {
    let mut s = String::from("file://localhost");
    let p = p.replace('\\', "/");
    if !p.starts_with('/') {
        s.push('/');
    }
    for b in p.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' | b':' => s.push(b as char),
            _ => {
                let _ = write!(s, "%{b:02X}");
            }
        }
    }
    s
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn rate_xml(tb: u32, ntsc: bool) -> String {
    format!("<rate><timebase>{tb}</timebase><ntsc>{}</ntsc></rate>", if ntsc { "TRUE" } else { "FALSE" })
}

fn file_xml(id: &str, info: &MediaInfo, tb: u32, ntsc: bool, fps: f64, defined: &mut std::collections::HashSet<String>) -> String {
    if !defined.insert(id.to_string()) {
        return format!("<file id=\"{id}\"/>");
    }
    let name = esc(&ffmpeg::file_name(Path::new(&info.path)));
    let frames = (info.duration * fps).round() as i64;
    let mut s = format!(
        "<file id=\"{id}\"><name>{name}</name><pathurl>{}</pathurl>{}<duration>{frames}</duration>",
        esc(&path_url(&info.path)),
        rate_xml(tb, ntsc)
    );
    if let Some(tc) = &info.timecode {
        let _ = write!(s, "<timecode>{}<string>{}</string><displayformat>NDF</displayformat></timecode>", rate_xml(tb, ntsc), esc(tc));
    }
    s.push_str("<media>");
    if info.has_video {
        let _ = write!(s, "<video><samplecharacteristics>{}<width>{}</width><height>{}</height></samplecharacteristics></video>", rate_xml(tb, ntsc), info.width, info.height);
    }
    if info.has_audio {
        let _ = write!(
            s,
            "<audio><samplecharacteristics><depth>16</depth><samplerate>{}</samplerate></samplecharacteristics><channelcount>{}</channelcount></audio>",
            info.sample_rate.max(8000),
            info.audio_channels.max(1)
        );
    }
    s.push_str("</media></file>");
    s
}

/// Build an FCP7-style XML (xmeml v4) with one synced sequence per recorder file.
/// Cameras go on V1, V2… (overlapping clips get their own track), camera sound on matching
/// audio tracks, and the recorder on the track below them.
pub fn timeline_xml(matches: &[Match], references: &[crate::sync::Reference]) -> String {
    let mut x = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE xmeml>\n<xmeml version=\"4\">\n<project><name>JoshGrade Sync</name><children>\n");
    let mut defined = std::collections::HashSet::new();
    let mut uid = 0usize;
    for (ri, r) in references.iter().enumerate() {
        let ms: Vec<&Match> = matches.iter().filter(|m| m.reference.as_deref() == Some(r.path.as_str()) && m.clip != r.path).collect();
        if ms.is_empty() {
            continue;
        }
        // Sequence settings come from the first camera clip.
        let first = ms.iter().find(|m| m.clip_info.has_video).map(|m| &m.clip_info).unwrap_or(&ms[0].clip_info);
        let (tb, ntsc) = timebase(first);
        let fps = if first.fps() > 0.0 { first.fps() } else { tb as f64 };
        let t0 = ms.iter().map(|m| m.offset).fold(0.0f64, f64::min);
        let fr = |t: f64| ((t - t0) * fps).round() as i64;

        // Assign overlapping clips to separate tracks.
        let mut order: Vec<&Match> = ms.clone();
        order.sort_by(|a, b| a.offset.partial_cmp(&b.offset).unwrap());
        let mut track_end: Vec<i64> = Vec::new();
        let mut placed: Vec<(usize, &Match, i64, i64)> = Vec::new();
        for m in order {
            let s = fr(m.offset);
            let e = s + (m.clip_info.duration * fps).round() as i64;
            let t = match track_end.iter().position(|&end| end <= s) {
                Some(t) => t,
                None => {
                    track_end.push(i64::MIN);
                    track_end.len() - 1
                }
            };
            track_end[t] = e;
            placed.push((t, m, s, e));
        }
        let n_tracks = track_end.len();
        let rec_frames = (r.info.duration * fps).round() as i64;
        let seq_end = placed.iter().map(|p| p.3).max().unwrap_or(0).max(fr(0.0) + rec_frames);
        let rec_name = esc(&ffmpeg::file_name(Path::new(&r.path)));
        let fmt = format!(
            "<format><samplecharacteristics>{}<width>{}</width><height>{}</height><pixelaspectratio>square</pixelaspectratio></samplecharacteristics></format>",
            rate_xml(tb, ntsc), first.width.max(16), first.height.max(16)
        );
        let _ = write!(x, "<sequence id=\"sequence-{}\"><name>Synced – {rec_name}</name><duration>{seq_end}</duration>{}<media>\n<video>{fmt}\n", ri + 1, rate_xml(tb, ntsc));

        let mut ids: Vec<(usize, usize, String, String, String)> = Vec::new(); // (track, idx, vid id, aud id, file id)
        for (t, m, _, _) in &placed {
            uid += 1;
            let file_id = format!("file-{}", sanitize_id(&m.clip));
            ids.push((*t, uid, format!("clipitem-v{uid}"), format!("clipitem-a{uid}"), file_id));
            let _ = m;
        }
        for t in 0..n_tracks {
            x.push_str("<track>\n");
            let mut idx = 0;
            for (k, (pt, m, s, e)) in placed.iter().enumerate() {
                if *pt != t || !m.clip_info.has_video {
                    continue;
                }
                idx += 1;
                let (_, _, vid, aud, fid) = &ids[k];
                let frames = e - s;
                let _ = write!(
                    x,
                    "<clipitem id=\"{vid}\"><name>{}</name><enabled>TRUE</enabled><duration>{frames}</duration>{}<start>{s}</start><end>{e}</end><in>0</in><out>{frames}</out>{}",
                    esc(&ffmpeg::file_name(Path::new(&m.clip))), rate_xml(tb, ntsc),
                    file_xml(fid, &m.clip_info, tb, ntsc, fps, &mut defined)
                );
                let _ = write!(x, "<link><linkclipref>{vid}</linkclipref><mediatype>video</mediatype><trackindex>{}</trackindex><clipindex>{idx}</clipindex></link>", t + 1);
                if m.clip_info.has_audio {
                    let _ = write!(x, "<link><linkclipref>{aud}</linkclipref><mediatype>audio</mediatype><trackindex>{}</trackindex><clipindex>{idx}</clipindex></link>", t + 1);
                }
                x.push_str("</clipitem>\n");
            }
            x.push_str("</track>\n");
        }
        x.push_str("</video>\n<audio>\n");
        // camera sound
        for t in 0..n_tracks {
            x.push_str("<track>\n");
            for (k, (pt, m, s, e)) in placed.iter().enumerate() {
                if *pt != t || !m.clip_info.has_audio {
                    continue;
                }
                let (_, _, _, aud, fid) = &ids[k];
                let frames = e - s;
                let _ = write!(
                    x,
                    "<clipitem id=\"{aud}\"><name>{}</name><enabled>TRUE</enabled><duration>{frames}</duration>{}<start>{s}</start><end>{e}</end><in>0</in><out>{frames}</out>{}<sourcetrack><mediatype>audio</mediatype><trackindex>1</trackindex></sourcetrack></clipitem>\n",
                    esc(&ffmpeg::file_name(Path::new(&m.clip))), rate_xml(tb, ntsc),
                    file_xml(fid, &m.clip_info, tb, ntsc, fps, &mut defined)
                );
            }
            x.push_str("</track>\n");
        }
        // recorder
        uid += 1;
        let rs = fr(0.0);
        let _ = write!(
            x,
            "<track>\n<clipitem id=\"clipitem-r{uid}\"><name>{rec_name}</name><enabled>TRUE</enabled><duration>{rec_frames}</duration>{}<start>{rs}</start><end>{}</end><in>0</in><out>{rec_frames}</out>{}<sourcetrack><mediatype>audio</mediatype><trackindex>1</trackindex></sourcetrack></clipitem>\n</track>\n",
            rate_xml(tb, ntsc), rs + rec_frames,
            file_xml(&format!("file-{}", sanitize_id(&r.path)), &r.info, tb, ntsc, fps, &mut defined)
        );
        x.push_str("</audio>\n</media></sequence>\n");
    }
    x.push_str("</children></project>\n</xmeml>\n");
    x
}

fn sanitize_id(p: &str) -> String {
    let mut h: u64 = 1469598103934665603;
    for b in p.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(1099511628211);
    }
    format!("{:x}", h)
}
