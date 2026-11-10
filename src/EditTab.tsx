import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { save } from "@tauri-apps/plugin-dialog";

type MediaInfo = { path: string; duration: number; has_video: boolean; has_audio: boolean; width: number; height: number; fps_num: number; fps_den: number };
export type EditItem = { path: string; kind: "video" | "audio"; info?: MediaInfo | null };
type Seg = { start: number; end: number };
type Clip = { path: string; info: MediaInfo; keep: Seg[] | null; pauses?: Seg[]; removedPauses?: number };
type Look = { id: string; name: string; description: string };

const baseName = (p: string) => p.split(/[\\/]/).pop() ?? p;
const dirName = (p: string) => p.slice(0, Math.max(p.lastIndexOf("/"), p.lastIndexOf("\\")));
const stem = (p: string) => baseName(p).replace(/\.[^.]+$/, "");
const fmt = (t: number) => {
  const m = Math.floor(t / 60);
  const s = t - m * 60;
  return `${m}:${s.toFixed(1).padStart(4, "0")}`;
};
const keptOf = (c: Clip): Seg[] => c.keep ?? [{ start: 0, end: c.info.duration }];
const keptLen = (c: Clip) => keptOf(c).reduce((a, s) => a + (s.end - s.start), 0);

/** Remove [a,b] from a keep-list. */
function cutOut(keep: Seg[], a: number, b: number): Seg[] {
  const out: Seg[] = [];
  for (const s of keep) {
    if (b <= s.start || a >= s.end) out.push(s);
    else {
      if (a > s.start) out.push({ start: s.start, end: a });
      if (b < s.end) out.push({ start: b, end: s.end });
    }
  }
  return out.filter((s) => s.end - s.start > 0.04);
}

/** Keep only [a,b] (intersect). */
function keepOnly(keep: Seg[], a: number, b: number): Seg[] {
  return keep.map((s) => ({ start: Math.max(s.start, a), end: Math.min(s.end, b) })).filter((s) => s.end - s.start > 0.04);
}

export function EditTab({ items, setItems, toast }: { items: EditItem[]; setItems: (f: (p: EditItem[]) => EditItem[]) => void; toast: (t: string) => void }) {
  const [clips, setClips] = useState<Clip[]>([]);
  const [sel, setSel] = useState(0);
  const [inPt, setIn] = useState<number | null>(null);
  const [outPt, setOut] = useState<number | null>(null);
  const [now, setNow] = useState(0);
  const [skipCuts, setSkipCuts] = useState(true);
  const [looks, setLooks] = useState<Look[]>([]);
  const [grade, setGrade] = useState({ on: false, auto: true, look: "natural", strength: 1 });
  const [gradeFrame, setGradeFrame] = useState<string | null>(null);
  const [showGrade, setShowGrade] = useState(true);
  const [paused, setPaused] = useState(true);
  const [pauseOpts, setPauseOpts] = useState({ db: -35, min: 0.6, pad: 0.15 });
  const [finding, setFinding] = useState(false);
  const videoRef = useRef<HTMLVideoElement>(null);

  // new files from the toolbar / drops -> clips
  useEffect(() => {
    setClips((prev) => {
      const known = new Set(prev.map((c) => c.path));
      const add = items.filter((i) => i.kind === "video" && i.info && !known.has(i.path)).map((i) => ({ path: i.path, info: i.info as MediaInfo, keep: null }));
      const stillThere = prev.filter((c) => items.some((i) => i.path === c.path));
      return add.length || stillThere.length !== prev.length ? [...stillThere, ...add] : prev;
    });
  }, [items]);
  useEffect(() => {
    invoke<Look[]>("list_looks").then(setLooks).catch(() => {});
  }, []);
  useEffect(() => {
    if (sel >= clips.length) setSel(Math.max(0, clips.length - 1));
  }, [clips.length, sel]);

  const clip = clips[sel] as Clip | undefined;
  const kept = clip ? keptOf(clip) : [];
  const total = clips.reduce((a, c) => a + keptLen(c), 0);

  const setKeep = useCallback((f: (k: Seg[], c: Clip) => Seg[] | null) => {
    setClips((cs) => cs.map((c, i) => (i === sel ? { ...c, keep: f(keptOf(c), c) } : c)));
  }, [sel]);

  // Preview the edit: playback jumps over the parts that are cut.
  const onTime = () => {
    const v = videoRef.current;
    if (!v || !clip) return;
    const t = v.currentTime;
    setNow(t);
    if (skipCuts && !v.paused && clip.keep) {
      const inside = kept.some((s) => t >= s.start - 0.01 && t < s.end);
      if (!inside) {
        const next = kept.find((s) => s.start > t);
        if (next) v.currentTime = next.start;
        else v.pause();
      }
    }
  };

  // Graded preview frame (when paused, so it doesn't fight playback).
  const gradeKey = JSON.stringify(grade);
  useEffect(() => {
    if (!clip || !grade.on || !paused) {
      setGradeFrame(null);
      return;
    }
    let cancel = false;
    const t = setTimeout(() => {
      invoke<string>("edit_graded_frame", { path: clip.path, time: now, settings: { auto: grade.auto, look: grade.look, strength: grade.strength } })
        .then((u) => !cancel && setGradeFrame(u))
        .catch(() => {});
    }, 180);
    return () => {
      cancel = true;
      clearTimeout(t);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [clip?.path, gradeKey, paused, Math.round(now * 4)]);

  // I / O / X keys like an editor
  useEffect(() => {
    const k = (e: KeyboardEvent) => {
      if ((e.target as HTMLElement).tagName === "INPUT" && (e.target as HTMLInputElement).type !== "range") return;
      if (e.metaKey || e.ctrlKey) return;
      const v = videoRef.current;
      if (!v || (document.querySelector(".edit-stage") as HTMLElement | null)?.offsetParent == null) return;
      if (e.key === "i") setIn(v.currentTime);
      if (e.key === "o") setOut(v.currentTime);
      if (e.key === " ") {
        e.preventDefault();
        v.paused ? v.play() : v.pause();
      }
    };
    window.addEventListener("keydown", k);
    return () => window.removeEventListener("keydown", k);
  }, []);

  const sel2 = inPt !== null && outPt !== null && outPt > inPt ? { a: inPt, b: outPt } : null;

  const findPauses = async (all: boolean) => {
    const targets = all ? clips.map((_, i) => i) : [sel];
    setFinding(true);
    let removed = 0;
    let saved = 0;
    for (const i of targets) {
      const c = clips[i];
      if (!c?.info.has_audio) continue;
      try {
        const r = await invoke<{ silences: Seg[]; keep: Seg[] }>("edit_find_pauses", { path: c.path, thresholdDb: pauseOpts.db, minLen: pauseOpts.min, pad: pauseOpts.pad });
        // combine with existing cuts
        const before = keptOf(c);
        let k = before;
        for (const s of r.silences) {
          const a = s.start + pauseOpts.pad;
          const b = s.end - pauseOpts.pad;
          if (b - a >= 0.1) k = cutOut(k, a, b);
        }
        const gained = before.reduce((x, s) => x + s.end - s.start, 0) - k.reduce((x, s) => x + s.end - s.start, 0);
        removed += r.silences.length;
        saved += gained;
        setClips((cs) => cs.map((cc, j) => (j === i ? { ...cc, keep: k, pauses: r.silences } : cc)));
      } catch (e) {
        toast(`Couldn't check ${baseName(c.path)}: ${e}`);
      }
    }
    setFinding(false);
    toast(removed ? `Took out ${removed} pause${removed > 1 ? "s" : ""}, ${saved.toFixed(1)} s shorter. Press play to hear it.` : "No pauses found. Try a higher sensitivity.");
  };

  const exportVideo = async () => {
    if (!clips.length) return;
    const dest = await save({
      title: "Export the edited video",
      defaultPath: `${dirName(clips[0].path)}/${clips.length > 1 ? "Edit" : stem(clips[0].path) + "_edit"}.mp4`,
      filters: [{ name: "MP4 video", extensions: ["mp4"] }],
    });
    if (!dest) return;
    try {
      await invoke("job_edit_export", {
        clips: clips.map((c) => ({ path: c.path, info: c.info, keep: keptOf(c) })),
        grade: grade.on ? { auto: grade.auto, look: grade.look, strength: grade.strength } : null,
        dest,
      });
      toast("Exporting in the background. Follow it in Jobs (bottom left), and keep editing.");
    } catch (e) {
      toast(String(e));
    }
  };

  const move = (i: number, d: number) => {
    const j = i + d;
    if (j < 0 || j >= clips.length) return;
    setClips((cs) => {
      const n = [...cs];
      [n[i], n[j]] = [n[j], n[i]];
      return n;
    });
    setSel(j);
  };
  const remove = (i: number) => {
    const p = clips[i].path;
    setItems((it) => it.filter((x) => x.path !== p));
  };

  const dur = clip?.info.duration ?? 1;
  const pct = (t: number) => `${(t / dur) * 100}%`;
  const removedParts = useMemo(() => {
    const gaps: Seg[] = [];
    let t = 0;
    for (const s of kept) {
      if (s.start > t) gaps.push({ start: t, end: s.start });
      t = s.end;
    }
    if (clip && t < dur) gaps.push({ start: t, end: dur });
    return gaps;
  }, [clip, kept, dur]);

  const seek = (e: React.MouseEvent<HTMLDivElement>) => {
    const r = e.currentTarget.getBoundingClientRect();
    const t = ((e.clientX - r.left) / r.width) * dur;
    if (videoRef.current) videoRef.current.currentTime = Math.max(0, Math.min(dur, t));
  };

  return (
    <>
      <aside className="strip media-lists" aria-label="Clips">
        <section>
          <h2>Clips (joined in this order)</h2>
          {clips.length ? (
            <ul className="media-list edit-list">
              {clips.map((c, i) => (
                <li key={c.path} className={i === sel ? "on" : ""} onClick={() => setSel(i)} title={c.path}>
                  <span className="mi-icon" aria-hidden>{i + 1}</span>
                  <span className="mi-name">{baseName(c.path)}</span>
                  <span className="mi-dur">{fmt(keptLen(c))}</span>
                  <span className="edit-li-actions">
                    <button className="link" aria-label="Move up" disabled={i === 0} onClick={(e) => (e.stopPropagation(), move(i, -1))}>↑</button>
                    <button className="link" aria-label="Move down" disabled={i === clips.length - 1} onClick={(e) => (e.stopPropagation(), move(i, 1))}>↓</button>
                    <button className="link" aria-label="Remove" onClick={(e) => (e.stopPropagation(), remove(i))}>×</button>
                  </span>
                </li>
              ))}
            </ul>
          ) : (
            <p className="empty-note">Add one clip to trim it, or several to join them into one video.</p>
          )}
        </section>
      </aside>

      <main className="stage edit-stage">
        {clip ? (
          <div className="edit-layout">
            <div className="edit-player">
              <video
                ref={videoRef}
                key={clip.path}
                src={convertFileSrc(clip.path)}
                onTimeUpdate={onTime}
                onPlay={() => setPaused(false)}
                onPause={() => setPaused(true)}
                onSeeked={onTime}
                controls
              />
              {grade.on && showGrade && paused && gradeFrame && <img className="edit-graded" src={gradeFrame} alt="Graded preview" />}
            </div>
            <div className="edit-timeline">
              <div className="etl-bar" onClick={seek} role="slider" aria-label="Position" aria-valuemin={0} aria-valuemax={dur} aria-valuenow={now}>
                {kept.map((s, i) => <span key={`k${i}`} className="etl-keep" style={{ left: pct(s.start), width: pct(s.end - s.start) }} />)}
                {removedParts.map((s, i) => <span key={`r${i}`} className="etl-cut" style={{ left: pct(s.start), width: pct(s.end - s.start) }} />)}
                {sel2 && <span className="etl-sel" style={{ left: pct(sel2.a), width: pct(sel2.b - sel2.a) }} />}
                {inPt !== null && <span className="etl-mark" style={{ left: pct(inPt) }} />}
                {outPt !== null && <span className="etl-mark" style={{ left: pct(outPt) }} />}
                <span className="etl-head" style={{ left: pct(now) }} />
              </div>
              <div className="etl-times">
                <span>{fmt(now)}</span>
                <span>Clip {fmt(dur)} → kept {fmt(keptLen(clip))}</span>
              </div>
              <div className="etl-tools">
                <button onClick={() => videoRef.current && setIn(videoRef.current.currentTime)} title="Shortcut: I">Mark start (I)</button>
                <button onClick={() => videoRef.current && setOut(videoRef.current.currentTime)} title="Shortcut: O">Mark end (O)</button>
                <span className="sep" />
                <button disabled={!sel2} onClick={() => sel2 && (setKeep((k) => keepOnly(k, sel2.a, sel2.b)), setIn(null), setOut(null))}>Keep only this part</button>
                <button disabled={!sel2} onClick={() => sel2 && (setKeep((k) => cutOut(k, sel2.a, sel2.b)), setIn(null), setOut(null))}>Cut this part out</button>
                <span className="sep" />
                <button className="link" disabled={!clip.keep} onClick={() => (setKeep(() => null), setIn(null), setOut(null))}>Reset clip</button>
              </div>
              <label className="check small">
                <input type="checkbox" checked={skipCuts} onChange={(e) => setSkipCuts(e.target.checked)} /> Play the edit (skip the cut parts)
              </label>
            </div>
          </div>
        ) : (
          <div className="empty">
            <h1>Edit video</h1>
            <p>Trim and cut clips, take out pauses, join clips into one video and grade it with your looks.</p>
            <p className="muted">Click <strong>Add videos</strong> or drop clips here.</p>
          </div>
        )}
      </main>

      <section className="panel" aria-label="Edit settings">
        <div className="group">
          <h2>Remove pauses</h2>
          <p className="save-hint">Finds the quiet gaps between words and takes them out, keeping a little breath so speech doesn't sound chopped.</p>
          <div className="slider">
            <div className="slider-head"><label htmlFor="p-db">Sensitivity</label><output>{pauseOpts.db} dB</output></div>
            <div className="slider-track"><input id="p-db" type="range" min={-50} max={-20} value={pauseOpts.db} onChange={(e) => setPauseOpts((o) => ({ ...o, db: +e.target.value }))} /></div>
          </div>
          <div className="slider">
            <div className="slider-head"><label htmlFor="p-min">Shortest pause to cut</label><output>{pauseOpts.min.toFixed(1)} s</output></div>
            <div className="slider-track"><input id="p-min" type="range" min={3} max={20} value={pauseOpts.min * 10} onChange={(e) => setPauseOpts((o) => ({ ...o, min: +e.target.value / 10 }))} /></div>
          </div>
          <p className="save-hint">Noisy room? Move Sensitivity right (towards −20 dB). Cutting too much? Raise the shortest pause.</p>
          <div className="btn-row">
            <button disabled={!clip || finding} onClick={() => findPauses(false)}>{finding ? "Listening…" : "This clip"}</button>
            <button disabled={!clips.length || finding} onClick={() => findPauses(true)}>All clips</button>
          </div>
        </div>

        <div className="group">
          <div className="row">
            <h2>Grade</h2>
            <label className="switch">
              <span>Grade video</span>
              <input type="checkbox" role="switch" checked={grade.on} onChange={(e) => setGrade((g) => ({ ...g, on: e.target.checked }))} />
              <span className="switch-ui" aria-hidden />
            </label>
          </div>
          {grade.on && (
            <>
              <div className="look-chips">
                {looks.map((l) => (
                  <button key={l.id} className={l.id === grade.look ? "on" : ""} onClick={() => setGrade((g) => ({ ...g, look: l.id }))} title={l.description}>{l.name}</button>
                ))}
              </div>
              <div className="slider">
                <div className="slider-head"><label htmlFor="g-str">Strength</label><output>{Math.round(grade.strength * 100)}%</output></div>
                <div className="slider-track"><input id="g-str" type="range" min={0} max={200} value={Math.round(grade.strength * 100)} onChange={(e) => setGrade((g) => ({ ...g, strength: +e.target.value / 100 }))} /></div>
              </div>
              <label className="check"><input type="checkbox" checked={grade.auto} onChange={(e) => setGrade((g) => ({ ...g, auto: e.target.checked }))} /> Auto-correct exposure and colour</label>
              <label className="check"><input type="checkbox" checked={showGrade} onChange={(e) => setShowGrade(e.target.checked)} /> Show the grade on the paused frame</label>
              <p className="save-hint">One grade for the whole video, worked out from frames across all clips, so every shot matches.</p>
            </>
          )}
        </div>

        <div className="group export">
          <h2>Export</h2>
          <p className="save-hint">
            {clips.length ? `${clips.length} clip${clips.length > 1 ? "s" : ""} → one video, ${fmt(total)} long. ` : ""}
            Exports run in the background, so you can start more than one and keep working.
          </p>
          <button className="primary wide" disabled={!clips.length} onClick={exportVideo}>Export video…</button>
        </div>
      </section>
    </>
  );
}
