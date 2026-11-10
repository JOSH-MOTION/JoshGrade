import { ReactNode, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { EditItem, EditTab } from "./EditTab";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open, save } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { CaptionView, Template, TemplateTile } from "./CaptionTemplates";

// ---------------------------------------------------------------- types
type MediaInfo = {
  path: string; duration: number; has_video: boolean; has_audio: boolean; width: number; height: number;
  fps_num: number; fps_den: number; audio_channels: number; sample_rate: number; timecode: string | null; start_epoch?: number | null;
};
type Item = { path: string; kind: "video" | "audio"; info?: MediaInfo | null; error?: string | null };
type Confidence = "high" | "medium" | "low" | "none";
type Match = {
  clip: string; reference: string | null; offset: number; drift_ppm: number; score: number;
  confidence: Confidence; overlap: number; clip_info: MediaInfo; envelope: number[];
};
type Reference = { path: string; info: MediaInfo; envelope: number[] };
type Report = { references: Reference[]; matches: Match[]; positions?: number[] };
type Word = { text: string; start: number; end: number; prob: number };
type Cue = { start: number; end: number; text: string; words: Word[] };
type Transcript = { language: string; duration: number; words: Word[] };
type ModelState = { id: string; label: string; size_mb: number; description: string; downloaded: boolean };
type Step = { step: string; done: number; total: number };

const baseName = (p: string) => p.split(/[\\/]/).pop() ?? p;
const dirName = (p: string) => p.slice(0, Math.max(p.lastIndexOf("/"), p.lastIndexOf("\\")));
const stem = (p: string) => baseName(p).replace(/\.[^.]+$/, "");

/** 83.4567 → "1:23.457" (minutes:seconds.ms), with sign for negative offsets. */
function fmtTime(t: number, ms = true) {
  const sign = t < -0.0005 ? "−" : "";
  // Round first so 59.9996 shows as 1:00.000, not 0:60.000.
  const a = ms ? Math.round(Math.abs(t) * 1000) / 1000 : Math.floor(Math.abs(t));
  const h = Math.floor(a / 3600);
  const m = Math.floor((a % 3600) / 60);
  const s = a % 60;
  const ss = ms ? s.toFixed(3).padStart(6, "0") : Math.floor(s).toString().padStart(2, "0");
  return `${sign}${h ? `${h}:${String(m).padStart(2, "0")}` : m}:${ss}`;
}
const fmtDur = (t: number) => fmtTime(t, false);

const CONF_TEXT: Record<Confidence, string> = {
  high: "Synced",
  medium: "Synced – check it",
  low: "Unsure – check it",
  none: "No match",
};
const CONF_TIP: Record<Confidence, string> = {
  high: "The sound lines up clearly in one place. Accurate to a fraction of a frame.",
  medium: "Found a match, but the sound is noisy or a similar sound happens more than once. Play it back once to confirm.",
  low: "Weak match. Check it before you rely on it.",
  none: "This clip's sound wasn't found in the recording. It may have been recorded at a different time, or have no usable sound.",
};

// ---------------------------------------------------------------- small pieces
function Wave({ env, color }: { env: number[]; color: string }) {
  if (!env.length) return null;
  const n = env.length;
  const pts = env.map((v, i) => `${(i / Math.max(1, n - 1)) * 1000},${50 - v * 46}`).join(" ");
  const bottom = env.map((v, i) => `${((n - 1 - i) / Math.max(1, n - 1)) * 1000},${50 + env[n - 1 - i] * 46}`).join(" ");
  return (
    <svg className="wave" viewBox="0 0 1000 100" preserveAspectRatio="none" aria-hidden>
      <polygon points={`${pts} ${bottom}`} fill={color} />
    </svg>
  );
}

function Spinner() {
  return <span className="spinner small" aria-hidden />;
}

// ---------------------------------------------------------------- app
export default function VideoApp({ modeSwitch, active }: { modeSwitch?: ReactNode; active: boolean }) {
  const [tab, setTab] = useState<"sync" | "captions" | "edit">("sync");
  const [editItems, setEditItems] = useState<EditItem[]>([]);
  const [toast, setToast] = useState<{ text: string; reveal?: string } | null>(null);
  const [dragOver, setDragOver] = useState(false);
  const activeRef = useRef(active);
  activeRef.current = active;
  const tabRef = useRef(tab);
  tabRef.current = tab;

  // sync state
  const [clips, setClips] = useState<Item[]>([]);
  const [recs, setRecs] = useState<Item[]>([]);
  const [report, setReport] = useState<Report | null>(null);
  const [syncing, setSyncing] = useState(false);
  const [step, setStep] = useState<Step | null>(null);
  const [selectedClip, setSelectedClip] = useState<string | null>(null);
  const [keepCam, setKeepCam] = useState(true);
  const [fixDrift, setFixDrift] = useState(true);
  const [render, setRender] = useState<{ done: number; total: number; running: boolean; dir: string; failed: number } | null>(null);

  // captions state
  const [capItems, setCapItems] = useState<Item[]>([]);
  const [capSel, setCapSel] = useState(0);
  const [models, setModels] = useState<ModelState[]>([]);
  const [model, setModel] = useState("accurate");
  const [download, setDownload] = useState<{ id: string; got: number; total: number } | null>(null);
  const [language, setLanguage] = useState("auto");
  const [translate, setTranslate] = useState(false);
  const [transcripts, setTranscripts] = useState<Record<string, Transcript>>({});
  const [cues, setCues] = useState<Record<string, Cue[]>>({});
  const [capRun, setCapRun] = useState<{ percent: number; latest: string } | null>(null);
  const [templates, setTemplates] = useState<Template[]>([]);
  const [tplId, setTplId] = useState("classic");
  const [scale, setScale] = useState(1);
  const [position, setPosition] = useState(""); // "" = the template's own position
  const [lines, setLines] = useState(0); // 0 = template default
  const [wordsAt, setWordsAt] = useState(0); // 0 = template default
  const burning: string | null = null;
  const [now, setNow] = useState(0);
  const videoRef = useRef<HTMLVideoElement & HTMLAudioElement>(null);
  const playerRef = useRef<HTMLDivElement>(null);
  const [videoBox, setVideoBox] = useState({ w: 0, h: 0, top: 0, left: 0 });

  // ---------- adding media
  const addPaths = useCallback(async (paths: string[]) => {
    const { items, skipped } = await invoke<{ items: Item[]; skipped: string[] }>("media_collect", { paths });
    if (!items.length) {
      setToast({ text: `${skipped.length ? `“${baseName(skipped[0])}”` : "That"} isn't a video or audio file JoshGrade can open.` });
      return;
    }
    const broken = items.filter((i) => i.error);
    const good = items.filter((i) => !i.error);
    if (broken.length) setToast({ text: `Couldn't read ${broken.length} file${broken.length > 1 ? "s" : ""}: ${broken[0].error}` });
    const merge = (prev: Item[], add: Item[]) => {
      const known = new Set(prev.map((p) => p.path));
      return [...prev, ...add.filter((a) => !known.has(a.path))];
    };
    if (tabRef.current === "sync") {
      setClips((p) => merge(p, good.filter((i) => i.kind === "video")));
      setRecs((p) => merge(p, good.filter((i) => i.kind === "audio")));
      setReport(null);
    } else if (tabRef.current === "edit") {
      setEditItems((p) => merge(p as Item[], good.filter((i) => i.kind === "video")) as unknown as EditItem[]);
      if (good.some((i) => i.kind !== "video")) setToast({ text: "Edit works on videos; audio files were skipped." });
    } else {
      setCapItems((p) => merge(p, good));
    }
  }, []);

  useEffect(() => {
    const un = getCurrentWebview().onDragDropEvent((e) => {
      if (!activeRef.current) return;
      if (e.payload.type === "over" || e.payload.type === "enter") setDragOver(true);
      else if (e.payload.type === "drop") {
        setDragOver(false);
        addPaths(e.payload.paths);
      } else setDragOver(false);
    });
    const subs = [
      listen<Step>("sync-progress", (e) => setStep(e.payload)),
      listen<{ index: number; total: number; error?: string }>("file-progress", (e) =>
        setRender((r) => r && { ...r, done: e.payload.index + 1, failed: r.failed + (e.payload.error ? 1 : 0) }),
      ),
      listen<{ id: string; got: number; total: number }>("model-progress", (e) => setDownload(e.payload)),
      listen<{ percent: number; latest: string }>("caption-progress", (e) => setCapRun(e.payload)),
    ];
    return () => {
      un.then((f) => f());
      subs.forEach((s) => s.then((f) => f()));
    };
  }, [addPaths]);

  const refreshModels = useCallback(() => {
    invoke<ModelState[]>("caption_models").then((m) => {
      setModels(m);
    });
  }, []);
  useEffect(refreshModels, [refreshModels]);

  useEffect(() => {
    if (!toast) return;
    const t = setTimeout(() => setToast(null), 8000);
    return () => clearTimeout(t);
  }, [toast]);

  const pick = async (kind: "video" | "audio" | "any") => {
    const ext =
      kind === "audio"
        ? ["wav", "mp3", "m4a", "aac", "aif", "aiff", "flac", "ogg", "opus", "caf", "bwf"]
        : kind === "video"
          ? ["mp4", "mov", "m4v", "mxf", "mts", "m2ts", "avi", "mkv", "webm", "3gp"]
          : ["mp4", "mov", "m4v", "mxf", "mts", "m2ts", "avi", "mkv", "webm", "wav", "mp3", "m4a", "aac", "aif", "aiff", "flac"];
    const r = await open({ multiple: true, filters: [{ name: kind === "audio" ? "Audio" : "Media", extensions: [...ext, ...ext.map((e) => e.toUpperCase())] }] });
    if (!r) return;
    const paths = Array.isArray(r) ? r : [r];
    if (kind === "audio" && tab === "sync") {
      // Files picked with "Add recorder audio" always go to the recorder list, even if they're videos.
      const { items } = await invoke<{ items: Item[] }>("media_collect", { paths });
      setRecs((p) => [...p, ...items.filter((i) => !i.error && !p.some((x) => x.path === i.path))]);
      setReport(null);
    } else addPaths(paths);
  };

  // ================================================================ SYNC
  const runSync = async () => {
    setSyncing(true);
    setReport(null);
    setStep({ step: "Starting…", done: 0, total: 1 });
    try {
      const r = await invoke<Report>("sync_run", { clips: clips.map((c) => c.path), references: recs.map((c) => c.path) });
      setReport(r);
      setSelectedClip(r.matches.find((m) => m.reference)?.clip ?? null);
    } catch (e) {
      if (String(e) !== "cancelled") setToast({ text: `Sync stopped: ${e}` });
    }
    setSyncing(false);
    setStep(null);
  };

  const synced = report?.matches.filter((m) => m.reference && !report.references.some((r) => r.path === m.clip)) ?? [];
  const unmatched = report?.matches.filter((m) => !m.reference) ?? [];

  const saveSynced = async () => {
    if (!synced.length) return;
    const dir = await open({ directory: true, title: "Choose a folder for the synced videos", defaultPath: dirName(synced[0].clip) });
    if (typeof dir !== "string") return;
    try {
      await invoke("job_sync_render", { matches: synced, outDir: dir, options: { keep_camera_audio: keepCam, fix_drift: fixDrift } });
      setToast({ text: `Saving ${synced.length} synced video${synced.length > 1 ? "s" : ""} in the background. Follow it in Jobs (bottom left) and keep working.` });
    } catch (e) {
      setToast({ text: String(e) });
    }
  };

  const saveTimeline = async () => {
    if (!report) return;
    const dest = await save({
      title: "Save timeline for Premiere / DaVinci",
      defaultPath: `${dirName(synced[0]?.clip ?? report.references[0]?.path ?? "")}/JoshGrade Sync.xml`,
      filters: [{ name: "XML timeline", extensions: ["xml"] }],
    });
    if (!dest) return;
    try {
      await invoke("sync_timeline", { report, dest });
      setToast({ text: "Timeline saved. In Premiere: File › Import. In DaVinci Resolve: File › Import › Timeline.", reveal: dest });
    } catch (e) {
      setToast({ text: `Couldn't save the timeline: ${e}` });
    }
  };

  // ================================================================ CAPTIONS
  const capItem = capItems[capSel] as Item | undefined;
  useEffect(() => {
    if (capSel >= capItems.length && capItems.length) setCapSel(capItems.length - 1);
  }, [capItems.length, capSel]);
  const transcript = capItem ? transcripts[capItem.path] : undefined;
  const itemCues = capItem ? cues[capItem.path] : undefined;
  const modelState = models.find((m) => m.id === model);

  useEffect(() => {
    invoke<Template[]>("caption_templates").then(setTemplates);
  }, []);
  const tpl = templates.find((t) => t.id === tplId);
  const categories = useMemo(() => [...new Set(templates.map((t) => t.category))], [templates]);
  const vw = capItem?.info?.width || 1920;
  const vh = capItem?.info?.height || 1080;

  // Re-split captions when the transcript, template or layout changes (fits the video's shape).
  useEffect(() => {
    if (!capItem || !transcript) return;
    invoke<Cue[]>("caption_cues", { words: transcript.words, template: tplId, width: vw, height: vh, scale, lines, wordsAtOnce: wordsAt })
      .then((c) => setCues((m) => ({ ...m, [capItem.path]: c })));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [capItem?.path, transcript, tplId, scale, lines, wordsAt, vw, vh]);

  const downloadModel = async () => {
    setDownload({ id: model, got: 0, total: (modelState?.size_mb ?? 0) * 1e6 });
    try {
      await invoke("caption_download", { id: model });
      refreshModels();
    } catch (e) {
      if (String(e) !== "cancelled") setToast({ text: String(e) });
    }
    setDownload(null);
  };

  const generate = async () => {
    if (!capItem) return;
    setCapRun({ percent: 0, latest: "" });
    try {
      const t = await invoke<Transcript>("caption_transcribe", { path: capItem.path, model, language, translate });
      setTranscripts((m) => ({ ...m, [capItem.path]: t }));
      if (!t.words.length) setToast({ text: "No speech was found in this file." });
    } catch (e) {
      if (String(e) !== "cancelled") setToast({ text: `Captions stopped: ${e}` });
    }
    setCapRun(null);
  };

  const editCue = (i: number, text: string) => {
    if (!capItem) return;
    setCues((m) => ({ ...m, [capItem.path]: m[capItem.path].map((c, j) => (j === i ? { ...c, text } : c)) }));
  };

  const saveCaptionFile = async (format: "srt" | "vtt" | "txt") => {
    if (!capItem || !itemCues) return;
    const dest = await save({ defaultPath: `${dirName(capItem.path)}/${stem(capItem.path)}.${format}`, filters: [{ name: format.toUpperCase(), extensions: [format] }] });
    if (!dest) return;
    try {
      await invoke("caption_save", { cues: itemCues, words: format === "txt" ? transcript?.words ?? [] : [], format, dest });
      setToast({ text: `Saved ${baseName(dest)}`, reveal: dest });
    } catch (e) {
      setToast({ text: `Couldn't save: ${e}` });
    }
  };

  const intoVideo = async (mode: "track" | "burn") => {
    if (!capItem || !itemCues) return;
    const dir = await open({ directory: true, title: mode === "burn" ? "Where should the captioned video go?" : "Where should the video with captions go?", defaultPath: dirName(capItem.path) });
    if (typeof dir !== "string") return;
    try {
      await invoke("job_caption_video", {
        req: { path: capItem.path, cues: itemCues, language: transcript?.language ?? "en", mode, style: { template: tplId, scale, position }, out_dir: dir },
      });
      setToast({ text: `${mode === "burn" ? "Burning captions" : "Adding the caption track"} in the background. Follow it in Jobs (bottom left). You can caption the next video now.` });
    } catch (e) {
      setToast({ text: String(e) });
    }
  };

  const activeCue = itemCues?.findIndex((c) => now >= c.start && now <= c.end) ?? -1;
  const cueListRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (activeCue < 0) return;
    const el = cueListRef.current?.querySelector<HTMLElement>(`[data-cue="${activeCue}"]`);
    el?.scrollIntoView({ block: "nearest", behavior: "smooth" });
  }, [activeCue]);

  // Caption preview is sized and placed relative to the visible video picture (not the letterbox).
  useEffect(() => {
    const el = playerRef.current;
    if (!el) return;
    const measure = () => {
      const v = videoRef.current as HTMLVideoElement | null;
      const W = el.clientWidth;
      const H = el.clientHeight - 40; // leave room for the playback controls
      if (v && v.videoWidth) {
        const s = Math.min(W / v.videoWidth, H / v.videoHeight);
        const h = v.videoHeight * s;
        const w = v.videoWidth * s;
        setVideoBox({ w, h, top: (H - h) / 2, left: (W - w) / 2 });
      } else setVideoBox({ w: W, h: H, top: 0, left: 0 });
    };
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    const v = videoRef.current;
    v?.addEventListener("loadedmetadata", measure);
    return () => {
      ro.disconnect();
      v?.removeEventListener("loadedmetadata", measure);
    };
  }, [capItem?.path, tab]);

  // ================================================================ render
  return (
    <div className={`app video-app ${dragOver ? "drag-over" : ""}`} style={active ? undefined : { display: "none" }}>
      <header className="titlebar" data-tauri-drag-region>
        <span className="brand" data-tauri-drag-region>JoshGrade</span>
        {modeSwitch}
        <div className="tabs" role="tablist" aria-label="Video tools">
          <button role="tab" aria-selected={tab === "sync"} className={tab === "sync" ? "on" : ""} onClick={() => setTab("sync")}>Sync audio</button>
          <button role="tab" aria-selected={tab === "captions"} className={tab === "captions" ? "on" : ""} onClick={() => setTab("captions")}>Auto captions</button>
          <button role="tab" aria-selected={tab === "edit"} className={tab === "edit" ? "on" : ""} onClick={() => setTab("edit")}>Edit</button>
        </div>
        <div className="title-actions">
          {tab === "sync" ? (
            <>
              <button onClick={() => pick("video")}>Add camera clips</button>
              <button onClick={() => pick("audio")}>Add recorder audio</button>
            </>
          ) : (
            <button onClick={() => pick(tab === "edit" ? "video" : "any")}>Add videos</button>
          )}
        </div>
      </header>

      {tab === "edit" ? (
        <EditTab items={editItems} setItems={setEditItems} toast={(text) => setToast({ text })} />
      ) : tab === "sync" ? (
        <>
          {/* ---------- inputs */}
          <aside className="strip media-lists" aria-label="Files to sync">
            <section>
              <h2>Recorder audio</h2>
              {recs.length ? (
                <ul className="media-list">
                  {recs.map((r) => (
                    <li key={r.path} title={r.path}>
                      <span className="mi-icon audio" aria-hidden>♪</span>
                      <span className="mi-name">{baseName(r.path)}</span>
                      <span className="mi-dur">{r.info ? fmtDur(r.info.duration) : ""}</span>
                      <button className="mi-remove" onClick={() => { setRecs((p) => p.filter((x) => x.path !== r.path)); setReport(null); }} aria-label={`Remove ${baseName(r.path)}`}>×</button>
                    </li>
                  ))}
                </ul>
              ) : (
                <p className="hint-small">Your Zoom / Tascam / lav recording. No recorder? Leave this empty and JoshGrade lines the cameras up with each other.</p>
              )}
            </section>
            <section>
              <h2>Camera clips</h2>
              {clips.length ? (
                <ul className="media-list">
                  {clips.map((c) => {
                    const m = report?.matches.find((x) => x.clip === c.path);
                    return (
                      <li key={c.path} title={c.path} className={selectedClip === c.path ? "on" : ""} onClick={() => setSelectedClip(c.path)}>
                        <span className={`mi-icon video ${m ? `c-${m.confidence}` : ""}`} aria-hidden>▶</span>
                        <span className="mi-name">{baseName(c.path)}</span>
                        <span className="mi-dur">{c.info ? fmtDur(c.info.duration) : ""}</span>
                        <button className="mi-remove" onClick={(e) => { e.stopPropagation(); setClips((p) => p.filter((x) => x.path !== c.path)); setReport(null); }} aria-label={`Remove ${baseName(c.path)}`}>×</button>
                      </li>
                    );
                  })}
                </ul>
              ) : (
                <p className="hint-small">Drop your camera clips here. Any camera, any mix.</p>
              )}
            </section>
          </aside>

          {/* ---------- stage */}
          <main className="stage sync-stage">
            {!report ? (
              <div className="empty">
                <div className="empty-frame">
                  <h1>{syncing ? "Syncing…" : "Sync camera clips to your recorder"}</h1>
                  {syncing && step ? (
                    <div className="progress big" aria-live="polite">
                      <div className="bar"><span style={{ width: `${(step.done / Math.max(1, step.total)) * 100}%` }} /></div>
                      <p>{step.step}</p>
                      <button onClick={() => invoke("media_cancel")}>Stop</button>
                    </div>
                  ) : (
                    <>
                      <p>Drop your camera clips and the separately recorded audio. JoshGrade listens to both and lines them up by their sound, like PluralEyes, accurate to a fraction of a frame. It also measures clock drift on long clips.</p>
                      <div className="empty-actions">
                        <button className="primary" disabled={!clips.length} onClick={runSync}>
                          {clips.length ? `Sync ${clips.length} clip${clips.length > 1 ? "s" : ""}` : "Add clips to sync"}
                        </button>
                        {!clips.length && <button onClick={() => pick("video")}>Add camera clips</button>}
                      </div>
                      <p className="formats">Works with MP4, MOV, MXF, MTS and more. Recorder audio: WAV, MP3, M4A, AIFF, FLAC. Nothing leaves this Mac.</p>
                    </>
                  )}
                </div>
              </div>
            ) : (
              <SyncTimeline report={report} selected={selectedClip} onSelect={setSelectedClip} />
            )}
          </main>

          {/* ---------- results + export */}
          <section className="panel" aria-label="Sync results">
            {report ? (
              <>
                <div className="group">
                  <div className="row">
                    <h2>Results</h2>
                    <button className="link" onClick={runSync} disabled={syncing}>Sync again</button>
                  </div>
                  <p className="sync-summary">
                    <strong>{synced.length}</strong> of {synced.length + unmatched.length} clip{synced.length + unmatched.length === 1 ? "" : "s"} synced
                    {unmatched.length > 0 && <span className="warn-text"> · {unmatched.length} couldn't be matched</span>}
                  </p>
                  <ul className="result-list">
                    {report.matches
                      .filter((m) => !report.references.some((r) => r.path === m.clip))
                      .map((m) => (
                        <li key={m.clip} className={selectedClip === m.clip ? "on" : ""}>
                          <button onClick={() => setSelectedClip(m.clip)}>
                            <span className="r-name">{baseName(m.clip)}</span>
                            <span className={`conf c-${m.confidence}`} title={CONF_TIP[m.confidence]}>{CONF_TEXT[m.confidence]}</span>
                            {m.reference && (
                              <span className="r-detail">
                                Starts at {fmtTime(m.offset)} in {baseName(m.reference)}
                                {Math.abs(m.drift_ppm) >= 5 && (
                                  <> · clock drift {Math.abs(m.drift_ppm).toFixed(0)} ppm ({((Math.abs(m.drift_ppm) * m.clip_info.duration) / 1000).toFixed(1)} ms over the clip)</>
                                )}
                              </span>
                            )}
                          </button>
                        </li>
                      ))}
                  </ul>
                </div>

                <div className="group export">
                  <h2>Export</h2>
                  <label className="check">
                    <input type="checkbox" checked={keepCam} onChange={(e) => setKeepCam(e.target.checked)} />
                    Keep the camera's own sound as a second track
                  </label>
                  <label className="check">
                    <input type="checkbox" checked={fixDrift} onChange={(e) => setFixDrift(e.target.checked)} />
                    Correct clock drift on long clips
                  </label>
                  {render && (
                    <div className="progress" aria-live="polite">
                      <div className="bar"><span style={{ width: `${(render.done / render.total) * 100}%` }} /></div>
                      <div className="row small">
                        <span>{render.running ? `Saving ${Math.min(render.done + 1, render.total)} of ${render.total}` : `Saved ${render.done - render.failed} of ${render.total}`}{render.failed ? `, ${render.failed} failed` : ""}</span>
                        {render.running ? <button className="link" onClick={() => invoke("media_cancel")}>Stop</button> : <button className="link" onClick={() => revealItemInDir(render.dir)}>Show in Finder</button>}
                      </div>
                    </div>
                  )}
                  <button className="primary wide" disabled={!synced.length || render?.running} onClick={saveSynced}>Save synced videos…</button>
                  <p className="save-hint">New files with the recorder's sound laid in. The picture is copied untouched, so there's no quality loss and it's fast.</p>
                  <button className="wide" disabled={!synced.length} onClick={saveTimeline}>Export timeline for Premiere / DaVinci…</button>
                  <p className="save-hint">One XML timeline with every clip and every recorder file already in place. Premiere: File › Import. DaVinci Resolve: File › Import › Timeline.</p>
                </div>
              </>
            ) : (
              <div className="group">
                <h2>How it works</h2>
                <ol className="howto">
                  <li>Add the camera clips and the recorder's audio file (or drop a whole card folder; JoshGrade sorts video and audio).</li>
                  <li>Press <strong>Sync</strong>. Each clip is placed on the recorder's timeline by its sound.</li>
                  <li>Save new synced videos, or export a timeline for Premiere / DaVinci.</li>
                </ol>
                <p className="save-hint">Tip: a clap or a loud moment at the start of each take makes syncing bulletproof, but it isn't required.</p>
              </div>
            )}
          </section>
        </>
      ) : (
        <>
          {/* ================= CAPTIONS ================= */}
          <aside className="strip media-lists" aria-label="Videos to caption">
            <section>
              <h2>Videos</h2>
              {capItems.length ? (
                <ul className="media-list">
                  {capItems.map((c, i) => (
                    <li key={c.path} title={c.path} className={i === capSel ? "on" : ""} onClick={() => setCapSel(i)}>
                      <span className={`mi-icon ${c.kind}`} aria-hidden>{c.kind === "audio" ? "♪" : "▶"}</span>
                      <span className="mi-name">{baseName(c.path)}</span>
                      <span className="mi-dur">{transcripts[c.path] ? "✓" : c.info ? fmtDur(c.info.duration) : ""}</span>
                      <button className="mi-remove" onClick={(e) => { e.stopPropagation(); setCapItems((p) => p.filter((x) => x.path !== c.path)); }} aria-label={`Remove ${baseName(c.path)}`}>×</button>
                    </li>
                  ))}
                </ul>
              ) : (
                <p className="hint-small">Drop the videos (or audio) you want captioned.</p>
              )}
            </section>
          </aside>

          <main className="stage cap-stage">
            {!capItem ? (
              <div className="empty">
                <div className="empty-frame">
                  <h1>Auto captions</h1>
                  <p>JoshGrade listens to your video and writes the captions, timed to each word. It runs on this Mac, so it works offline and nothing is uploaded.</p>
                  <div className="empty-actions">
                    <button className="primary" onClick={() => pick("any")}>Add videos</button>
                  </div>
                  <p className="formats">Export SRT for Premiere, CapCut and YouTube, add a caption track, or burn captions into the video.</p>
                </div>
              </div>
            ) : (
              <div className="cap-layout">
                <div className="player" ref={playerRef}>
                  {capItem.kind === "video" ? (
                    <video ref={videoRef} src={convertFileSrc(capItem.path)} controls onTimeUpdate={(e) => setNow(e.currentTarget.currentTime)} />
                  ) : (
                    <div className="audio-only">
                      <span>♪ {baseName(capItem.path)}</span>
                      <audio ref={videoRef} src={convertFileSrc(capItem.path)} controls onTimeUpdate={(e) => setNow(e.currentTarget.currentTime)} />
                    </div>
                  )}
                  {activeCue >= 0 && itemCues && tpl && (
                    <div className="cap-frame" style={{ top: videoBox.top, left: videoBox.left, width: videoBox.w, height: videoBox.h }}>
                      <CaptionView tpl={tpl} cue={itemCues[activeCue]} t={now} frameW={videoBox.w} frameH={videoBox.h} scale={scale} position={position} cueKey={activeCue} />
                    </div>
                  )}
                </div>
                <div className="cue-panel">
                  {capRun ? (
                    <div className="cap-progress" aria-live="polite">
                      <div className="bar"><span style={{ width: `${capRun.percent}%` }} /></div>
                      <p>{capRun.percent < 1 ? capRun.latest || "Listening…" : `${capRun.percent.toFixed(0)}% · ${capRun.latest}`}</p>
                      <button onClick={() => invoke("media_cancel")}>Stop</button>
                    </div>
                  ) : itemCues?.length ? (
                    <div className="cue-list" ref={cueListRef}>
                      {itemCues.map((c, i) => (
                        <div key={i} data-cue={i} className={`cue ${i === activeCue ? "on" : ""}`}>
                          <button className="cue-time" onClick={() => { if (videoRef.current) { videoRef.current.currentTime = c.start + 0.01; videoRef.current.play(); } }} title="Play from here">
                            {fmtTime(c.start, false)}
                          </button>
                          <textarea value={c.text} rows={c.text.split("\n").length} onChange={(e) => editCue(i, e.target.value)} aria-label={`Caption at ${fmtTime(c.start)}`} />
                        </div>
                      ))}
                    </div>
                  ) : (
                    <p className="cue-empty">{transcript ? "No speech found." : "Captions will appear here. Choose the engine and language on the right, then press Generate captions."}</p>
                  )}
                </div>
              </div>
            )}
          </main>

          <section className="panel" aria-label="Caption settings">
            <div className="group">
              <h2>Caption engine</h2>
              <div className="models" role="radiogroup" aria-label="Caption engine">
                {models.map((m) => (
                  <button key={m.id} role="radio" aria-checked={model === m.id} className={`model ${model === m.id ? "on" : ""}`} onClick={() => setModel(m.id)}>
                    <span className="m-top">
                      <strong>{m.label}</strong>
                      <span className="m-size">{m.downloaded ? "Ready" : `${m.size_mb} MB`}</span>
                    </span>
                    <span className="m-desc">{m.description}</span>
                  </button>
                ))}
              </div>
              {modelState && !modelState.downloaded && (
                download ? (
                  <div className="progress">
                    <div className="bar"><span style={{ width: `${(download.got / Math.max(1, download.total)) * 100}%` }} /></div>
                    <div className="row small">
                      <span>Downloading {(download.got / 1e6).toFixed(0)} of {(download.total / 1e6).toFixed(0)} MB</span>
                      <button className="link" onClick={() => invoke("media_cancel")}>Stop</button>
                    </div>
                  </div>
                ) : (
                  <>
                    <button className="wide" onClick={downloadModel}>Download {modelState.label} engine ({modelState.size_mb} MB)</button>
                    <p className="save-hint">One-time download. After that, captions work fully offline.</p>
                  </>
                )
              )}

              <label className="field">
                <span>Spoken language</span>
                <select value={language} onChange={(e) => setLanguage(e.target.value)}>
                  <option value="auto">Detect automatically</option>
                  <option value="en">English</option>
                  <option value="fr">French</option>
                  <option value="es">Spanish</option>
                  <option value="pt">Portuguese</option>
                  <option value="de">German</option>
                  <option value="it">Italian</option>
                  <option value="nl">Dutch</option>
                  <option value="ar">Arabic</option>
                  <option value="sw">Swahili</option>
                  <option value="yo">Yoruba</option>
                  <option value="ha">Hausa</option>
                  <option value="zh">Chinese</option>
                  <option value="ja">Japanese</option>
                  <option value="ko">Korean</option>
                  <option value="hi">Hindi</option>
                  <option value="ru">Russian</option>
                </select>
              </label>
              <label className="check">
                <input type="checkbox" checked={translate} onChange={(e) => setTranslate(e.target.checked)} />
                Translate the captions into English
              </label>
              <p className="save-hint">Twi, Ga and Ewe aren't supported by the speech engine yet. English (including Ghanaian English) works well.</p>
              <button className="primary wide" disabled={!capItem || !modelState?.downloaded || !!capRun} onClick={generate}>
                {capRun ? <><Spinner /> Generating…</> : transcript ? "Generate again" : "Generate captions"}
              </button>
            </div>

            <div className="group">
              <h2>Caption template</h2>
              {categories.map((cat) => (
                <div key={cat}>
                  <h3 className="tpl-cat">{cat}</h3>
                  <div className="tpl-grid">
                    {templates.filter((t) => t.category === cat).map((t) => (
                      <TemplateTile key={t.id} tpl={t} selected={t.id === tplId} onPick={() => setTplId(t.id)} />
                    ))}
                  </div>
                </div>
              ))}
              <p className="tpl-desc">{tpl?.description}</p>
              <div className="field-row">
                <label className="field">
                  <span>Size: {Math.round(scale * 100)}%</span>
                  <input type="range" min={0.6} max={1.6} step={0.05} value={scale} onChange={(e) => setScale(+e.target.value)} onDoubleClick={() => setScale(1)} />
                </label>
              </div>
              <div className="field-row">
                <label className="field">
                  <span>Position</span>
                  <select value={position} onChange={(e) => setPosition(e.target.value)}>
                    <option value="">Template default</option>
                    <option value="bottom">Bottom</option>
                    <option value="lower">Lower third (clear of TikTok / Reels buttons)</option>
                    <option value="middle">Middle</option>
                    <option value="top">Top</option>
                  </select>
                </label>
              </div>
              <div className="field-row">
                <label className="field">
                  <span>Lines</span>
                  <select value={lines} onChange={(e) => setLines(+e.target.value)}>
                    <option value={0}>Template default</option>
                    <option value={1}>1 line</option>
                    <option value={2}>2 lines</option>
                  </select>
                </label>
                <label className="field">
                  <span>Words at a time</span>
                  <select value={wordsAt} onChange={(e) => setWordsAt(+e.target.value)}>
                    <option value={0}>Template default</option>
                    {[1, 2, 3, 4, 6].map((n) => <option key={n} value={n}>{n}</option>)}
                  </select>
                </label>
              </div>
              <p className="save-hint">Line length fits your video's shape automatically. Changing template, size, lines or words re-splits the captions, which resets text edits.</p>
            </div>

            <div className="group export">
              <h2>Save</h2>
              {burning ? (
                <div className="cap-progress small"><Spinner /> <span>{burning}</span></div>
              ) : (
                <>
                  <button className="primary wide" disabled={!itemCues?.length} onClick={() => saveCaptionFile("srt")}>Save captions (SRT)…</button>
                  <p className="save-hint">For Premiere (File › Import, then drag onto the timeline), CapCut, YouTube and Facebook.</p>
                  <div className="btn-row">
                    <button disabled={!itemCues?.length} onClick={() => saveCaptionFile("vtt")}>WebVTT…</button>
                    <button disabled={!itemCues?.length} onClick={() => saveCaptionFile("txt")}>Transcript…</button>
                  </div>
                  <button className="wide" disabled={!itemCues?.length || capItem?.kind !== "video"} onClick={() => intoVideo("burn")}>Burn captions into the video…</button>
                  <button className="wide quiet" disabled={!itemCues?.length || capItem?.kind !== "video"} onClick={() => intoVideo("track")}>Add as a caption track…</button>
                  <p className="save-hint">Burned-in captions are part of the picture, which is right for Instagram, TikTok and WhatsApp. A caption track can be turned on and off and is instant.</p>
                </>
              )}
            </div>
          </section>
        </>
      )}

      {dragOver && <div className="drop-veil">{tab === "sync" ? "Drop clips and audio" : tab === "edit" ? "Drop videos to edit" : "Drop videos to caption"}</div>}
      {toast && (
        <div className="toast" role="status">
          <span>{toast.text}</span>
          {toast.reveal && <button className="link dark" onClick={() => revealItemInDir(toast.reveal!)}>Show in Finder</button>}
        </div>
      )}
    </div>
  );
}

// ---------------------------------------------------------------- sync timeline
function SyncTimeline({ report, selected, onSelect }: { report: Report; selected: string | null; onSelect: (p: string) => void }) {
  // One shared timeline, laid out exactly like the exported XML.
  const pos = report.references.map((_, i) => report.positions?.[i] ?? 0);
  const refIdx = (p: string | null) => report.references.findIndex((r) => r.path === p);
  const items = report.matches
    .filter((m) => m.reference && refIdx(m.reference) >= 0 && !(m.clip === m.reference && !m.clip_info.has_video))
    .map((m) => ({ m, start: pos[refIdx(m.reference)] + m.offset, dur: m.clip_info.duration }));
  const recs = report.references
    .map((r, i) => ({ r, start: pos[i], dur: r.info.duration }))
    .filter((x) => !x.r.info.has_video);
  const all = [...items, ...recs];
  const t0 = Math.min(0, ...all.map((x) => x.start));
  const t1 = Math.max(1, ...all.map((x) => x.start + x.dur));
  const span = Math.max(1, t1 - t0);
  const x = (t: number) => `${((t - t0) / span) * 100}%`;
  const w = (d: number) => `${(d / span) * 100}%`;
  // overlapping things get their own row
  function lanesOf<T extends { start: number; dur: number }>(list: T[]) {
    const lanes: T[][] = [];
    [...list].sort((a, b) => a.start - b.start).forEach((it) => {
      const lane = lanes.find((l) => l[l.length - 1].start + l[l.length - 1].dur <= it.start + 1e-6);
      if (lane) lane.push(it);
      else lanes.push([it]);
    });
    return lanes;
  }
  const clipLanes = lanesOf(items);
  const recLanes = lanesOf(recs);
  const tick = [5, 10, 15, 30, 60, 120, 300, 600, 900, 1800, 3600].find((s) => span / s <= 10) ?? 3600;
  const ticks: number[] = [];
  for (let t = 0; t <= span; t += tick) ticks.push(t);
  const several = recs.length > 1;
  return (
    <div className="timelines">
      <section className="tl">
        <div className="tl-ruler">
          {ticks.map((t) => (
            <span key={t} style={{ left: x(t + t0) }}>{fmtDur(t)}</span>
          ))}
        </div>
        {clipLanes.map((lane, li) => (
          <div key={`v${li}`} className="tl-lane">
            {lane.map(({ m, start, dur }) => (
              <button
                key={m.clip}
                className={`tl-clip c-${m.confidence} ${selected === m.clip ? "on" : ""}`}
                style={{ left: x(start), width: w(dur) }}
                onClick={() => onSelect(m.clip)}
                title={`${baseName(m.clip)}: starts ${fmtTime(m.offset)} into ${baseName(m.reference ?? "")}`}
              >
                <Wave env={m.envelope} color="rgba(140,180,255,0.7)" />
                <span className="tl-label">▶ {baseName(m.clip)}</span>
              </button>
            ))}
          </div>
        ))}
        {recLanes.map((lane, li) => (
          <div key={`r${li}`} className="tl-lane ref">
            {lane.map(({ r, start, dur }) => (
              <div
                key={r.path}
                className="tl-clip ref"
                style={{ left: x(start), width: w(dur) }}
                title={`${r.path}${r.info.start_epoch ? `\nRecorded at ${new Date(r.info.start_epoch * 1000).toISOString().slice(11, 19)} (from the file)` : ""}`}
              >
                <Wave env={r.envelope} color="rgba(120,200,140,0.75)" />
                <span className="tl-label">♪ {baseName(r.path)}</span>
              </div>
            ))}
          </div>
        ))}
        {items.length === 0 && <p className="tl-none">No clips matched the recording.</p>}
      </section>
      {report.matches.some((m) => !m.reference) && (
        <section className="tl-unmatched">
          <h3>Couldn't match</h3>
          <ul>
            {report.matches.filter((m) => !m.reference).map((m) => (
              <li key={m.clip}>
                <strong>{baseName(m.clip)}</strong>: {CONF_TIP.none}
              </li>
            ))}
          </ul>
        </section>
      )}
      <p className="tl-help">
        Each clip sits where its sound matches the recording — this is the same single timeline you get in Premiere / DaVinci.
        Green = recorder{several ? " files, placed by the time each one started recording (files without a time follow on in order)" : ""}; blue = camera clips. A yellow edge means "check it".
      </p>
    </div>
  );
}
