//! Thin wrapper around the bundled ffmpeg / ffprobe programs.

use crate::Error;
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Debug, Clone)]
pub struct Tools {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
}

impl Tools {
    /// Find ffmpeg/ffprobe: next to this program (bundled app), then $JOSHGRADE_FFMPEG_DIR,
    /// then common install locations and $PATH.
    pub fn locate() -> Result<Tools, Error> {
        let mut dirs: Vec<PathBuf> = Vec::new();
        if let Ok(exe) = std::env::current_exe() {
            if let Some(d) = exe.parent() {
                dirs.push(d.to_path_buf());
            }
        }
        if let Ok(d) = std::env::var("JOSHGRADE_FFMPEG_DIR") {
            dirs.push(PathBuf::from(d));
        }
        for d in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"] {
            dirs.push(PathBuf::from(d));
        }
        if let Some(p) = std::env::var_os("PATH") {
            dirs.extend(std::env::split_paths(&p));
        }
        let exe = if cfg!(windows) { ".exe" } else { "" };
        let find = |name: &str| dirs.iter().map(|d| d.join(format!("{name}{exe}"))).find(|p| p.is_file());
        match (find("ffmpeg"), find("ffprobe")) {
            (Some(ffmpeg), Some(ffprobe)) => Ok(Tools { ffmpeg, ffprobe }),
            _ => Err(Error::Other("ffmpeg isn't installed with JoshGrade. Re-run the installer.".into())),
        }
    }

    pub fn ffmpeg_cmd(&self) -> Command {
        let mut c = Command::new(&self.ffmpeg);
        c.args(["-hide_banner", "-nostdin", "-loglevel", "error"]);
        c
    }

    /// Whether this ffmpeg can burn subtitles into video (needs libass).
    pub fn can_burn_subtitles(&self) -> bool {
        Command::new(&self.ffmpeg)
            .args(["-hide_banner", "-filters"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains(" subtitles "))
            .unwrap_or(false)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct MediaInfo {
    pub path: String,
    pub duration: f64,
    pub has_video: bool,
    pub has_audio: bool,
    pub width: u32,
    pub height: u32,
    /// Frame rate as a fraction (e.g. 30000/1001).
    pub fps_num: u32,
    pub fps_den: u32,
    pub audio_channels: u32,
    pub sample_rate: u32,
    /// Embedded start timecode, if the camera wrote one (e.g. "14:03:22:11").
    pub timecode: Option<String>,
    /// When recording started, as seconds since 1970 (from the recorder's BWF time
    /// reference, the timecode, or the creation time) — used to put several recorder
    /// files on one timeline.
    pub start_epoch: Option<f64>,
}

impl MediaInfo {
    pub fn fps(&self) -> f64 {
        if self.fps_den == 0 { 0.0 } else { self.fps_num as f64 / self.fps_den as f64 }
    }
}

pub fn probe(tools: &Tools, path: &Path) -> Result<MediaInfo, Error> {
    let out = Command::new(&tools.ffprobe)
        .args(["-v", "error", "-print_format", "json", "-show_format", "-show_streams"])
        .arg(path)
        .output()?;
    if !out.status.success() {
        return Err(Error::Other(format!("couldn't read {}: {}", file_name(path), String::from_utf8_lossy(&out.stderr).trim())));
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).map_err(|e| Error::Other(e.to_string()))?;
    let mut info = MediaInfo { path: path.to_string_lossy().into_owned(), ..Default::default() };
    info.duration = v["format"]["duration"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0.0);
    let mut tc = v["format"]["tags"]["timecode"].as_str().map(String::from);
    for s in v["streams"].as_array().cloned().unwrap_or_default() {
        match s["codec_type"].as_str() {
            Some("video") if !info.has_video && s["disposition"]["attached_pic"].as_i64() != Some(1) => {
                info.has_video = true;
                info.width = s["width"].as_u64().unwrap_or(0) as u32;
                info.height = s["height"].as_u64().unwrap_or(0) as u32;
                let rate = s["avg_frame_rate"].as_str().filter(|r| *r != "0/0").or(s["r_frame_rate"].as_str()).unwrap_or("0/0");
                let mut it = rate.split('/');
                info.fps_num = it.next().and_then(|x| x.parse().ok()).unwrap_or(0);
                info.fps_den = it.next().and_then(|x| x.parse().ok()).unwrap_or(1);
                if tc.is_none() {
                    tc = s["tags"]["timecode"].as_str().map(String::from);
                }
            }
            Some("audio") if !info.has_audio => {
                info.has_audio = true;
                info.audio_channels = s["channels"].as_u64().unwrap_or(0) as u32;
                info.sample_rate = s["sample_rate"].as_str().and_then(|x| x.parse().ok()).unwrap_or(0);
            }
            Some("data") if tc.is_none() => tc = s["tags"]["timecode"].as_str().map(String::from),
            _ => {}
        }
    }
    info.timecode = tc;
    info.start_epoch = start_time(&v["format"]["tags"], &info);
    Ok(info)
}

/// Days since 1970-01-01 for a civil date (proleptic Gregorian).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn parse_date(s: &str) -> Option<i64> {
    let p: Vec<i64> = s.get(..10)?.split(['-', ':', '/']).filter_map(|x| x.parse().ok()).collect();
    (p.len() == 3 && p[0] > 1970).then(|| days_from_civil(p[0], p[1], p[2]))
}

fn parse_hms(s: &str) -> Option<f64> {
    let p: Vec<f64> = s.split([':', ';', '-']).filter_map(|x| x.parse().ok()).collect();
    (p.len() >= 3).then(|| p[0] * 3600.0 + p[1] * 60.0 + p[2])
}

fn start_time(tags: &serde_json::Value, info: &MediaInfo) -> Option<f64> {
    let get = |k: &str| tags[k].as_str().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let created = get("creation_time");
    // A date: BWF "date"/"origination_date", or the date part of an ISO creation time.
    let day = get("date").or_else(|| get("origination_date")).and_then(|d| parse_date(&d)).or_else(|| created.as_deref().and_then(parse_date));
    // 1. Broadcast WAV time reference (samples since midnight) — exact, written by Zoom/Tascam/Sound Devices.
    if let (Some(tr), Some(d)) = (get("time_reference").and_then(|t| t.parse::<f64>().ok()), day) {
        if info.sample_rate > 0 && tr > 0.0 {
            return Some(d as f64 * 86400.0 + tr / info.sample_rate as f64);
        }
    }
    // 2. Timecode (time of day on most cameras/recorders).
    if let (Some(tc), Some(d)) = (info.timecode.as_deref(), day) {
        let p: Vec<f64> = tc.split([':', ';']).filter_map(|x| x.parse().ok()).collect();
        if p.len() == 4 && (p[0] + p[1] + p[2]) > 0.0 {
            let fps = if info.fps() > 0.0 { info.fps().round() } else { 25.0 };
            return Some(d as f64 * 86400.0 + p[0] * 3600.0 + p[1] * 60.0 + p[2] + p[3] / fps);
        }
    }
    // 3. Creation time: either "HH:MM:SS" with a separate date (BWF) or full ISO "2026-11-08T10:22:31Z".
    let c = created?;
    if c.len() >= 19 && c.as_bytes()[10] == b'T' {
        return Some(parse_date(&c)? as f64 * 86400.0 + parse_hms(&c[11..])?);
    }
    Some(day? as f64 * 86400.0 + parse_hms(&c)?)
}

/// Stream a file's audio as mono f32 at `rate` Hz, handing it to `sink` in chunks.
/// `start`/`dur` (seconds) limit the range. Returns the number of samples produced.
pub fn stream_audio(
    tools: &Tools,
    path: &Path,
    rate: u32,
    start: Option<f64>,
    dur: Option<f64>,
    mut sink: impl FnMut(&[f32]),
) -> Result<usize, Error> {
    let mut cmd = tools.ffmpeg_cmd();
    if let Some(s) = start {
        cmd.args(["-ss", &format!("{:.6}", s.max(0.0))]);
    }
    cmd.arg("-i").arg(path);
    if let Some(d) = dur {
        cmd.args(["-t", &format!("{:.6}", d)]);
    }
    cmd.args(["-vn", "-sn", "-dn", "-ac", "1", "-ar", &rate.to_string(), "-f", "f32le", "-"]);
    let mut child = cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    let mut stdout = child.stdout.take().unwrap();
    let mut buf = vec![0u8; 1 << 16];
    let mut carry: Vec<u8> = Vec::new();
    let mut samples = Vec::with_capacity(1 << 14);
    let mut total = 0usize;
    loop {
        let n = stdout.read(&mut buf)?;
        if n == 0 {
            break;
        }
        carry.extend_from_slice(&buf[..n]);
        let whole = carry.len() / 4 * 4;
        samples.clear();
        samples.extend(carry[..whole].chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])));
        carry.drain(..whole);
        total += samples.len();
        sink(&samples);
    }
    let mut err = String::new();
    if let Some(mut e) = child.stderr.take() {
        let _ = e.read_to_string(&mut err);
    }
    let status = child.wait()?;
    if !status.success() && total == 0 {
        return Err(Error::Other(format!("couldn't read audio from {}: {}", file_name(path), err.trim())));
    }
    Ok(total)
}

/// Whole range as one buffer (only for short ranges).
pub fn read_audio(tools: &Tools, path: &Path, rate: u32, start: Option<f64>, dur: Option<f64>) -> Result<Vec<f32>, Error> {
    let mut v = Vec::new();
    stream_audio(tools, path, rate, start, dur, |s| v.extend_from_slice(s))?;
    Ok(v)
}

/// Run an ffmpeg command to completion, turning failure into a readable error.
pub fn run(mut cmd: Command, what: &str) -> Result<(), Error> {
    let out = cmd.stdout(Stdio::null()).stderr(Stdio::piped()).output()?;
    if out.status.success() {
        Ok(())
    } else {
        let msg = String::from_utf8_lossy(&out.stderr);
        let last = msg.lines().filter(|l| !l.trim().is_empty()).last().unwrap_or("unknown error");
        Err(Error::Other(format!("{what} failed: {last}")))
    }
}

pub fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}
