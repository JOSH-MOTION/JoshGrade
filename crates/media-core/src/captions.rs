//! Offline auto captions with Whisper (whisper.cpp), word timings, caption formatting,
//! SRT / VTT / TXT output, and putting captions into the video (soft track or burned in).

use crate::ffmpeg::{self, Tools};
use crate::Error;
use serde::{Deserialize, Serialize};
use std::fmt::Write as _;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

#[derive(Debug, Clone, Serialize)]
pub struct ModelInfo {
    pub id: &'static str,
    pub label: &'static str,
    pub file: &'static str,
    pub size_mb: u32,
    pub description: &'static str,
}

pub const MODELS: &[ModelInfo] = &[
    ModelInfo { id: "fast", label: "Fast", file: "ggml-base.bin", size_mb: 148, description: "Quick drafts. Good for clear speech." },
    ModelInfo { id: "accurate", label: "Accurate", file: "ggml-small.bin", size_mb: 488, description: "Recommended. Handles accents and background noise much better." },
    ModelInfo { id: "best", label: "Best", file: "ggml-large-v3-turbo-q5_0.bin", size_mb: 574, description: "Most accurate, slowest. For interviews and difficult audio." },
];

pub fn model(id: &str) -> Option<&'static ModelInfo> {
    MODELS.iter().find(|m| m.id == id)
}

pub fn model_path(dir: &Path, id: &str) -> Option<PathBuf> {
    model(id).map(|m| dir.join(m.file))
}

/// Download a caption model once; afterwards everything runs offline.
pub fn download_model(dir: &Path, id: &str, cancel: &AtomicBool, mut progress: impl FnMut(u64, u64)) -> Result<PathBuf, Error> {
    let m = model(id).ok_or_else(|| Error::Other(format!("unknown model {id}")))?;
    std::fs::create_dir_all(dir)?;
    let dest = dir.join(m.file);
    if dest.exists() {
        return Ok(dest);
    }
    let url = format!("https://huggingface.co/ggerganov/whisper.cpp/resolve/main/{}", m.file);
    let resp = ureq::get(&url).call().map_err(|e| Error::Other(format!("Couldn't download the caption engine ({e}). Check your internet connection and try again.")))?;
    let total: u64 = resp.header("Content-Length").and_then(|v| v.parse().ok()).unwrap_or(m.size_mb as u64 * 1_000_000);
    let part = dir.join(format!("{}.part", m.file));
    let mut f = std::fs::File::create(&part)?;
    let mut r = resp.into_reader();
    let mut buf = vec![0u8; 1 << 16];
    let mut got = 0u64;
    loop {
        if cancel.load(Ordering::Relaxed) {
            drop(f);
            let _ = std::fs::remove_file(&part);
            return Err(Error::Cancelled);
        }
        let n = r.read(&mut buf)?;
        if n == 0 {
            break;
        }
        f.write_all(&buf[..n])?;
        got += n as u64;
        progress(got, total);
    }
    f.flush()?;
    drop(f);
    if got < total / 2 {
        let _ = std::fs::remove_file(&part);
        return Err(Error::Other("The download was cut off. Try again.".into()));
    }
    std::fs::rename(&part, &dest)?;
    Ok(dest)
}

pub fn load_model(path: &Path) -> Result<WhisperContext, Error> {
    whisper_rs::install_logging_hooks(); // keep whisper.cpp quiet
    WhisperContext::new_with_params(path, WhisperContextParameters::default())
        .map_err(|e| Error::Other(format!("couldn't load the caption engine: {e}")))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Word {
    pub text: String,
    pub start: f64,
    pub end: f64,
    pub prob: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transcript {
    pub language: String,
    pub duration: f64,
    pub words: Vec<Word>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TranscribeOptions {
    /// Language code ("en", "fr"…) or "auto".
    pub language: String,
    /// Translate the speech into English.
    pub translate: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct TranscribeProgress {
    /// 0..100
    pub percent: f32,
    /// Text recognised so far (latest few words), for a live preview.
    pub latest: String,
}

const SR: usize = 16000;
const CHUNK: usize = SR * 300; // work in ~5-minute pieces (memory + cancel points)

pub fn transcribe(
    tools: &Tools,
    ctx: &WhisperContext,
    path: &Path,
    opts: &TranscribeOptions,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(TranscribeProgress),
) -> Result<Transcript, Error> {
    let info = ffmpeg::probe(tools, path)?;
    if !info.has_audio {
        return Err(Error::Other(format!("{} has no sound to caption", ffmpeg::file_name(path))));
    }
    let duration = info.duration.max(0.1);
    let mut words: Vec<Word> = Vec::new();
    let mut language = opts.language.clone();
    let mut pending: Vec<f32> = Vec::new();
    let mut consumed = 0usize; // samples already transcribed
    let mut failure: Option<Error> = None;

    let run_chunk = |audio: &[f32], offset: usize, words: &mut Vec<Word>, language: &mut String| -> Result<(), Error> {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let mut state = ctx.create_state().map_err(|e| Error::Other(e.to_string()))?;
        let mut p = FullParams::new(SamplingStrategy::Greedy { best_of: 5 });
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(8) as i32;
        p.set_n_threads(threads);
        let lang = if language == "auto" { None } else { Some(language.as_str()) };
        p.set_language(Some(lang.unwrap_or("auto")));
        p.set_translate(opts.translate);
        p.set_token_timestamps(true);
        p.set_print_progress(false);
        p.set_print_realtime(false);
        p.set_print_special(false);
        p.set_print_timestamps(false);
        p.set_suppress_nst(true);
        p.set_no_speech_thold(0.6);
        state.full(p, audio).map_err(|e| Error::Other(format!("transcription failed: {e}")))?;
        if language == "auto" {
            let id = state.full_lang_id_from_state();
            if let Some(code) = whisper_rs::get_lang_str(id) {
                *language = code.to_string();
            }
        }
        let eot = ctx.token_eot();
        let base = offset as f64 / SR as f64;
        let chunk_end = base + audio.len() as f64 / SR as f64;
        for seg in state.as_iter() {
            let seg_t0 = base + seg.start_timestamp() as f64 / 100.0;
            let seg_t1 = base + seg.end_timestamp() as f64 / 100.0;
            let mut cur: Option<(Vec<u8>, f64, f64, f32, u32)> = None;
            let flush = |cur: &mut Option<(Vec<u8>, f64, f64, f32, u32)>, words: &mut Vec<Word>| {
                if let Some((bytes, s, e, p, n)) = cur.take() {
                    let text = String::from_utf8_lossy(&bytes).trim().to_string();
                    if !text.is_empty() {
                        words.push(Word { text, start: s, end: e.max(s + 0.05), prob: p / n.max(1) as f32 });
                    }
                }
            };
            for t in 0..seg.n_tokens() {
                let Some(tok) = seg.get_token(t) else { continue };
                if tok.token_id() >= eot {
                    continue; // timestamps and special tokens
                }
                let bytes = tok.to_bytes().unwrap_or(&[]).to_vec();
                if bytes.is_empty() {
                    continue;
                }
                let d = tok.token_data();
                let (t0, t1) = ((base + d.t0 as f64 / 100.0).clamp(seg_t0, seg_t1), (base + d.t1 as f64 / 100.0).clamp(seg_t0, seg_t1));
                let starts_word = bytes[0] == b' ';
                let punct_only = bytes.iter().all(|b| b.is_ascii_punctuation());
                match &mut cur {
                    Some((b, _, e, p, n)) if !starts_word || punct_only => {
                        b.extend_from_slice(&bytes);
                        *e = e.max(t1);
                        *p += tok.token_probability();
                        *n += 1;
                    }
                    _ => {
                        flush(&mut cur, words);
                        cur = Some((bytes, t0, t1, tok.token_probability(), 1));
                    }
                }
            }
            flush(&mut cur, words);
        }
        // keep word times inside the chunk and in order
        for w in words.iter_mut() {
            w.end = w.end.min(chunk_end);
        }
        Ok(())
    };

    let prog = |done: usize, words: &Vec<Word>, progress: &mut dyn FnMut(TranscribeProgress)| {
        let latest = words.iter().rev().take(12).collect::<Vec<_>>().into_iter().rev().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ");
        progress(TranscribeProgress { percent: ((done as f64 / SR as f64) / duration * 100.0).min(99.0) as f32, latest });
    };
    progress(TranscribeProgress { percent: 0.0, latest: String::new() });

    ffmpeg::stream_audio(tools, path, SR as u32, None, None, |s| {
        if failure.is_some() {
            return;
        }
        pending.extend_from_slice(s);
        if pending.len() >= CHUNK + SR * 30 {
            // cut at the quietest moment in the last 30 s of the chunk so words aren't split
            let cut = quietest_point(&pending, CHUNK, CHUNK + SR * 30);
            let rest = pending.split_off(cut);
            if let Err(e) = run_chunk(&pending, consumed, &mut words, &mut language) {
                failure = Some(e);
                return;
            }
            consumed += pending.len();
            pending = rest;
            prog(consumed, &words, progress);
        }
    })?;
    if let Some(e) = failure {
        return Err(e);
    }
    if pending.len() > SR / 4 {
        // whisper wants at least 1 s
        if pending.len() < SR + SR / 10 {
            pending.resize(SR + SR / 10, 0.0);
        }
        run_chunk(&pending, consumed, &mut words, &mut language)?;
    }
    words.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap());
    // Remove whisper's occasional repeated-hallucination loops (same phrase many times in a row).
    dedupe_loops(&mut words);
    progress(TranscribeProgress { percent: 100.0, latest: String::new() });
    Ok(Transcript { language, duration, words })
}

fn quietest_point(x: &[f32], from: usize, to: usize) -> usize {
    let w = SR / 5;
    let (mut best, mut be) = (from, f32::MAX);
    let mut i = from;
    while i + w <= to.min(x.len()) {
        let e: f32 = x[i..i + w].iter().map(|v| v * v).sum();
        if e < be {
            be = e;
            best = i + w / 2;
        }
        i += w / 2;
    }
    best
}

fn dedupe_loops(words: &mut Vec<Word>) {
    let norm = |s: &str| s.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect::<String>();
    let mut out: Vec<Word> = Vec::with_capacity(words.len());
    for w in words.drain(..) {
        let n = norm(&w.text);
        let repeats = out.iter().rev().take(3).filter(|p| norm(&p.text) == n && !n.is_empty()).count();
        if repeats < 3 {
            out.push(w);
        }
    }
    *words = out;
}

// ------------------------------------------------------------------ caption formatting

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cue {
    pub start: f64,
    pub end: f64,
    pub text: String,
    /// The recognised words in this caption (for word-by-word template effects).
    #[serde(default)]
    pub words: Vec<Word>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CueStyle {
    /// Max characters per line (42 is the broadcast standard; 20–30 for social).
    pub max_chars: usize,
    /// 1 or 2 lines.
    pub max_lines: usize,
    /// Max words per caption (0 = no limit). 1–4 gives the punchy social style.
    pub max_words: usize,
    pub uppercase: bool,
}

/// Lines needed to set these words at `max` characters per line (greedy, like a subtitle editor).
fn lines_needed(words: &[&Word], max: usize) -> usize {
    let mut lines = 1;
    let mut cur = 0usize;
    for w in words {
        let l = w.text.chars().count();
        if cur == 0 {
            cur = l;
        } else if cur + 1 + l <= max {
            cur += 1 + l;
        } else {
            lines += 1;
            cur = l;
        }
    }
    lines
}

/// Abbreviations whose full stop doesn't end a sentence.
fn is_abbrev(w: &str) -> bool {
    matches!(w.to_lowercase().trim_end_matches('.'), "mr" | "mrs" | "ms" | "dr" | "st" | "prof" | "sr" | "jr" | "vs" | "etc" | "e.g" | "i.e" | "no")
}

pub fn build_cues(words: &[Word], style: &CueStyle) -> Vec<Cue> {
    let max_total = style.max_chars * style.max_lines.max(1);
    let mut cues: Vec<Cue> = Vec::new();
    let mut cur: Vec<&Word> = Vec::new();
    let len = |ws: &[&Word]| ws.iter().map(|w| w.text.chars().count()).sum::<usize>() + ws.len().saturating_sub(1);
    let push = |cur: &mut Vec<&Word>, cues: &mut Vec<Cue>| {
        if cur.is_empty() {
            return;
        }
        let text = wrap(&cur.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" "), style.max_chars, style.max_lines);
        let text = if style.uppercase { text.to_uppercase() } else { text };
        let words = cur.iter().map(|w| (*w).clone()).collect();
        cues.push(Cue { start: cur[0].start, end: cur[cur.len() - 1].end, text, words });
        cur.clear();
    };
    for (i, w) in words.iter().enumerate() {
        if let Some(last) = cur.last() {
            let gap = w.start - last.end;
            let sentence_end = last.text.ends_with(['.', '?', '!']) && !is_abbrev(&last.text);
            let clause_end = last.text.ends_with([',', ';', ':']);
            let mut with: Vec<&Word> = cur.clone();
            with.push(w);
            let too_long = lines_needed(&with, style.max_chars) > style.max_lines.max(1);
            let too_slow = w.end - cur[0].start > 6.0;
            let too_many = style.max_words > 0 && cur.len() >= style.max_words;
            // Start a new caption at a sentence end, and at a comma once the caption is well filled.
            let next_capital = w.text.chars().next().map(|c| c.is_uppercase()).unwrap_or(false);
            let natural = (sentence_end && (len(&cur) >= 12 || gap > 0.25 || next_capital)) || (clause_end && len(&cur) as f64 > max_total as f64 * 0.6);
            if gap > 0.7 || too_long || too_slow || too_many || natural {
                push(&mut cur, &mut cues);
            }
        }
        cur.push(w);
        let _ = i;
    }
    push(&mut cur, &mut cues);
    // Timing polish: readable minimum, a short hang after speech, never overlapping.
    for i in 0..cues.len() {
        let next = cues.get(i + 1).map(|c| c.start).unwrap_or(f64::MAX);
        let c = &mut cues[i];
        c.end = (c.end + 0.25).max(c.start + 0.8).min(next - 0.04).max(c.start + 0.3);
    }
    cues
}

/// Split into up to `lines` balanced lines of at most `max` characters (prefers breaking after punctuation).
fn wrap(text: &str, max: usize, lines: usize) -> String {
    if lines < 2 || text.chars().count() <= max {
        return text.to_string();
    }
    let words: Vec<&str> = text.split(' ').collect();
    let total = text.chars().count();
    let mut best = (f64::MAX, 1usize);
    let fits_any = (1..words.len()).any(|k| {
        let a = words[..k].join(" ").chars().count();
        a <= max && total - a - 1 <= max
    });
    for k in 1..words.len() {
        let a = words[..k].join(" ").chars().count();
        let b = total - a - 1;
        if fits_any && (a > max || b > max) {
            continue;
        }
        let mut cost = (a as f64 - b as f64).abs();
        if words[k - 1].ends_with([',', '.', '?', '!', ';', ':']) {
            cost -= 8.0;
        }
        if a < b {
            cost -= 2.0; // pyramid shape reads better
        }
        if cost < best.0 {
            best = (cost, k);
        }
    }
    format!("{}\n{}", words[..best.1].join(" "), words[best.1..].join(" "))
}

fn ts(t: f64, sep: char) -> String {
    let ms = (t.max(0.0) * 1000.0).round() as u64;
    format!("{:02}:{:02}:{:02}{sep}{:03}", ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000)
}

pub fn to_srt(cues: &[Cue]) -> String {
    let mut s = String::new();
    for (i, c) in cues.iter().enumerate() {
        let _ = write!(s, "{}\n{} --> {}\n{}\n\n", i + 1, ts(c.start, ','), ts(c.end, ','), c.text.trim());
    }
    s
}

pub fn to_vtt(cues: &[Cue]) -> String {
    let mut s = String::from("WEBVTT\n\n");
    for c in cues {
        let _ = write!(s, "{} --> {}\n{}\n\n", ts(c.start, '.'), ts(c.end, '.'), c.text.trim());
    }
    s
}

/// Plain transcript with paragraphs at long pauses.
pub fn to_txt(words: &[Word]) -> String {
    let mut s = String::new();
    let mut last_end = 0.0;
    for w in words {
        if !s.is_empty() {
            s.push(if w.start - last_end > 1.5 { '\n' } else { ' ' });
            if w.start - last_end > 1.5 {
                s.push('\n');
            }
        }
        s.push_str(&w.text);
        last_end = w.end;
    }
    s.push('\n');
    s
}

// ------------------------------------------------------------------ into the video

#[derive(Debug, Clone, Deserialize)]
pub struct BurnStyle {
    /// "classic" (white with outline), "box" (white on a dark box) or "bold" (big, social style).
    pub preset: String,
    /// Text size as % of video height (default 5).
    pub size: f32,
    /// "bottom", "lower" (lower third, clear of TikTok/Reels buttons), "middle" or "top".
    pub position: String,
}

/// Fonts folder on macOS with Arial/Helvetica (passed to libass so it never depends on fontconfig).
fn fonts_dir() -> Option<&'static str> {
    ["/System/Library/Fonts/Supplemental", "/Library/Fonts", "/usr/share/fonts/truetype/dejavu", "C:/Windows/Fonts"]
        .into_iter()
        .find(|d| Path::new(d).is_dir())
}

fn ass_style(style: &BurnStyle, video_h: u32) -> String {
    // libass scales SRT styles to a 288-line canvas.
    let size = (style.size.clamp(2.0, 12.0) / 100.0 * 288.0).round() as u32;
    let align = match style.position.as_str() {
        "top" => 8,
        "middle" => 5,
        _ => 2,
    };
    let margin = match style.position.as_str() {
        "middle" => 0,
        "lower" => 72,
        _ => 18,
    };
    let font = if Path::new("/System/Library/Fonts/Supplemental/Arial Bold.ttf").exists() || cfg!(any(target_os = "macos", windows)) { "Arial" } else { "DejaVu Sans" };
    let _ = video_h;
    match style.preset.as_str() {
        "box" => format!("FontName={font},FontSize={size},Bold=1,PrimaryColour=&H00FFFFFF,BackColour=&H99000000,OutlineColour=&H99000000,BorderStyle=3,Outline=4,Shadow=0,Alignment={align},MarginV={margin}"),
        "bold" => format!("FontName={font},FontSize={},Bold=1,PrimaryColour=&H0000FFFF,OutlineColour=&H00000000,BorderStyle=1,Outline=3,Shadow=1,Alignment={align},MarginV={margin}", size + size / 3),
        _ => format!("FontName={font},FontSize={size},Bold=1,PrimaryColour=&H00FFFFFF,OutlineColour=&H00000000,BorderStyle=1,Outline=2,Shadow=1,Alignment={align},MarginV={margin}"),
    }
}

fn unique(dir: &Path, stem: &str, suffix: &str, ext: &str) -> PathBuf {
    let mut p = dir.join(format!("{stem}{suffix}.{ext}"));
    let mut n = 2;
    while p.exists() {
        p = dir.join(format!("{stem}{suffix}_{n}.{ext}"));
        n += 1;
    }
    p
}

/// Add captions as a switchable subtitle track (no re-encoding, instant).
pub fn add_caption_track(tools: &Tools, video: &Path, cues: &[Cue], language: &str, out_dir: &Path) -> Result<PathBuf, Error> {
    let tmp = std::env::temp_dir().join(format!("joshgrade-{}.srt", std::process::id()));
    std::fs::write(&tmp, to_srt(cues))?;
    let stem = video.file_stem().and_then(|s| s.to_str()).unwrap_or("video");
    let ext = match video.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("mp4") | Some("m4v") => "mp4",
        _ => "mov",
    };
    std::fs::create_dir_all(out_dir)?;
    let out = unique(out_dir, stem, "_captioned", ext);
    let mut cmd = tools.ffmpeg_cmd();
    cmd.arg("-i").arg(video).arg("-i").arg(&tmp);
    cmd.args(["-map", "0:v", "-map", "0:a?", "-map", "1:0", "-c", "copy", "-c:s", "mov_text"]);
    cmd.args(["-metadata:s:s:0", &format!("language={}", iso639_2(language))]);
    cmd.arg(&out);
    let r = ffmpeg::run(cmd, "Adding captions");
    let _ = std::fs::remove_file(&tmp);
    r.map(|_| out)
}

/// Burn captions into the picture (re-encodes video with high-quality H.264).
pub fn burn_captions(tools: &Tools, video: &Path, cues: &[Cue], style: &BurnStyle, out_dir: &Path) -> Result<PathBuf, Error> {
    let info = ffmpeg::probe(tools, video)?;
    // Work from a temp folder with a plain file name: ffmpeg filter paths are fussy about quoting.
    let work = std::env::temp_dir().join(format!("joshgrade-burn-{}", std::process::id()));
    std::fs::create_dir_all(&work)?;
    std::fs::write(work.join("captions.srt"), to_srt(cues))?;
    let mut filter = String::from("subtitles=captions.srt");
    if let Some(fd) = fonts_dir() {
        let _ = write!(filter, ":fontsdir='{}'", fd.replace('\'', "\\'"));
    }
    let _ = write!(filter, ":force_style='{}'", ass_style(style, info.height));
    let stem = video.file_stem().and_then(|s| s.to_str()).unwrap_or("video");
    std::fs::create_dir_all(out_dir)?;
    let out = unique(out_dir, stem, "_captions", "mp4");
    let out_abs = if out.is_absolute() { out.clone() } else { std::env::current_dir()?.join(&out) };
    let video_abs = if video.is_absolute() { video.to_path_buf() } else { std::env::current_dir()?.join(video) };
    let mut cmd = tools.ffmpeg_cmd();
    cmd.current_dir(&work);
    cmd.arg("-i").arg(&video_abs);
    cmd.args(["-vf", &filter, "-map", "0:v:0", "-map", "0:a?", "-c:v", "libx264", "-preset", "medium", "-crf", "18", "-pix_fmt", "yuv420p", "-c:a", "aac", "-b:a", "256k", "-movflags", "+faststart"]);
    cmd.arg(&out_abs);
    let r = ffmpeg::run(cmd, "Burning in captions");
    let _ = std::fs::remove_dir_all(&work);
    r.map(|_| out_abs)
}

#[derive(Debug, Clone, Deserialize)]
pub struct TemplateStyle {
    pub template: String,
    /// Size multiplier (1.0 = template default).
    pub scale: f32,
    /// "" = template default, or "bottom" / "lower" / "middle" / "top".
    pub position: String,
}

/// Burn captions into the picture using an animated template and the bundled fonts.
pub fn burn_template(tools: &Tools, video: &Path, cues: &[Cue], style: &TemplateStyle, fonts: Option<&Path>, out_dir: &Path) -> Result<PathBuf, Error> {
    let tpl = crate::templates::template(&style.template).ok_or_else(|| Error::Other(format!("unknown caption template {}", style.template)))?;
    let info = ffmpeg::probe(tools, video)?;
    // Rotated phone videos report their stored size; the picture we burn onto is the displayed size.
    let (w, h) = (info.width.max(16), info.height.max(16));
    let ass = crate::templates::to_ass(cues, tpl, &crate::templates::RenderOpts { width: w, height: h, scale: style.scale, position: style.position.clone() });
    let work = std::env::temp_dir().join(format!("joshgrade-burn-{}", std::process::id()));
    std::fs::create_dir_all(&work)?;
    std::fs::write(work.join("captions.ass"), ass)?;
    let mut filter = String::from("ass=captions.ass");
    let fd = fonts.map(|p| p.to_string_lossy().into_owned()).or_else(|| fonts_dir().map(String::from));
    if let Some(fd) = fd {
        let _ = write!(filter, ":fontsdir='{}'", fd.replace('\\', "/"));
    }
    let stem = video.file_stem().and_then(|s| s.to_str()).unwrap_or("video");
    std::fs::create_dir_all(out_dir)?;
    let out = unique(out_dir, stem, &format!("_{}", tpl.id), "mp4");
    let out_abs = if out.is_absolute() { out.clone() } else { std::env::current_dir()?.join(&out) };
    let video_abs = if video.is_absolute() { video.to_path_buf() } else { std::env::current_dir()?.join(video) };
    let mut cmd = tools.ffmpeg_cmd();
    cmd.current_dir(&work);
    cmd.arg("-i").arg(&video_abs);
    cmd.args(["-vf", &filter, "-map", "0:v:0", "-map", "0:a?", "-c:v", "libx264", "-preset", "medium", "-crf", "18", "-pix_fmt", "yuv420p", "-c:a", "aac", "-b:a", "256k", "-movflags", "+faststart"]);
    cmd.arg(&out_abs);
    let r = ffmpeg::run(cmd, "Burning in captions");
    let _ = std::fs::remove_dir_all(&work);
    r.map(|_| out_abs)
}

fn iso639_2(code: &str) -> &'static str {
    match code {
        "en" => "eng", "fr" => "fra", "es" => "spa", "de" => "deu", "pt" => "por", "it" => "ita",
        "yo" => "yor", "ha" => "hau", "sw" => "swa", "ar" => "ara", "zh" => "zho", "ja" => "jpn",
        "ko" => "kor", "ru" => "rus", "nl" => "nld", "hi" => "hin", _ => "und",
    }
}
