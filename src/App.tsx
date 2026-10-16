import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open, save } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";

type Look = { id: string; name: string; description: string };
type Photo = { path: string; name: string; thumb?: string; state?: "done" | "error"; output?: string; error?: string };
type Preview = { before: string; after: string; width: number; height: number; notes: string[] };
type Progress = { index: number; total: number; file: string; ok: boolean; output?: string; error?: string };
type Format = "jpeg" | "tiff" | "png";

const EXTENSIONS = [
  "jpg", "jpeg", "png", "tif", "tiff", "webp",
  "cr2", "cr3", "crw", "nef", "nrw", "arw", "srf", "sr2", "raf", "rw2", "rwl", "orf", "pef",
  "dng", "srw", "3fr", "fff", "iiq", "mos", "erf", "kdc", "dcr", "mrw", "x3f",
];

const baseName = (p: string) => p.split(/[\\/]/).pop() ?? p;
const stem = (p: string) => baseName(p).replace(/\.[^.]+$/, "");

export default function App() {
  const [looks, setLooks] = useState<Look[]>([]);
  const [photos, setPhotos] = useState<Photo[]>([]);
  const [selected, setSelected] = useState(0);
  const [look, setLook] = useState("natural");
  const [strength, setStrength] = useState(100);
  const [preview, setPreview] = useState<Preview | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [lookThumbs, setLookThumbs] = useState<Record<string, string>>({});
  const [split, setSplit] = useState(50);
  const [showBefore, setShowBefore] = useState(false);
  const [format, setFormat] = useState<Format>("jpeg");
  const [writeLut, setWriteLut] = useState(false);
  const [outDir, setOutDir] = useState<string | null>(null);
  const [batch, setBatch] = useState<{ done: number; total: number; running: boolean; last?: string; failed: number } | null>(null);
  const [dragOver, setDragOver] = useState(false);
  const [toast, setToast] = useState<string | null>(null);

  const current = photos[selected];
  const reqId = useRef(0);

  // ---------- adding photos
  const addPaths = useCallback(async (paths: string[]) => {
    const found = await invoke<string[]>("collect_photos", { paths });
    if (found.length === 0) {
      setToast("No supported photos there. JoshGrade reads JPEG, PNG, TIFF, WebP and camera RAW files.");
      return;
    }
    setPhotos((prev) => {
      const known = new Set(prev.map((p) => p.path));
      const fresh = found.filter((p) => !known.has(p)).map((path) => ({ path, name: baseName(path) }));
      return [...prev, ...fresh];
    });
  }, []);

  useEffect(() => {
    invoke<Look[]>("list_looks").then(setLooks);
    const un = getCurrentWebview().onDragDropEvent((e) => {
      if (e.payload.type === "over" || e.payload.type === "enter") setDragOver(true);
      else if (e.payload.type === "drop") {
        setDragOver(false);
        addPaths(e.payload.paths);
      } else setDragOver(false);
    });
    const un2 = listen<Progress>("grade-progress", (e) => {
      const p = e.payload;
      setPhotos((prev) => prev.map((ph) => (ph.path === p.file ? { ...ph, state: p.ok ? "done" : "error", output: p.output, error: p.error } : ph)));
      setBatch((b) => b && { ...b, done: p.index + 1, last: p.output ?? b.last, failed: b.failed + (p.ok ? 0 : 1) });
    });
    return () => {
      un.then((f) => f());
      un2.then((f) => f());
    };
  }, [addPaths]);

  // Load list thumbnails one at a time so the app stays responsive.
  useEffect(() => {
    const next = photos.find((p) => !p.thumb);
    if (!next) return;
    let cancelled = false;
    invoke<string>("thumbnail", { path: next.path })
      .catch(() => "")
      .then((thumb) => {
        if (!cancelled) setPhotos((prev) => prev.map((p) => (p.path === next.path ? { ...p, thumb: thumb || "none" } : p)));
      });
    return () => {
      cancelled = true;
    };
  }, [photos]);

  const pickFiles = async () => {
    const r = await open({ multiple: true, filters: [{ name: "Photos", extensions: EXTENSIONS }] });
    if (r) addPaths(Array.isArray(r) ? r : [r]);
  };
  const pickFolder = async () => {
    const r = await open({ directory: true });
    if (typeof r === "string") addPaths([r]);
  };

  // ---------- live preview
  useEffect(() => {
    if (!current) {
      setPreview(null);
      return;
    }
    const id = ++reqId.current;
    setLoading(true);
    const t = setTimeout(() => {
      invoke<Preview>("preview", { path: current.path, look, strength: strength / 100 })
        .then((p) => {
          if (id === reqId.current) {
            setPreview(p);
            setError(null);
          }
        })
        .catch((e) => id === reqId.current && setError(String(e)))
        .finally(() => id === reqId.current && setLoading(false));
    }, 90);
    return () => clearTimeout(t);
  }, [current?.path, look, strength]);

  useEffect(() => {
    if (!current) return;
    let cancelled = false;
    const t = setTimeout(() => {
      invoke<{ id: string; image: string }[]>("look_thumbs", { path: current.path, strength: strength / 100 })
        .then((list) => !cancelled && setLookThumbs(Object.fromEntries(list.map((l) => [l.id, l.image]))))
        .catch(() => {});
    }, 350);
    return () => {
      cancelled = true;
      clearTimeout(t);
    };
  }, [current?.path, strength]);

  // ---------- keyboard: ← → photos, B hold for before
  useEffect(() => {
    const down = (e: KeyboardEvent) => {
      if ((e.target as HTMLElement).tagName === "INPUT") return;
      if (e.key === "ArrowDown" || e.key === "ArrowRight") setSelected((s) => Math.min(s + 1, photos.length - 1));
      if (e.key === "ArrowUp" || e.key === "ArrowLeft") setSelected((s) => Math.max(s - 1, 0));
      if (e.key.toLowerCase() === "b") setShowBefore(true);
    };
    const up = (e: KeyboardEvent) => e.key.toLowerCase() === "b" && setShowBefore(false);
    window.addEventListener("keydown", down);
    window.addEventListener("keyup", up);
    return () => {
      window.removeEventListener("keydown", down);
      window.removeEventListener("keyup", up);
    };
  }, [photos.length]);

  // ---------- actions
  const removePhoto = (i: number) => {
    setPhotos((p) => p.filter((_, j) => j !== i));
    setSelected((s) => (i < s ? s - 1 : Math.min(s, photos.length - 2)));
  };

  const exportLut = async () => {
    if (!current) return;
    const dest = await save({ defaultPath: `${stem(current.path)}_${look}.cube`, filters: [{ name: "3D LUT", extensions: ["cube"] }] });
    if (!dest) return;
    try {
      await invoke("export_lut", { path: current.path, look, strength: strength / 100, dest });
      setToast("LUT saved. In Photoshop: Layer › New Adjustment Layer › Color Lookup › Load 3D LUT.");
    } catch (e) {
      setToast(`Couldn't save the LUT: ${e}`);
    }
  };

  const chooseOutDir = async () => {
    const r = await open({ directory: true });
    if (typeof r === "string") setOutDir(r);
  };

  const gradeAll = async () => {
    if (!photos.length) return;
    setPhotos((p) => p.map((ph) => ({ ...ph, state: undefined, output: undefined, error: undefined })));
    setBatch({ done: 0, total: photos.length, running: true, failed: 0 });
    try {
      await invoke("grade_batch", {
        req: { paths: photos.map((p) => p.path), look, strength: strength / 100, format, write_lut: writeLut, out_dir: outDir },
      });
    } catch (e) {
      setToast(String(e));
    }
    setBatch((b) => b && { ...b, running: false });
  };

  useEffect(() => {
    if (!toast) return;
    const t = setTimeout(() => setToast(null), 6000);
    return () => clearTimeout(t);
  }, [toast]);

  const lookName = useMemo(() => looks.find((l) => l.id === look)?.name ?? "", [looks, look]);

  // ---------- compare slider drag
  const stageRef = useRef<HTMLDivElement>(null);
  const [box, setBox] = useState({ w: 0, h: 0 });
  useEffect(() => {
    const el = stageRef.current;
    if (!el) return;
    const ro = new ResizeObserver(([e]) => setBox({ w: e.contentRect.width, h: e.contentRect.height }));
    ro.observe(el);
    return () => ro.disconnect();
  }, [current?.path, !!preview]);
  const fit = (w: number, h: number) => {
    const s = Math.min(box.w / w, box.h / h);
    return s > 0 && isFinite(s) ? { width: Math.floor(w * s), height: Math.floor(h * s) } : { width: 0, height: 0 };
  };
  const dragSplit = (clientX: number) => {
    const r = stageRef.current?.getBoundingClientRect();
    if (r) setSplit(Math.max(0, Math.min(100, ((clientX - r.left) / r.width) * 100)));
  };

  return (
    <div className={`app ${dragOver ? "drag-over" : ""}`}>
      <header className="titlebar" data-tauri-drag-region>
        <span className="brand" data-tauri-drag-region>JoshGrade</span>
        <div className="title-actions">
          <button onClick={pickFiles}>Add photos</button>
          <button onClick={pickFolder}>Add folder</button>
        </div>
      </header>

      {/* -------- film strip */}
      <aside className="strip" aria-label="Photos">
        {photos.length === 0 ? (
          <p className="strip-empty">Your photos will appear here.</p>
        ) : (
          <ul>
            {photos.map((p, i) => (
              <li key={p.path} className={i === selected ? "on" : ""}>
                <button className="strip-item" onClick={() => setSelected(i)} title={p.path}>
                  {p.thumb && p.thumb !== "none" ? <img src={p.thumb} alt="" /> : <span className="thumb-wait" />}
                  <span className="strip-name">{p.name}</span>
                  {p.state === "done" && <span className="badge ok" title={p.output}>Saved</span>}
                  {p.state === "error" && <span className="badge err" title={p.error}>Failed</span>}
                </button>
                <button className="remove" onClick={() => removePhoto(i)} aria-label={`Remove ${p.name}`}>×</button>
              </li>
            ))}
          </ul>
        )}
        {photos.length > 0 && <div className="strip-count">{photos.length} photo{photos.length > 1 ? "s" : ""}</div>}
      </aside>

      {/* -------- stage */}
      <main className="stage">
        {!current ? (
          <div className="empty">
            <div className="empty-frame">
              <h1>Drop photos or a folder here</h1>
              <p>JoshGrade fixes exposure, white balance, contrast and colour on its own, then applies the look you pick. Everything stays on this Mac.</p>
              <div className="empty-actions">
                <button className="primary" onClick={pickFiles}>Add photos</button>
                <button onClick={pickFolder}>Add a folder</button>
              </div>
              <p className="formats">Works with JPEG, PNG, TIFF, WebP and RAW files from Canon, Nikon, Sony, Fujifilm, Panasonic, Olympus, Pentax and DNG.</p>
            </div>
          </div>
        ) : (
          <>
            <div
              className="compare"
              ref={stageRef}
              onPointerDown={(e) => {
                (e.target as HTMLElement).setPointerCapture(e.pointerId);
                dragSplit(e.clientX);
              }}
              onPointerMove={(e) => e.buttons === 1 && dragSplit(e.clientX)}
            >
              {preview ? (
                <div className="frame" style={fit(preview.width, preview.height)}>
                  <img className="layer" src={preview.after} alt={`${current.name} graded`} draggable={false} />
                  <img
                    className="layer before"
                    src={preview.before}
                    alt={`${current.name} original`}
                    draggable={false}
                    style={{ clipPath: showBefore ? "none" : `inset(0 ${100 - split}% 0 0)` }}
                  />
                  {!showBefore && (
                    <div
                      className="divider"
                      style={{ left: `${split}%` }}
                      role="slider"
                      tabIndex={0}
                      aria-label="Before and after divider"
                      aria-valuenow={Math.round(split)}
                      onKeyDown={(e) => {
                        if (e.key === "ArrowLeft") setSplit((s) => Math.max(0, s - 5));
                        if (e.key === "ArrowRight") setSplit((s) => Math.min(100, s + 5));
                      }}
                    >
                      <span className="knob" />
                    </div>
                  )}
                  <span className="tag tag-l">Original</span>
                  {!showBefore && <span className="tag tag-r">{lookName}</span>}
                </div>
              ) : (
                <div className="developing">{error ? error : "Reading photo…"}</div>
              )}
              {loading && preview && <div className="busy" aria-live="polite">Updating</div>}
            </div>
            <div className="stage-foot">
              <span>{current.name}</span>
              <span className="hint">Drag across the photo to compare. Hold B to see the original.</span>
            </div>
          </>
        )}
      </main>

      {/* -------- controls */}
      <section className="panel" aria-label="Grade settings">
        <div className="group">
          <h2>Look</h2>
          <div className="looks">
            {looks.map((l) => (
              <button key={l.id} className={`look ${l.id === look ? "on" : ""}`} onClick={() => setLook(l.id)} title={l.description} aria-pressed={l.id === look}>
                {lookThumbs[l.id] ? <img src={lookThumbs[l.id]} alt="" /> : <span className="look-ph" />}
                <span>{l.name}</span>
              </button>
            ))}
          </div>
          <p className="look-desc">{looks.find((l) => l.id === look)?.description}</p>
        </div>

        <div className="group">
          <label className="row" htmlFor="strength">
            <h2>Strength</h2>
            <output>{strength}%</output>
          </label>
          <input id="strength" type="range" min={0} max={150} step={5} value={strength} onChange={(e) => setStrength(+e.target.value)} onDoubleClick={() => setStrength(100)} />
        </div>

        {preview && (
          <div className="group fixes">
            <h2>What JoshGrade fixed</h2>
            <ul>
              {preview.notes.map((n) => (
                <li key={n} className={n.startsWith("Note:") ? "warn" : ""}>{n.replace(/^Note: /, "")}</li>
              ))}
            </ul>
          </div>
        )}

        <div className="group export">
          <h2>Save</h2>
          <div className="seg" role="radiogroup" aria-label="File type">
            {(["jpeg", "tiff", "png"] as Format[]).map((f) => (
              <button key={f} role="radio" aria-checked={format === f} className={format === f ? "on" : ""} onClick={() => setFormat(f)}>
                {f === "jpeg" ? "JPEG" : f === "tiff" ? "TIFF 16-bit" : "PNG 16-bit"}
              </button>
            ))}
          </div>
          <div className="dest">
            <span className="dest-label">{outDir ? baseName(outDir) : "A “Graded” folder next to each photo"}</span>
            <button className="link" onClick={outDir ? () => setOutDir(null) : chooseOutDir}>{outDir ? "Reset" : "Change"}</button>
          </div>
          <label className="check">
            <input type="checkbox" checked={writeLut} onChange={(e) => setWriteLut(e.target.checked)} />
            Also save a LUT (.cube) for each photo
          </label>

          {batch && (
            <div className="progress" aria-live="polite">
              <div className="bar"><span style={{ width: `${(batch.done / batch.total) * 100}%` }} /></div>
              <div className="row small">
                <span>
                  {batch.running ? `Grading ${batch.done + 1 > batch.total ? batch.total : batch.done + 1} of ${batch.total}` : `Graded ${batch.done - batch.failed} of ${batch.total}`}
                  {batch.failed > 0 && `, ${batch.failed} failed`}
                </span>
                {batch.running ? (
                  <button className="link" onClick={() => invoke("cancel_batch")}>Stop</button>
                ) : (
                  batch.last && <button className="link" onClick={() => revealItemInDir(batch.last!)}>Show in Finder</button>
                )}
              </div>
            </div>
          )}

          <button className="primary wide" disabled={!photos.length || batch?.running} onClick={gradeAll}>
            {photos.length > 1 ? `Grade all ${photos.length} photos` : "Grade photo"}
          </button>
          <button className="wide" disabled={!current} onClick={exportLut}>Export LUT for Photoshop</button>
        </div>
      </section>

      {dragOver && <div className="drop-veil">Drop to add</div>}
      {toast && <div className="toast" role="status">{toast}</div>}
    </div>
  );
}
