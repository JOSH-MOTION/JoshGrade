//! Caption templates: animated, styled captions (Hormozi, karaoke, neon, typewriter…),
//! written as an ASS subtitle script and burned in with libass through ffmpeg.
//!
//! Every effect is built from word timings, so the active word lights up exactly when it's said.

use crate::captions::{Cue, Word};
use serde::{Deserialize, Serialize};
use std::fmt::Write as _;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Whole caption at once.
    Static,
    /// Active word changes colour.
    Highlight,
    /// Active word changes colour and pops bigger.
    HighlightScale,
    /// Active word pops, cycling through highlight colours (MrBeast style).
    HighlightCycle,
    /// Line stays up, words fill with colour as they're sung.
    Karaoke,
    /// Words appear one by one as they're spoken.
    Build,
    /// One word on screen at a time.
    Single,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Anim {
    None,
    Fade,
    SlowFade,
    Pop,
    Bounce,
    SlideUp,
    Neon,
    Glitch,
}

#[derive(Debug, Clone, Serialize)]
pub struct Template {
    pub id: &'static str,
    pub name: &'static str,
    /// "Clean", "Social", "Music" or "Stylish".
    pub category: &'static str,
    pub description: &'static str,
    /// Font family name as stored in the bundled font file.
    pub font: &'static str,
    pub bold: bool,
    pub italic: bool,
    pub uppercase: bool,
    /// Text size, % of video height.
    pub size: f32,
    /// Colours as "#RRGGBB".
    pub color: &'static str,
    pub highlight: &'static str,
    /// Extra colours for HighlightCycle.
    pub cycle: &'static [&'static str],
    pub mode: Mode,
    pub anim: Anim,
    pub outline: &'static str,
    /// Outline thickness as a fraction of the font size.
    pub outline_w: f32,
    /// Drop shadow distance as a fraction of the font size (0 = none).
    pub shadow: f32,
    /// Shadow colour and opacity (0..255).
    pub shadow_color: &'static str,
    pub shadow_alpha: u8,
    /// Background box colour and opacity (0..255); None = no box.
    pub box_color: Option<&'static str>,
    pub box_alpha: u8,
    /// Neon glow colour.
    pub glow: &'static str,
    /// Letter spacing, fraction of font size.
    pub spacing: f32,
    /// Tilt in degrees (handwritten look).
    pub tilt: f32,
    /// Default position: "bottom", "lower", "middle", "top".
    pub position: &'static str,
    /// Average character width as a fraction of the font size (measured from the font).
    pub char_w: f32,
    /// CSS em size relative to the ASS font size (ASS sizes by full font height), for previews.
    pub em: f32,
    /// Caption layout.
    pub max_words: usize,
    pub max_lines: usize,
    pub max_chars: usize,
}

const BASE: Template = Template {
    id: "", name: "", category: "Clean", description: "",
    font: "Poppins", bold: true, italic: false, uppercase: false, size: 5.0,
    color: "#FFFFFF", highlight: "#FFE600", cycle: &[], mode: Mode::Static, anim: Anim::Fade,
    outline: "#000000", outline_w: 0.09, shadow: 0.03, shadow_color: "#000000", shadow_alpha: 160,
    box_color: None, box_alpha: 0, glow: "#FF2BD6", spacing: 0.0, tilt: 0.0, position: "bottom",
    char_w: 0.30, em: 0.568, max_words: 0, max_lines: 2, max_chars: 42,
};

pub const TEMPLATES: &[Template] = &[
    // ---------------- Clean
    Template { id: "classic", name: "Classic", category: "Clean",
        description: "Bold white with a black outline. Readable on anything: interviews, documentaries, YouTube.",
        ..BASE },
    Template { id: "box", name: "Box", category: "Clean",
        description: "White text on a dark translucent box, like TV news. Readable over busy footage.",
        font: "Poppins SemiBold", bold: false, size: 4.5, outline_w: 0.0, shadow: 0.0,
        box_color: Some("#000000"), box_alpha: 160, max_chars: 38, ..BASE },
    Template { id: "minimal", name: "Minimal", category: "Clean",
        description: "Clean, soft shadow, the spoken word gently highlighted. Education and explainers (Ali Abdaal style).",
        font: "Poppins SemiBold", bold: false, size: 4.6, outline_w: 0.0, shadow: 0.06, shadow_alpha: 120,
        highlight: "#FFD84D", mode: Mode::Highlight, max_chars: 32, ..BASE },
    Template { id: "typewriter", name: "Typewriter", category: "Clean",
        description: "Words type on as they're spoken, in a monospace font on a dark box. Storytelling and narration.",
        font: "Space Mono", size: 4.0, outline_w: 0.0, shadow: 0.0, mode: Mode::Build, anim: Anim::None,
        box_color: Some("#111111"), box_alpha: 200, max_chars: 30, char_w: 0.413, em: 0.675, ..BASE },
    // ---------------- Social
    Template { id: "hormozi", name: "Hormozi", category: "Social",
        description: "Heavy capitals, 3 words at a time, the spoken word pops in yellow. Podcasts, business, talking heads.",
        font: "Montserrat Black", bold: false, uppercase: true, size: 8.5, mode: Mode::HighlightScale, anim: Anim::Pop,
        outline_w: 0.13, shadow: 0.05, shadow_alpha: 200, position: "lower", max_words: 3, max_lines: 2, max_chars: 16, char_w: 0.427, em: 0.64, ..BASE },
    Template { id: "beast", name: "Beast", category: "Social",
        description: "Comic-style capitals that bounce in, each word flashing a different colour. High-energy YouTube & TikTok.",
        font: "Bangers", bold: false, uppercase: true, size: 11.0, mode: Mode::HighlightCycle, anim: Anim::Bounce,
        highlight: "#3DFF6B", cycle: &["#3DFF6B", "#FFE600", "#FF4D4D"], outline_w: 0.14, shadow: 0.06, shadow_alpha: 220,
        position: "lower", max_words: 2, max_lines: 1, max_chars: 14, spacing: 0.03, char_w: 0.212, em: 0.57, ..BASE },
    Template { id: "word-pop", name: "Word Pop", category: "Social",
        description: "One big word at a time, popping on the beat of the speech. Motivational clips and hooks.",
        font: "Anton", bold: false, uppercase: true, size: 14.0, mode: Mode::Single, anim: Anim::Pop,
        outline_w: 0.08, shadow: 0.05, shadow_alpha: 200, position: "middle", max_words: 4, max_lines: 1, max_chars: 24, char_w: 0.242, em: 0.576, ..BASE },
    Template { id: "yellow-bold", name: "Yellow Bold", category: "Social",
        description: "Big yellow capitals with a thick black outline, 3 words at a time. Reels, TikTok, Shorts.",
        font: "Montserrat Black", bold: false, uppercase: true, size: 8.8, color: "#FFE100", anim: Anim::Pop,
        outline_w: 0.13, shadow: 0.05, shadow_alpha: 200, position: "lower", max_words: 3, max_lines: 1, max_chars: 18, char_w: 0.427, em: 0.64, ..BASE },
    Template { id: "bubble", name: "Bubble", category: "Social",
        description: "Black text on a white sticker box that pops in. Clean CapCut look for lifestyle and vlogs.",
        font: "Poppins", size: 6.0, color: "#111111", outline_w: 0.0, shadow: 0.0, anim: Anim::Pop,
        box_color: Some("#FFFFFF"), box_alpha: 255, position: "lower", max_words: 5, max_chars: 24, ..BASE },
    // ---------------- Music
    Template { id: "karaoke", name: "Karaoke", category: "Music",
        description: "The whole line shows, and each word fills with gold as it's sung. Music videos and lyric videos.",
        font: "Montserrat ExtraBold", bold: false, size: 5.5, color: "#FFFFFF", highlight: "#FFC93C",
        mode: Mode::Karaoke, outline_w: 0.08, shadow: 0.04, max_chars: 30, char_w: 0.355, em: 0.64, ..BASE },
    Template { id: "neon", name: "Neon", category: "Music",
        description: "Glowing pink neon that pulses gently. Nightlife, DJ sets, Afrobeats, after-dark visuals.",
        font: "Poppins SemiBold", bold: false, size: 6.0, color: "#FFF2FB", anim: Anim::Neon, glow: "#FF2BD6",
        outline_w: 0.03, shadow: 0.0, position: "lower", max_chars: 24, ..BASE },
    Template { id: "cinematic", name: "Cinematic", category: "Music",
        description: "Tall, widely spaced capitals that fade in slowly. Film-style music videos, trailers, short films.",
        font: "Bebas Neue", bold: false, uppercase: true, size: 5.5, anim: Anim::SlowFade, outline_w: 0.0,
        shadow: 0.05, shadow_alpha: 140, spacing: 0.14, max_lines: 1, max_chars: 34, char_w: 0.269, em: 0.77, ..BASE },
    Template { id: "love-song", name: "Love Song", category: "Music",
        description: "Flowing script with a soft glow, fading in slowly. Love songs, weddings, beauty and fashion.",
        font: "Great Vibes", bold: false, size: 8.0, anim: Anim::SlowFade, outline_w: 0.02, outline: "#3A1E2A",
        shadow: 0.05, shadow_color: "#3A1E2A", shadow_alpha: 150, position: "lower", max_chars: 26, char_w: 0.164, em: 0.57, ..BASE },
    // ---------------- Stylish
    Template { id: "marker", name: "Marker", category: "Stylish",
        description: "Hand-drawn marker lettering, slightly tilted, popping in. Vlogs, comedy, behind-the-scenes.",
        font: "Permanent Marker", bold: false, size: 8.0, anim: Anim::Pop, tilt: -3.0, outline_w: 0.07,
        shadow: 0.04, position: "lower", max_words: 4, max_chars: 18, highlight: "#FFE600", mode: Mode::Highlight, char_w: 0.365, em: 0.7, ..BASE },
    Template { id: "glitch", name: "Glitch", category: "Stylish",
        description: "Red/blue split that snaps into place. Tech, gaming, sci-fi and edgy edits.",
        font: "Anton", bold: false, uppercase: true, size: 9.0, anim: Anim::Glitch, outline_w: 0.05,
        shadow: 0.0, position: "middle", max_words: 3, max_chars: 16, spacing: 0.02, char_w: 0.242, em: 0.576, ..BASE },
    Template { id: "editorial", name: "Editorial", category: "Stylish",
        description: "Elegant italic serif in warm cream that slides up gently. Fashion, food, travel, documentaries.",
        font: "DM Serif Display", bold: false, italic: true, size: 5.5, color: "#F5E6C8", anim: Anim::SlideUp,
        outline_w: 0.0, shadow: 0.05, shadow_alpha: 170, max_chars: 30, char_w: 0.32, em: 0.73, ..BASE },
];

pub fn template(id: &str) -> Option<&'static Template> {
    // Old style names from before templates existed.
    let id = match id {
        "bold" => "yellow-bold",
        x => x,
    };
    TEMPLATES.iter().find(|t| t.id == id)
}

#[derive(Debug, Clone, Deserialize)]
pub struct RenderOpts {
    pub width: u32,
    pub height: u32,
    /// Size multiplier, 1.0 = template default.
    pub scale: f32,
    /// Override position ("" = template default).
    pub position: String,
}

/// Font size in pixels. Based on the frame's shorter side, so text looks the same size on a
/// vertical phone video and a landscape one.
pub fn font_px(tpl: &Template, w: f32, h: f32, scale: f32) -> f32 {
    (tpl.size * scale.clamp(0.4, 2.5) / 100.0 * w.min(h)).max(8.0)
}

/// How to split captions for this template on a video of this size: the template's own
/// limits, narrowed so every line fits inside the frame.
pub fn layout(tpl: &Template, width: u32, height: u32, scale: f32) -> crate::captions::CueStyle {
    let (w, h) = (width.max(16) as f32, height.max(16) as f32);
    let fs = font_px(tpl, w, h, scale);
    let pad = if tpl.box_color.is_some() { fs * 0.56 } else { fs * tpl.outline_w * 2.0 };
    let usable = w * 0.88 - pad;
    let fit = (usable / (fs * (tpl.char_w + tpl.spacing))).floor().max(4.0) as usize;
    crate::captions::CueStyle {
        max_chars: tpl.max_chars.min(fit),
        max_lines: tpl.max_lines,
        max_words: tpl.max_words,
        uppercase: false, // templates apply capitals at render time, so edits stay readable
    }
}

// ------------------------------------------------------------------ ASS helpers

/// "#RRGGBB" + opacity (0..255) → ASS "&HAABBGGRR".
fn ass_color(hex: &str, opacity: u8) -> String {
    let h = hex.trim_start_matches('#');
    let v = u32::from_str_radix(h, 16).unwrap_or(0xFFFFFF);
    let (r, g, b) = ((v >> 16) & 255, (v >> 8) & 255, v & 255);
    format!("&H{:02X}{:02X}{:02X}{:02X}", 255 - opacity, b, g, r)
}
/// Colour override tag value (no alpha): "&HBBGGRR&".
fn tag_color(hex: &str) -> String {
    let h = hex.trim_start_matches('#');
    let v = u32::from_str_radix(h, 16).unwrap_or(0xFFFFFF);
    format!("&H{:02X}{:02X}{:02X}&", v & 255, (v >> 8) & 255, (v >> 16) & 255)
}

fn ass_time(t: f64) -> String {
    let cs = (t.max(0.0) * 100.0).round() as u64;
    format!("{}:{:02}:{:02}.{:02}", cs / 360_000, cs / 6000 % 60, cs / 100 % 60, cs % 100)
}

fn escape(s: &str) -> String {
    s.replace('\\', "/").replace('{', "(").replace('}', ")")
}

/// The caption's words with their timings. If the text was edited and no longer matches the
/// recognised words, timings are spread over the caption by word length.
fn timed_words(cue: &Cue) -> Vec<(String, f64, f64, bool)> {
    // (word, start, end, line-break-before)
    let mut toks: Vec<(String, bool)> = Vec::new();
    for (li, line) in cue.text.split('\n').enumerate() {
        for (wi, w) in line.split_whitespace().enumerate() {
            toks.push((w.to_string(), li > 0 && wi == 0));
        }
    }
    if toks.is_empty() {
        return vec![];
    }
    let words: &[Word] = &cue.words;
    if words.len() == toks.len() {
        return toks.into_iter().zip(words).map(|((t, br), w)| (t, w.start.max(cue.start), w.end.min(cue.end), br)).collect();
    }
    let total: usize = toks.iter().map(|t| t.0.chars().count() + 1).sum();
    let dur = (cue.end - cue.start).max(0.1);
    let mut t = cue.start;
    toks.into_iter()
        .map(|(w, br)| {
            let d = dur * (w.chars().count() + 1) as f64 / total as f64;
            let r = (w, t, t + d, br);
            t += d;
            r
        })
        .collect()
}

struct Ctx<'a> {
    tpl: &'a Template,
    x: f32,
    y: f32,
    h: f32,
}

impl Ctx<'_> {
    /// Entrance animation tags for an event of `dur` seconds (first = first event of the caption,
    /// last = last event of the caption).
    fn entrance(&self, first: bool, last: bool, dur: f64) -> String {
        let (x, y) = (self.x, self.y);
        let ms = (dur * 1000.0) as i64;
        let mut s = format!("\\pos({x:.0},{y:.0})");
        if self.tpl.tilt != 0.0 {
            let _ = write!(s, "\\frz{:.1}", -self.tpl.tilt);
        }
        let fade = |i: i64, o: i64| format!("\\fad({},{})", if first { i } else { 0 }, if last { o.min(ms / 3) } else { 0 });
        match self.tpl.anim {
            Anim::None => {}
            Anim::Fade => s.push_str(&fade(150, 120)),
            Anim::SlowFade => s.push_str(&fade(420, 300)),
            Anim::Pop if first => s.push_str("\\fscx72\\fscy72\\t(0,90,\\fscx108\\fscy108)\\t(90,170,\\fscx100\\fscy100)\\fad(60,0)"),
            Anim::Bounce if first => {
                let dy = self.h * 0.045;
                s = format!("\\move({x:.0},{:.0},{x:.0},{y:.0},0,140)\\fscy82\\t(0,130,\\fscy110)\\t(130,220,\\fscy100)\\fad(50,0)", y + dy);
            }
            Anim::SlideUp if first => {
                let dy = self.h * 0.03;
                s = format!("\\move({x:.0},{:.0},{x:.0},{y:.0},0,260)\\fad(240,{})", y + dy, if last { 160 } else { 0 });
            }
            Anim::Neon => {
                s.push_str(&fade(220, 160));
            }
            Anim::Glitch => {}
            _ => {
                if last {
                    s.push_str(&format!("\\fad(0,{})", 80.min(ms / 3)));
                }
            }
        }
        s
    }
}

fn style_line(name: &str, tpl: &Template, fs: f32, primary: &str, secondary: &str, align: u8, margin_v: u32) -> String {
    let (border_style, outline_px, outline_col, back_col) = match tpl.box_color {
        Some(c) => (3, fs * 0.28, ass_color(c, tpl.box_alpha), ass_color(c, tpl.box_alpha)),
        None => (1, fs * tpl.outline_w, ass_color(tpl.outline, 255), ass_color(tpl.shadow_color, tpl.shadow_alpha)),
    };
    format!(
        "Style: {name},{},{:.0},{primary},{secondary},{outline_col},{back_col},{},{},0,0,100,100,{:.1},0,{border_style},{:.1},{:.1},{align},20,20,{margin_v},1\n",
        tpl.font,
        fs,
        if tpl.bold { -1 } else { 0 },
        if tpl.italic { -1 } else { 0 },
        tpl.spacing * fs,
        outline_px,
        if tpl.box_color.is_some() { 0.0 } else { tpl.shadow * fs },
    )
}

/// Build the complete ASS script for these captions in this template.
pub fn to_ass(cues: &[Cue], tpl: &Template, o: &RenderOpts) -> String {
    let (w, h) = (o.width.max(16) as f32, o.height.max(16) as f32);
    let fs = font_px(tpl, w, h, o.scale);
    let position = if o.position.is_empty() { tpl.position } else { o.position.as_str() };
    let (align, y, margin_v) = match position {
        "top" => (8u8, h * 0.06, (h * 0.06) as u32),
        "middle" => (5, h * 0.5, 0),
        "lower" => (2, h * 0.75, (h * 0.25) as u32),
        _ => (2, h * 0.94, (h * 0.06) as u32),
    };
    let ctx = Ctx { tpl, x: w / 2.0, y, h };

    let mut s = String::new();
    let _ = write!(
        s,
        "[Script Info]\nScriptType: v4.00+\nPlayResX: {}\nPlayResY: {}\nWrapStyle: 2\nScaledBorderAndShadow: yes\nYCbCr Matrix: TV.709\n\n[V4+ Styles]\n\
Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n",
        w as u32, h as u32
    );
    let base = ass_color(tpl.color, 255);
    if tpl.mode == Mode::Karaoke {
        // \kf fills from SecondaryColour (not yet sung) to PrimaryColour (sung).
        s.push_str(&style_line("Main", tpl, fs, &ass_color(tpl.highlight, 255), &ass_color(tpl.color, 235), align, margin_v));
    } else {
        s.push_str(&style_line("Main", tpl, fs, &base, &base, align, margin_v));
    }
    s.push_str("\n[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n");

    let hl = tag_color(tpl.highlight);
    let basec = tag_color(tpl.color);
    let up = |w: &str| if tpl.uppercase { w.to_uppercase() } else { w.to_string() };
    let ev = |s: &mut String, layer: u8, a: f64, b: f64, text: &str| {
        if b - a >= 0.02 {
            let _ = writeln!(s, "Dialogue: {layer},{},{},Main,,0,0,0,,{text}", ass_time(a), ass_time(b));
        }
    };

    for cue in cues {
        let words = timed_words(cue);
        if words.is_empty() {
            continue;
        }
        let join = |f: &dyn Fn(usize, &str) -> String| -> String {
            let mut t = String::new();
            for (i, (wd, _, _, br)) in words.iter().enumerate() {
                if i > 0 {
                    t.push_str(if *br { "\\N" } else { " " });
                }
                t.push_str(&f(i, &escape(&up(wd))));
            }
            t
        };
        // Event boundaries for per-word modes: each word's moment lasts until the next word starts.
        let bounds: Vec<(f64, f64)> = (0..words.len())
            .map(|i| {
                let a = if i == 0 { cue.start } else { words[i].1 };
                let b = if i + 1 < words.len() { words[i + 1].1 } else { cue.end };
                (a, b.max(a + 0.04))
            })
            .collect();
        let n = words.len();

        match tpl.mode {
            Mode::Static => {
                let text = join(&|_, w| w.to_string());
                let dur = cue.end - cue.start;
                match tpl.anim {
                    Anim::Neon => {
                        // Layer 0: blurred glow that breathes; layer 1: crisp tube.
                        let g = tag_color(tpl.glow);
                        let mut pulse = String::new();
                        let mut t = 0i64;
                        let mut on = false;
                        while (t as f64) < dur * 1000.0 {
                            let _ = write!(pulse, "\\t({},{},\\3a&H{}&)", t, t + 600, if on { "10" } else { "70" });
                            on = !on;
                            t += 600;
                        }
                        let glow = format!("{{{}\\bord{:.1}\\3c{g}\\blur{:.1}\\1a&HFF&\\shad0{pulse}}}{text}", ctx.entrance(true, true, dur), fs * 0.22, fs * 0.35);
                        let tube = format!("{{{}\\bord{:.1}\\3c{g}\\blur{:.1}\\shad0}}{text}", ctx.entrance(true, true, dur), fs * 0.045, fs * 0.04);
                        ev(&mut s, 0, cue.start, cue.end, &glow);
                        ev(&mut s, 1, cue.start, cue.end, &tube);
                    }
                    Anim::Glitch => {
                        // Red and cyan copies snap in from the sides, then fade into the white text.
                        let off = fs * 0.12;
                        let (x, y) = (ctx.x, ctx.y);
                        let red = format!("{{\\move({:.0},{:.0},{x:.0},{y:.0},0,200)\\c&H3030FF&\\bord0\\shad0\\1a&H30&\\t(120,260,\\1a&HFF&)}}{text}", x - off, y - off * 0.3);
                        let cyan = format!("{{\\move({:.0},{:.0},{x:.0},{y:.0},0,200)\\c&HFFFF30&\\bord0\\shad0\\1a&H30&\\t(120,260,\\1a&HFF&)}}{text}", x + off, y + off * 0.3);
                        let main = format!("{{\\pos({x:.0},{y:.0})\\fad(90,80)\\fscx112\\t(0,160,\\fscx100)}}{text}");
                        ev(&mut s, 0, cue.start, (cue.start + 0.3).min(cue.end), &red);
                        ev(&mut s, 1, cue.start, (cue.start + 0.3).min(cue.end), &cyan);
                        ev(&mut s, 2, cue.start, cue.end, &main);
                    }
                    _ => {
                        let t = format!("{{{}}}{text}", ctx.entrance(true, true, dur));
                        ev(&mut s, 0, cue.start, cue.end, &t);
                    }
                }
            }
            Mode::Karaoke => {
                let mut t = format!("{{{}}}", ctx.entrance(true, true, cue.end - cue.start));
                let lead = ((words[0].1 - cue.start) * 100.0).round().max(0.0) as i64;
                if lead > 0 {
                    let _ = write!(t, "{{\\k{lead}}}");
                }
                for (i, (wd, a, _, br)) in words.iter().enumerate() {
                    let next = if i + 1 < n { words[i + 1].1 } else { cue.end - 0.15 };
                    let cs = ((next - a) * 100.0).round().max(1.0) as i64;
                    if i > 0 {
                        t.push_str(if *br { "\\N" } else { " " });
                    }
                    let _ = write!(t, "{{\\kf{cs}}}{}", escape(&up(wd)));
                }
                ev(&mut s, 0, cue.start, cue.end, &t);
            }
            Mode::Highlight | Mode::HighlightScale | Mode::HighlightCycle => {
                for (k, &(a, b)) in bounds.iter().enumerate() {
                    let col = if tpl.mode == Mode::HighlightCycle && !tpl.cycle.is_empty() { tag_color(tpl.cycle[k % tpl.cycle.len()]) } else { hl.clone() };
                    let scale = tpl.mode != Mode::Highlight;
                    let text = join(&|i, w| {
                        if i == k {
                            if scale {
                                format!("{{\\c{col}\\fscx100\\fscy100\\t(0,80,\\fscx120\\fscy120)\\t(80,170,\\fscx112\\fscy112)}}{w}{{\\c{basec}\\fscx100\\fscy100}}")
                            } else {
                                format!("{{\\c{col}}}{w}{{\\c{basec}}}")
                            }
                        } else {
                            w.to_string()
                        }
                    });
                    let t = format!("{{{}}}{text}", ctx.entrance(k == 0, k + 1 == n, b - a));
                    ev(&mut s, 0, a, b, &t);
                }
            }
            Mode::Build => {
                // Typewriter: left-aligned block that types on word by word. The block's place is
                // fixed from the finished caption, so lines never jump as words appear.
                let lines: Vec<String> = cue.text.split('\n').map(|l| up(l)).collect();
                let longest = lines.iter().map(|l| l.chars().count()).max().unwrap_or(1) as f32;
                let line_h = fs * 1.15 + if tpl.box_color.is_some() { fs * 0.2 } else { 0.0 };
                let block_w = longest * fs * (tpl.char_w + tpl.spacing);
                let block_h = lines.len() as f32 * line_h;
                let left = (w - block_w) / 2.0;
                let top = match position {
                    "top" => y,
                    "middle" => y - block_h / 2.0,
                    _ => y - block_h,
                };
                for (k, &(a, b)) in bounds.iter().enumerate() {
                    let mut text = String::new();
                    for (i, (wd, _, _, br)) in words.iter().enumerate().take(k + 1) {
                        if i > 0 {
                            text.push_str(if *br { "\\N" } else { " " });
                        }
                        text.push_str(&escape(&up(wd)));
                    }
                    let fade = if k + 1 == n { format!("\\fad(0,{})", (((b - a) * 1000.0) as i64 / 3).min(120)) } else { String::new() };
                    let t = format!("{{\\an7\\pos({left:.0},{top:.0}){fade}}}{text}");
                    ev(&mut s, 0, a, b, &t);
                }
            }
            Mode::Single => {
                for (k, &(a, b)) in bounds.iter().enumerate() {
                    let wd = escape(&up(&words[k].0));
                    let t = format!("{{{}}}{wd}", ctx.entrance(true, k + 1 == n, b - a));
                    ev(&mut s, 0, a, b, &t);
                }
            }
        }
    }
    s
}
