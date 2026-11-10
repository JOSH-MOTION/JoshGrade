//! Background jobs: exports run on their own threads so you can start several and keep
//! working (grade photos while a video exports). Each job reports progress through the
//! "job" event and can be cancelled on its own.

use grade_core::{lut, Settings};
use media_core::captions;
use media_core::edit::{self, EditClip, ExportOptions, Segment};
use media_core::export::{self, RenderOptions};
use media_core::ffmpeg::{self, Tools};
use media_core::sync::Match;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager, State};

#[derive(Serialize, Clone, Debug)]
pub struct JobInfo {
    pub id: u64,
    pub kind: String,
    pub title: String,
    /// 0..1, or -1 while the length is unknown.
    pub progress: f32,
    /// "running" | "done" | "failed" | "cancelled"
    pub state: String,
    pub message: Option<String>,
    pub outputs: Vec<String>,
    pub started: u64,
}

#[derive(Default)]
pub struct Jobs {
    next: AtomicU64,
    map: Mutex<BTreeMap<u64, (JobInfo, Arc<AtomicBool>)>>,
}

pub type JSt<'a> = State<'a, Arc<Jobs>>;

pub struct Ctx {
    id: u64,
    app: AppHandle,
    pub cancel: Arc<AtomicBool>,
    last: Mutex<Instant>,
}

impl Ctx {
    /// Report progress (throttled to ~6 updates a second).
    pub fn progress(&self, p: f32, message: Option<String>) {
        {
            let mut l = self.last.lock().unwrap();
            if l.elapsed().as_millis() < 160 && p < 1.0 {
                return;
            }
            *l = Instant::now();
        }
        let jobs = self.app.state::<Arc<Jobs>>();
        let info = {
            let mut m = jobs.map.lock().unwrap();
            let Some((j, _)) = m.get_mut(&self.id) else { return };
            j.progress = p;
            if message.is_some() {
                j.message = message;
            }
            j.clone()
        };
        let _ = self.app.emit("job", info);
    }
    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

/// Start a job on its own thread; returns its id straight away.
pub fn start(app: &AppHandle, kind: &str, title: String, work: impl FnOnce(&Ctx) -> Result<Vec<String>, String> + Send + 'static) -> u64 {
    let jobs = app.state::<Arc<Jobs>>();
    let id = jobs.next.fetch_add(1, Ordering::Relaxed) + 1;
    let cancel = Arc::new(AtomicBool::new(false));
    let info = JobInfo {
        id,
        kind: kind.into(),
        title,
        progress: -1.0,
        state: "running".into(),
        message: None,
        outputs: vec![],
        started: SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
    };
    jobs.map.lock().unwrap().insert(id, (info.clone(), cancel.clone()));
    let _ = app.emit("job", info);
    let app = app.clone();
    std::thread::spawn(move || {
        let ctx = Ctx { id, app: app.clone(), cancel: cancel.clone(), last: Mutex::new(Instant::now() - std::time::Duration::from_secs(1)) };
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(&ctx))).unwrap_or_else(|_| Err("Something went wrong inside this job.".into()));
        let jobs = app.state::<Arc<Jobs>>();
        let info = {
            let mut m = jobs.map.lock().unwrap();
            let Some((j, _)) = m.get_mut(&id) else { return };
            match r {
                Ok(outs) => {
                    j.state = "done".into();
                    j.progress = 1.0;
                    j.outputs = outs;
                    j.message = None;
                }
                Err(_) if cancel.load(Ordering::Relaxed) => {
                    j.state = "cancelled".into();
                    j.message = None;
                }
                Err(e) => {
                    j.state = "failed".into();
                    j.message = Some(e);
                }
            }
            j.clone()
        };
        let _ = app.emit("job", info);
    });
    id
}

#[tauri::command]
pub fn jobs_list(jobs: JSt<'_>) -> Vec<JobInfo> {
    jobs.map.lock().unwrap().values().map(|(j, _)| j.clone()).collect()
}

#[tauri::command]
pub fn job_cancel(jobs: JSt<'_>, id: u64) {
    if let Some((_, c)) = jobs.map.lock().unwrap().get(&id) {
        c.store(true, Ordering::Relaxed);
    }
}

/// Forget finished jobs (the files stay where they are).
#[tauri::command]
pub fn jobs_clear(jobs: JSt<'_>) {
    jobs.map.lock().unwrap().retain(|_, (j, _)| j.state == "running");
}

fn tools() -> Result<Tools, String> {
    Tools::locate().map_err(|e| e.to_string())
}

fn base(p: &str) -> String {
    ffmpeg::file_name(Path::new(p))
}

// ------------------------------------------------------------------ sync & captions as jobs

#[tauri::command]
pub fn job_sync_render(app: AppHandle, matches: Vec<Match>, out_dir: String, options: RenderOptions) -> u64 {
    let n = matches.len();
    let title = format!("Save {n} synced video{}", if n == 1 { "" } else { "s" });
    start(&app, "sync", title, move |ctx| {
        let t = tools()?;
        let mut outs = vec![];
        let mut errors = vec![];
        for (i, m) in matches.iter().enumerate() {
            if ctx.cancelled() {
                return Err("cancelled".into());
            }
            ctx.progress(i as f32 / n.max(1) as f32, Some(base(&m.clip)));
            match export::render_synced(&t, m, Path::new(&out_dir), &options) {
                Ok(p) => outs.push(p.to_string_lossy().into_owned()),
                Err(e) => errors.push(format!("{}: {e}", base(&m.clip))),
            }
        }
        if outs.is_empty() && !errors.is_empty() {
            return Err(errors.join("\n"));
        }
        Ok(outs)
    })
}

#[derive(Deserialize)]
pub struct CaptionJob {
    path: String,
    cues: Vec<captions::Cue>,
    language: String,
    mode: String,
    style: captions::TemplateStyle,
    out_dir: String,
}

#[tauri::command]
pub fn job_caption_video(app: AppHandle, req: CaptionJob) -> u64 {
    let fonts = app.path().resource_dir().ok().map(|d| d.join("fonts")).filter(|d| d.exists());
    let title = format!("{} – {}", if req.mode == "burn" { "Burn captions" } else { "Add caption track" }, base(&req.path));
    start(&app, "captions", title, move |_ctx| {
        let t = tools()?;
        let out = if req.mode == "burn" {
            if !t.can_burn_subtitles() {
                return Err("This copy of ffmpeg can't burn in captions. Use 'caption track' instead.".into());
            }
            captions::burn_template(&t, Path::new(&req.path), &req.cues, &req.style, fonts.as_deref(), Path::new(&req.out_dir))
        } else {
            captions::add_caption_track(&t, Path::new(&req.path), &req.cues, &req.language, Path::new(&req.out_dir))
        };
        out.map(|p| vec![p.to_string_lossy().into_owned()]).map_err(|e| e.to_string())
    })
}

// ------------------------------------------------------------------ video editing

/// Analyse a video for grading from frames spread through it (one grade for the whole edit,
/// so the look stays consistent from shot to shot).
fn video_photo(t: &Tools, clips: &[EditClip]) -> Result<grade_core::Photo, String> {
    let mut data = Vec::new();
    let mut width = 0;
    let mut height = 0;
    let n_per = (8 / clips.len().max(1)).clamp(1, 5);
    for c in clips {
        let info = match &c.info {
            Some(i) if i.duration > 0.0 => i.clone(),
            _ => ffmpeg::probe(t, Path::new(&c.path)).map_err(|e| e.to_string())?,
        };
        if !info.has_video {
            continue;
        }
        let segs = edit::clean(&c.keep, info.duration);
        let total: f64 = segs.iter().map(|s| s.len()).sum();
        for k in 0..n_per {
            // evenly through the kept parts
            let mut want = total * (k as f64 + 0.5) / n_per as f64;
            let mut at = 0.0;
            for s in &segs {
                if want <= s.len() {
                    at = s.start + want;
                    break;
                }
                want -= s.len();
            }
            let (w, h, rgb) = edit::frame_rgb(t, Path::new(&c.path), at, 400).map_err(|e| e.to_string())?;
            // stack frames vertically, padding to the widest
            if width == 0 {
                width = w;
            }
            for y in 0..h {
                for x in 0..width {
                    let px = if x < w { let i = ((y * w + x) * 3) as usize; [rgb[i] as f32 / 255.0, rgb[i + 1] as f32 / 255.0, rgb[i + 2] as f32 / 255.0] } else { [0.5; 3] };
                    data.push(px);
                }
            }
            height += h;
        }
    }
    if data.is_empty() {
        return Err("No video to grade.".into());
    }
    Ok(grade_core::Photo { width, height, data, from_raw: false })
}

pub fn video_lut(t: &Tools, clips: &[EditClip], settings: &Settings) -> Result<PathBuf, String> {
    let photo = video_photo(t, clips)?;
    let (_, grade) = grade_core::grade_for(&photo, settings).map_err(|e| e.to_string())?;
    static N: AtomicU64 = AtomicU64::new(0);
    let p = std::env::temp_dir().join(format!("joshgrade-video-{}-{}.cube", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    std::fs::write(&p, lut::cube_string(&grade, 33, "JoshGrade video")).map_err(|e| e.to_string())?;
    Ok(p)
}

#[tauri::command]
pub fn job_edit_export(app: AppHandle, clips: Vec<EditClip>, grade: Option<Settings>, dest: String) -> u64 {
    let title = format!("Export {}", base(&dest));
    start(&app, "edit", title, move |ctx| {
        let t = tools()?;
        let mut opts = ExportOptions::default();
        let lut = match &grade {
            Some(s) => {
                ctx.progress(0.0, Some("Working out the grade…".into()));
                Some(video_lut(&t, &clips, s)?)
            }
            None => None,
        };
        opts.lut = lut.as_ref().map(|p| p.to_string_lossy().into_owned());
        ctx.progress(0.01, Some("Rendering…".into()));
        let r = edit::export(&t, &clips, &opts, Path::new(&dest), &ctx.cancel, |p| ctx.progress(0.01 + p * 0.99, None));
        if let Some(l) = lut {
            let _ = std::fs::remove_file(l);
        }
        r.map(|p| vec![p.to_string_lossy().into_owned()]).map_err(|e| e.to_string())
    })
}

#[derive(Serialize)]
pub struct Pauses {
    silences: Vec<Segment>,
    keep: Vec<Segment>,
}

/// Find the pauses in a clip and the parts to keep without them.
#[tauri::command]
pub async fn edit_find_pauses(path: String, threshold_db: f64, min_len: f64, pad: f64) -> Result<Pauses, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let t = tools()?;
        let info = ffmpeg::probe(&t, Path::new(&path)).map_err(|e| e.to_string())?;
        let silences = edit::find_silences(&t, Path::new(&path), threshold_db, min_len, &AtomicBool::new(false)).map_err(|e| e.to_string())?;
        let keep = edit::keep_without(&silences, info.duration, pad);
        Ok(Pauses { silences, keep })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// One frame of a video, graded with `settings` (for the grade preview), as a JPEG data URL.
/// The grade is worked out from frames across the whole clip, like the export.
#[tauri::command]
pub async fn edit_graded_frame(path: String, time: f64, settings: Option<Settings>) -> Result<String, String> {
    use base64::Engine;
    tauri::async_runtime::spawn_blocking(move || {
        let t = tools()?;
        let (w, h, rgb) = edit::frame_rgb(&t, Path::new(&path), time, 960).map_err(|e| e.to_string())?;
        let mut photo = grade_core::Photo { width: w, height: h, data: rgb.chunks_exact(3).map(|c| [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0]).collect(), from_raw: false };
        if let Some(s) = settings {
            let clip = EditClip { path: path.clone(), info: None, keep: vec![] };
            let key = format!("{path}");
            let analysis = {
                static CACHE: Mutex<Option<(String, grade_core::Analysis)>> = Mutex::new(None);
                let mut c = CACHE.lock().unwrap();
                match &*c {
                    Some((k, a)) if *k == key => a.clone(),
                    _ => {
                        let a = grade_core::analyze(&video_photo(&t, &[clip])?);
                        *c = Some((key, a.clone()));
                        a
                    }
                }
            };
            let g = grade_core::grade_with(&analysis, &s).map_err(|e| e.to_string())?;
            photo = g.apply(&photo);
        }
        let bytes = grade_core::encode::jpeg_bytes(&photo, 85).map_err(|e| e.to_string())?;
        Ok(format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)))
    })
    .await
    .map_err(|e| e.to_string())?
}
