//! JoshGrade video tools: PluralEyes-style audio sync and offline auto captions.

pub mod captions;
pub mod export;
pub mod ffmpeg;
pub mod sync;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("file error: {0}")]
    Io(#[from] std::io::Error),
    #[error("cancelled")]
    Cancelled,
    #[error("{0}")]
    Other(String),
}

/// File types JoshGrade treats as video / audio.
pub const VIDEO_EXT: &[&str] = &["mp4", "mov", "m4v", "mxf", "mts", "m2ts", "avi", "mkv", "webm", "3gp", "insv", "lrf"];
pub const AUDIO_EXT: &[&str] = &["wav", "mp3", "m4a", "aac", "aif", "aiff", "flac", "ogg", "opus", "caf", "wma", "bwf"];

fn ext(p: &std::path::Path) -> String {
    p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase()
}
pub fn is_video(p: &std::path::Path) -> bool {
    VIDEO_EXT.contains(&ext(p).as_str())
}
pub fn is_audio(p: &std::path::Path) -> bool {
    AUDIO_EXT.contains(&ext(p).as_str())
}
