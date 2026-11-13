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
    // A camera file is described at its own frame rate (a 29.97 clip in a 25 timeline stays 29.97);
    // sound-only files use the timeline's rate.
    let (tb, ntsc, fps) = if info.has_video && info.fps() > 0.0 {
        let (t, n) = timebase(info);
        (t, n, info.fps())
    } else {
        (tb, ntsc, fps)
    };
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

/// Where each recorder file sits on the one shared timeline (seconds from the timeline's
/// zero, before shifting so nothing starts below 0). Recorder files with a start time are
/// laid out by the clock (so two recorders running together overlap, like a lav and a boom);
/// a long pause between takes is shortened to a 10 s gap, and files without a start time
/// follow on in the order they were added.
pub fn reference_layout(matches: &[Match], references: &[crate::sync::Reference]) -> Vec<f64> {
    const GAP: f64 = 10.0;
    const MAX_REAL_GAP: f64 = 300.0;
    // Each reference's span relative to its own start, including clips that start before or run past it.
    let span = |r: &crate::sync::Reference| {
        let mut lo = 0.0f64;
        let mut hi = r.info.duration;
        for m in matches.iter().filter(|m| m.reference.as_deref() == Some(r.path.as_str())) {
            lo = lo.min(m.offset);
            hi = hi.max(m.offset + m.clip_info.duration);
        }
        (lo, hi)
    };
    let mut order: Vec<usize> = (0..references.len()).collect();
    // Timed files by start time first, then untimed in the given order.
    order.sort_by(|&a, &b| match (references[a].info.start_epoch, references[b].info.start_epoch) {
        (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.cmp(&b),
    });
    let mut pos = vec![0.0; references.len()];
    let mut end = f64::NEG_INFINITY; // timeline end so far
    let mut prev: Option<(f64, f64)> = None; // (epoch, timeline position) of the last timed file
    for &i in &order {
        let (lo, hi) = span(&references[i]);
        let after = if end.is_finite() { end + GAP - lo } else { 0.0 };
        let p = match (references[i].info.start_epoch, prev) {
            (Some(ep), Some((pe, pp))) => {
                let real = pp + (ep - pe);
                // Keep real timing while it's close; overlapping recorders stay overlapped.
                if real + lo - end <= MAX_REAL_GAP { real } else { after }
            }
            (Some(_), None) => 0.0,
            (None, _) => after,
        };
        if let Some(ep) = references[i].info.start_epoch {
            prev = Some((ep, p));
        }
        pos[i] = p;
        end = end.max(p + hi);
    }
    pos
}

/// Build an FCP7-style XML (xmeml v4) with ONE synced sequence holding every recorder file
/// and every matched clip. Cameras go on V1, V2… (overlapping clips get their own track),
/// camera sound on matching audio tracks, and the recorder files on the tracks below them.
pub fn timeline_xml(matches: &[Match], references: &[crate::sync::Reference]) -> String {
    let mut x = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE xmeml>\n<xmeml version=\"4\">\n<project><name>JoshGrade Sync</name><children>\n");
    let mut defined = std::collections::HashSet::new();
    let ref_pos = reference_layout(matches, references);
    let ref_of = |p: &str| references.iter().position(|r| r.path == p);

    // Clips that go on the picture/camera-sound tracks, with their timeline start in seconds.
    let mut items: Vec<(&Match, f64)> = Vec::new();
    let mut unsynced: Vec<&Match> = Vec::new();
    for m in matches {
        let Some(ri) = m.reference.as_deref().and_then(ref_of) else {
            if m.clip_info.duration > 0.0 && (m.clip_info.has_video || m.clip_info.has_audio) {
                unsynced.push(m);
            }
            continue;
        };
        // In camera-to-camera mode the reference is itself a camera clip: show it as one.
        if m.clip == references[ri].path && !m.clip_info.has_video {
            continue;
        }
        // With clock drift the NLE can't stretch the clip, so line it up at its middle
        // (halves the worst error at the ends; the rendered synced files fix drift fully).
        items.push((m, ref_pos[ri] + m.offset + m.drift_ppm * 1e-6 * m.clip_info.duration / 2.0));
    }
    // Recorder files (audio-only references) go on their own tracks.
    let recs: Vec<usize> = (0..references.len()).filter(|&i| !references[i].info.has_video).collect();
    // Clips that couldn't be synced still go in, one after another after the end (so nothing is
    // lost); the editor lines them up by hand.
    if !unsynced.is_empty() {
        let mut t = items.iter().map(|(m, t)| t + m.clip_info.duration)
            .chain(recs.iter().map(|&r| ref_pos[r] + references[r].info.duration))
            .fold(0.0f64, f64::max) + 10.0;
        for m in unsynced {
            items.push((m, t));
            t += m.clip_info.duration + 2.0;
        }
    }
    if items.is_empty() && recs.is_empty() {
        x.push_str("</children></project>\n</xmeml>\n");
        return x;
    }

    // Sequence settings come from the first camera clip.
    let first = items.iter().map(|i| &i.0.clip_info).find(|c| c.has_video).or(items.first().map(|i| &i.0.clip_info)).unwrap_or(&references[0].info);
    let (tb, ntsc) = timebase(first);
    let fps = if first.fps() > 0.0 { first.fps() } else { tb as f64 };
    let t0 = items.iter().map(|i| i.1).chain(recs.iter().map(|&r| ref_pos[r])).fold(f64::INFINITY, f64::min).min(0.0);
    let fr = |t: f64| ((t - t0) * fps).round() as i64;

    // Greedy: each item goes on the first track that is free at its start.
    fn assign(spans: &[(i64, i64)]) -> (Vec<usize>, usize) {
        let mut order: Vec<usize> = (0..spans.len()).collect();
        order.sort_by_key(|&i| spans[i].0);
        let mut ends: Vec<i64> = Vec::new();
        let mut track = vec![0; spans.len()];
        for i in order {
            let t = ends.iter().position(|&e| e <= spans[i].0).unwrap_or_else(|| {
                ends.push(i64::MIN);
                ends.len() - 1
            });
            ends[t] = spans[i].1;
            track[i] = t;
        }
        (track, ends.len())
    }
    let spans: Vec<(i64, i64)> = items.iter().map(|(m, t)| (fr(*t), fr(*t) + (m.clip_info.duration * fps).round() as i64)).collect();
    // One camera per track (camera A on V1, camera B on V2…), like a multicam edit. A camera
    // is told apart by folder, file-name prefix, picture size, frame rate and sound format.
    let cam_key = |m: &Match| {
        let p = Path::new(&m.clip);
        let name = ffmpeg::file_name(p);
        let prefix: String = name.chars().take_while(|c| !c.is_ascii_digit()).collect();
        let dir = p.parent().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default();
        let i = &m.clip_info;
        format!("{dir}|{prefix}|{}x{}|{:.2}|{}|{}", i.width, i.height, i.fps(), i.sample_rate, i.has_video)
    };
    let mut cams: Vec<String> = Vec::new();
    let mut by_start: Vec<usize> = (0..items.len()).collect();
    by_start.sort_by_key(|&k| spans[k].0);
    for &k in &by_start {
        let key = cam_key(items[k].0);
        if !cams.contains(&key) {
            cams.push(key);
        }
    }
    let mut track = vec![0usize; items.len()];
    let mut n_tracks = 0;
    for key in &cams {
        let ks: Vec<usize> = (0..items.len()).filter(|&k| &cam_key(items[k].0) == key).collect();
        let sub: Vec<(i64, i64)> = ks.iter().map(|&k| spans[k]).collect();
        let (t, n) = assign(&sub); // a camera's own clips never overlap, but be safe
        for (j, &k) in ks.iter().enumerate() {
            track[k] = n_tracks + t[j];
        }
        n_tracks += n;
    }
    let rec_spans: Vec<(i64, i64)> = recs.iter().map(|&r| (fr(ref_pos[r]), fr(ref_pos[r]) + (references[r].info.duration * fps).round() as i64)).collect();
    let (rec_track, n_rec_tracks) = assign(&rec_spans);
    let seq_end = spans.iter().chain(rec_spans.iter()).map(|s| s.1).max().unwrap_or(0);

    let fmt = format!(
        "<format><samplecharacteristics>{}<width>{}</width><height>{}</height><pixelaspectratio>square</pixelaspectratio></samplecharacteristics></format>",
        rate_xml(tb, ntsc), first.width.max(16), first.height.max(16)
    );
    let _ = write!(x, "<sequence id=\"sequence-1\"><name>JoshGrade Sync</name><duration>{seq_end}</duration>{}<timecode>{}<string>00:00:00:00</string><frame>0</frame><displayformat>NDF</displayformat></timecode><media>\n<video>{fmt}\n", rate_xml(tb, ntsc), rate_xml(tb, ntsc));

    let name = |p: &str| esc(&ffmpeg::file_name(Path::new(p)));
    let fid = |p: &str| format!("file-{}", sanitize_id(p));
    // Clip index of each item within its track (in time order), for the link elements.
    let mut clip_idx = vec![0usize; items.len()];
    for t in 0..n_tracks {
        let mut on: Vec<usize> = (0..items.len()).filter(|&k| track[k] == t).collect();
        on.sort_by_key(|&k| spans[k].0);
        for (n, k) in on.into_iter().enumerate() {
            clip_idx[k] = n + 1;
        }
    }
    let in_time_order = |t: usize, want_video: bool| {
        let mut on: Vec<usize> = (0..items.len()).filter(|&k| track[k] == t && if want_video { items[k].0.clip_info.has_video } else { items[k].0.clip_info.has_audio }).collect();
        on.sort_by_key(|&k| spans[k].0);
        on
    };
    for t in 0..n_tracks {
        x.push_str("<track>\n");
        for k in in_time_order(t, true) {
            let m = items[k].0;
            let (s, e) = spans[k];
            let frames = e - s;
            let _ = write!(
                x,
                "<clipitem id=\"clipitem-v{k}\"><name>{}</name><enabled>TRUE</enabled><duration>{frames}</duration>{}<start>{s}</start><end>{e}</end><in>0</in><out>{frames}</out>{}",
                name(&m.clip), rate_xml(tb, ntsc), file_xml(&fid(&m.clip), &m.clip_info, tb, ntsc, fps, &mut defined)
            );
            let _ = write!(x, "<link><linkclipref>clipitem-v{k}</linkclipref><mediatype>video</mediatype><trackindex>{}</trackindex><clipindex>{}</clipindex></link>", t + 1, clip_idx[k]);
            if m.clip_info.has_audio {
                let _ = write!(x, "<link><linkclipref>clipitem-a{k}</linkclipref><mediatype>audio</mediatype><trackindex>{}</trackindex><clipindex>{}</clipindex></link>", t + 1, clip_idx[k]);
            }
            x.push_str("</clipitem>\n");
        }
        x.push_str("</track>\n");
    }
    x.push_str("</video>\n<audio>\n");
    // camera sound, one track per picture track
    // Premiere reads a track as mono unless told: say which tracks carry stereo files.
    let track_tag = |stereo: bool| format!("<track premiereTrackType=\"{}\">\n", if stereo { "Stereo" } else { "Mono" });
    for t in 0..n_tracks {
        let on = in_time_order(t, false);
        x.push_str(&track_tag(!on.is_empty() && on.iter().all(|&k| items[k].0.clip_info.audio_channels == 2)));
        for k in on {
            let m = items[k].0;
            let (s, e) = spans[k];
            let frames = e - s;
            let _ = write!(
                x,
                "<clipitem id=\"clipitem-a{k}\"><name>{}</name><enabled>TRUE</enabled><duration>{frames}</duration>{}<start>{s}</start><end>{e}</end><in>0</in><out>{frames}</out>{}<sourcetrack><mediatype>audio</mediatype><trackindex>1</trackindex></sourcetrack></clipitem>\n",
                name(&m.clip), rate_xml(tb, ntsc), file_xml(&fid(&m.clip), &m.clip_info, tb, ntsc, fps, &mut defined)
            );
        }
        x.push_str("</track>\n");
    }
    // recorder files, below the camera sound
    for t in 0..n_rec_tracks {
        let mut on: Vec<usize> = (0..recs.len()).filter(|&k| rec_track[k] == t).collect();
        x.push_str(&track_tag(!on.is_empty() && on.iter().all(|&k| references[recs[k]].info.audio_channels == 2)));
        on.sort_by_key(|&k| rec_spans[k].0);
        for k in on {
            let r = &references[recs[k]];
            let (s, e) = rec_spans[k];
            let frames = e - s;
            let _ = write!(
                x,
                "<clipitem id=\"clipitem-r{k}\"><name>{}</name><enabled>TRUE</enabled><duration>{frames}</duration>{}<start>{s}</start><end>{e}</end><in>0</in><out>{frames}</out>{}<sourcetrack><mediatype>audio</mediatype><trackindex>1</trackindex></sourcetrack></clipitem>\n",
                name(&r.path), rate_xml(tb, ntsc), file_xml(&fid(&r.path), &r.info, tb, ntsc, fps, &mut defined)
            );
        }
        x.push_str("</track>\n");
    }
    x.push_str("</audio>\n</media></sequence>\n</children></project>\n</xmeml>\n");
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
