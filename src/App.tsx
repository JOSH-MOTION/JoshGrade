import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open, save } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { ColorWheel, Histogram, ScopeData, bandColor } from "./Scopes";

type Look = { id: string; name: string; description: string };
type Adjust = {
  exposure: number; contrast: number; highlights: number; shadows: number;
  temp: number; tint: number; vibrance: number; saturation: number;
  hue: number[]; sat: number[]; lum: number[];
};
type Settings = { look: string; strength: number; adjust: Adjust };
type Photo = { path: string; name: string; thumb?: string; saved?: string; error?: string };
type Preview = {
  path: string; after: string; overlay?: string | null; width: number; height: number;
  notes: string[]; auto_sat: number[]; scopes_before: ScopeData; scopes_after: ScopeData;
};
type Progress = { index: number; total: number; file: string; ok: boolean; output?: string; error?: string };
type Format = "jpeg" | "tiff" | "png";

const BANDS = [
  { name: "Red", hue: 0 }, { name: "Orange", hue: 30 }, { name: "Yellow", hue: 60 }, { name: "Green", hue: 120 },
  { name: "Aqua", hue: 180 }, { name: "Blue", hue: 220 }, { name: "Purple", hue: 270 }, { name: "Magenta", hue: 320 },
];
const ZERO8 = () => [0, 0, 0, 0, 0, 0, 0, 0];
const freshAdjust = (): Adjust => ({
  exposure: 0, contrast: 0, highlights: 0, shadows: 0, temp: 0, tint: 0, vibrance: 0, saturation: 0,
  hue: ZERO8(), sat: ZERO8(), lum: ZERO8(),
});
const freshSettings = (): Settings => ({ look: "natural", strength: 1, adjust: freshAdjust() });
const clone = (s: Settings): Settings => JSON.parse(JSON.stringify(s));

const EXTENSIONS = [
  "jpg", "jpeg", "png", "tif", "tiff", "webp", "heic", "heif",
  "cr2", "cr3", "crw", "nef", "nrw", "arw", "srf", "sr2", "raf", "rw2", "rwl", "orf", "pef",
  "dng", "srw", "3fr", "fff", "iiq", "mos", "erf", "kdc", "dcr", "mrw", "x3f",
];
const FORMAT_EXT: Record<Format, string> = { jpeg: "jpg", tiff: "tif", png: "png" };
const baseName = (p: string) => p.split(/[\\/]/).pop() ?? p;
const dirName = (p: string) => p.slice(0, Math.max(p.lastIndexOf("/"), p.lastIndexOf("\\")));
const stem = (p: string) => baseName(p).replace(/\.[^.]+$/, "");
const isRaw = (p: string) => /\.(cr2|cr3|crw|nef|nrw|arw|srf|sr2|raf|rw2|rwl|orf|pef|dng|srw|3fr|fff|iiq|mos|erf|kdc|dcr|mrw|x3f)$/i.test(p);

function Slider({ label, value, onChange, min = -100, max = 100, track, suffix = "" }: {
  label: string; value: number; onChange: (v: number) => void; min?: number; max?: number; track?: string; suffix?: string;
}) {
  const id = `s-${label.replace(/\W/g, "")}`;
  return (
    <div className="slider">
      <div className="slider-head">
        <label htmlFor={id} onDoubleClick={() => onChange(min < 0 ? 0 : 100)} title="Double-click to reset">{label}</label>
        <output className={value !== 0 && min < 0 ? "changed" : ""}>{value > 0 && min < 0 ? "+" : ""}{Math.round(value)}{suffix}</output>
      </div>
      <input
        id={id}
        type="range"
        min={min}
        max={max}
        step={1}
        value={value}
        onChange={(e) => onChange(+e.target.value)}
        onDoubleClick={() => onChange(min < 0 ? 0 : 100)}
        style={track ? ({ "--track": track } as React.CSSProperties) : undefined}
        className={track ? "tracked" : ""}
      />
    </div>
  );
}

export default function App() {
  const [looks, setLooks] = useState<Look[]>([]);
  const [photos, setPhotos] = useState<Photo[]>([]);
  const [selected, setSelected] = useState(0);
  const [settingsMap, setSettingsMap] = useState<Record<string, Settings>>({});
  const [lastUsed, setLastUsed] = useState<Settings>(freshSettings);
  const [preview, setPreview] = useState<Preview | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [lookThumbs, setLookThumbs] = useState<Record<string, string>>({});
  const [split, setSplit] = useState(50);
  const [showBefore, setShowBefore] = useState(false);
  const [clipping, setClipping] = useState(false);
  const [scopeView, setScopeView] = useState<"after" | "before">("after");
  const [band, setBand] = useState(0);
  const [format, setFormat] = useState<Format>("jpeg");
  const [writeLut, setWriteLut] = useState(false);
  const [batch, setBatch] = useState<{ done: number; total: number; running: boolean; dir: string; failed: number } | null>(null);
  const [dragOver, setDragOver] = useState(false);
  const [toast, setToast] = useState<{ text: string; reveal?: string } | null>(null);
  const beforeCache = useRef<Record<string, string>>({});
  const [beforeUrl, setBeforeUrl] = useState<string | null>(null);

  // Keep the selection valid whatever gets added or removed.
  useEffect(() => {
    if (photos.length === 0) {
      if (selected !== 0) setSelected(0);
    } else if (selected >= photos.length || selected < 0) {
      setSelected(Math.max(0, Math.min(selected, photos.length - 1)));
    }
  }, [photos.length, selected]);

  const current = photos[selected] as Photo | undefined;
  const settings: Settings = (current && settingsMap[current.path]) || lastUsed;
  // A photo takes your latest settings the first time you open it, then keeps its own.
  useEffect(() => {
    if (current && !settingsMap[current.path]) setSettingsMap((m) => ({ ...m, [current.path]: clone(lastUsed) }));
  }, [current?.path]);

  const update = useCallback(
    (fn: (s: Settings) => void) => {
      if (!current) {
        const s = clone(lastUsed);
        fn(s);
        setLastUsed(s);
        return;
      }
      setSettingsMap((m) => {
        const s = clone(m[current.path] ?? lastUsed);
        fn(s);
        setLastUsed(clone(s));
        return { ...m, [current.path]: s };
      });
    },
    [current, lastUsed],
  );
  const setAdj = (key: keyof Omit<Adjust, "hue" | "sat" | "lum">) => (v: number) => update((s) => void (s.adjust[key] = v));
  const setBandAdj = (key: "hue" | "sat" | "lum") => (v: number) => update((s) => void (s.adjust[key][band] = v));

  // ---------- adding photos
  const addPaths = useCallback(
    async (paths: string[]) => {
      const { photos: found, skipped } = await invoke<{ photos: string[]; skipped: string[] }>("collect_photos", { paths });
      if (found.length === 0) {
        const what = skipped.length ? `“${baseName(skipped[0])}”${skipped.length > 1 ? ` and ${skipped.length - 1} more` : ""}` : "That";
        setToast({ text: `${what} isn't a photo JoshGrade can open. It reads JPEG, PNG, TIFF, WebP, HEIC and camera RAW (CR3, CR2, NEF, ARW, RAF, DNG…).` });
        return;
      }
      setPhotos((prev) => {
        const known = new Set(prev.map((p) => p.path));
        const fresh = found.filter((p) => !known.has(p)).map((path) => ({ path, name: baseName(path) }));
        if (prev.length === 0) setSelected(0);
        return [...prev, ...fresh];
      });
      if (skipped.length) setToast({ text: `Added ${found.length} photo${found.length > 1 ? "s" : ""}. Skipped ${skipped.length} file${skipped.length > 1 ? "s" : ""} that ${skipped.length > 1 ? "aren't photos" : "isn't a photo"}.` });
    },
    [],
  );

  useEffect(() => {
    invoke<Look[]>("list_looks").then(setLooks);
    const un = getCurrentWebview().onDragDropEvent((e) => {
      if (e.payload.type === "over" || e.payload.type === "enter") setDragOver(true);
      else if (e.payload.type === "drop") {
        setDragOver(false);
        addPaths(e.payload.paths);
      } else setDragOver(false);
    });
    const un2 = listen<Progress>("save-progress", (e) => {
      const p = e.payload;
      setPhotos((prev) => prev.map((ph) => (ph.path === p.file ? { ...ph, saved: p.output ?? undefined, error: p.error ?? undefined } : ph)));
      setBatch((b) => b && { ...b, done: p.index + 1, failed: b.failed + (p.ok ? 0 : 1) });
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
    const r = await open({ multiple: true, filters: [{ name: "Photos", extensions: [...EXTENSIONS, ...EXTENSIONS.map((e) => e.toUpperCase())] }] });
    if (r) addPaths(Array.isArray(r) ? r : [r]);
  };
  const pickFolder = async () => {
    const r = await open({ directory: true });
    if (typeof r === "string") addPaths([r]);
  };

  // ---------- live preview
  const reqId = useRef(0);
  const settingsKey = JSON.stringify(settings);
  useEffect(() => {
    if (!current) {
      setPreview(null);
      return;
    }
    const path = current.path;
    setBeforeUrl(beforeCache.current[path] ?? null);
    const id = ++reqId.current;
    setLoading(true);
    const t = setTimeout(() => {
      invoke<Preview & { before?: string | null }>("preview", { path, settings, withBefore: !beforeCache.current[path], clipping })
        .then((p) => {
          if (p.before) beforeCache.current[path] = p.before;
          if (id === reqId.current) {
            setBeforeUrl(beforeCache.current[path] ?? null);
            setPreview({ ...p, path });
            setError(null);
          }
        })
        .catch((e) => {
          if (id === reqId.current) setError(String(e));
        })
        .finally(() => id === reqId.current && setLoading(false));
    }, 70);
    return () => clearTimeout(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [current?.path, settingsKey, clipping]);

  const thumbKey = JSON.stringify({ a: settings.adjust, s: settings.strength });
  useEffect(() => {
    if (!current) return;
    let cancelled = false;
    const t = setTimeout(() => {
      invoke<{ id: string; image: string }[]>("look_thumbs", { path: current.path, settings })
        .then((list) => !cancelled && setLookThumbs(Object.fromEntries(list.map((l) => [l.id, l.image]))))
        .catch(() => {});
    }, 450);
    return () => {
      cancelled = true;
      clearTimeout(t);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [current?.path, thumbKey]);

  // ---------- keyboard: ← → photos, hold B for original, J for clipping
  useEffect(() => {
    const down = (e: KeyboardEvent) => {
      if ((e.target as HTMLElement).tagName === "INPUT" && (e.target as HTMLInputElement).type !== "range") return;
      if (e.metaKey || e.ctrlKey) return;
      if (e.key === "ArrowDown") setSelected((s) => Math.min(s + 1, photos.length - 1));
      if (e.key === "ArrowUp") setSelected((s) => Math.max(s - 1, 0));
      if (e.key.toLowerCase() === "b" && !e.repeat) setShowBefore(true);
      if (e.key.toLowerCase() === "j" && !e.repeat) setClipping((c) => !c);
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
    setSelected((s) => (i < s ? s - 1 : s));
  };

  const applyToAll = () => {
    const s = clone(settings);
    setSettingsMap(Object.fromEntries(photos.map((p) => [p.path, clone(s)])));
    setToast({ text: `These settings now apply to all ${photos.length} photos. Each one still gets its own automatic correction.` });
  };

  const resetAdjust = () => update((s) => void (s.adjust = freshAdjust()));

  const saveOne = async () => {
    if (!current) return;
    const ext = FORMAT_EXT[format];
    const dest = await save({
      title: "Save graded photo",
      defaultPath: `${dirName(current.path)}/${stem(current.path)}_graded.${ext}`,
      filters: [{ name: format.toUpperCase(), extensions: [ext] }],
    });
    if (!dest) return;
    setToast({ text: `Saving ${baseName(dest)}…` });
    try {
      const out = await invoke<string>("save_photo", { path: current.path, settings, dest, format, writeLut });
      setPhotos((prev) => prev.map((p) => (p.path === current.path ? { ...p, saved: out, error: undefined } : p)));
      setToast({ text: `Saved ${baseName(out)}`, reveal: out });
    } catch (e) {
      setToast({ text: `Couldn't save: ${e}` });
    }
  };

  const saveAll = async () => {
    if (!photos.length) return;
    const dir = await open({ directory: true, title: "Choose a folder for the graded photos", defaultPath: dirName(photos[0].path) });
    if (typeof dir !== "string") return;
    setBatch({ done: 0, total: photos.length, running: true, dir, failed: 0 });
    try {
      await invoke("save_batch", {
        req: { items: photos.map((p) => ({ path: p.path, settings: settingsMap[p.path] ?? lastUsed })), format, write_lut: writeLut, out_dir: dir },
      });
    } catch (e) {
      setToast({ text: String(e) });
    }
    setBatch((b) => b && { ...b, running: false });
  };

  const exportLut = async () => {
    if (!current) return;
    const dest = await save({ defaultPath: `${dirName(current.path)}/${stem(current.path)}_${settings.look}.cube`, filters: [{ name: "3D LUT", extensions: ["cube"] }] });
    if (!dest) return;
    try {
      await invoke("export_lut", { path: current.path, settings, dest });
      setToast({ text: "LUT saved. In Photoshop: Layer › New Adjustment Layer › Color Lookup › Load 3D LUT.", reveal: dest });
    } catch (e) {
      setToast({ text: `Couldn't save the LUT: ${e}` });
    }
  };

  useEffect(() => {
    if (!toast) return;
    const t = setTimeout(() => setToast(null), 7000);
    return () => clearTimeout(t);
  }, [toast]);

  const lookName = useMemo(() => looks.find((l) => l.id === settings.look)?.name ?? "", [looks, settings.look]);
  const showing = preview && current && preview.path === current.path ? preview : null;
  const scopes = showing ? (scopeView === "after" ? showing.scopes_after : showing.scopes_before) : null;
  const a = settings.adjust;
  const adjusted = JSON.stringify(a) !== JSON.stringify(freshAdjust());

  // ---------- compare frame sizing + divider drag
  const stageRef = useRef<HTMLDivElement>(null);
  const [box, setBox] = useState({ w: 0, h: 0 });
  useEffect(() => {
    const el = stageRef.current;
    if (!el) return;
    const ro = new ResizeObserver(([e]) => setBox({ w: e.contentRect.width, h: e.contentRect.height }));
    ro.observe(el);
    return () => ro.disconnect();
  }, [!!current]);
  const fit = (w: number, h: number) => {
    const s = Math.min(box.w / w, box.h / h);
    return s > 0 && isFinite(s) ? { width: Math.floor(w * s), height: Math.floor(h * s) } : { width: 0, height: 0 };
  };
  const dragSplit = (clientX: number) => {
    const r = stageRef.current?.querySelector(".frame")?.getBoundingClientRect();
    if (r) setSplit(Math.max(0, Math.min(100, ((clientX - r.left) / r.width) * 100)));
  };

  const autoSat = showing?.auto_sat?.[band] ?? 1;

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
                  {p.saved && <span className="badge ok" title={p.saved}>Saved</span>}
                  {p.error && <span className="badge err" title={p.error}>Failed</span>}
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
              <p>JoshGrade fixes exposure, white balance, contrast and colour on its own, then applies the look you pick. Nothing is saved until you click Save. Everything stays on this Mac.</p>
              <div className="empty-actions">
                <button className="primary" onClick={pickFiles}>Add photos</button>
                <button onClick={pickFolder}>Add a folder</button>
              </div>
              <p className="formats">Works with JPEG, PNG, TIFF, WebP, HEIC and RAW files from Canon (CR3, CR2), Nikon, Sony, Fujifilm, Panasonic, Olympus, Pentax and DNG.</p>
            </div>
          </div>
        ) : (
          <>
            <div
              className="compare"
              ref={stageRef}
              onPointerDown={(e) => {
                if (!showing) return;
                (e.target as HTMLElement).setPointerCapture(e.pointerId);
                dragSplit(e.clientX);
              }}
              onPointerMove={(e) => e.buttons === 1 && dragSplit(e.clientX)}
            >
              {showing && beforeUrl ? (
                <div className="frame" style={fit(showing.width, showing.height)}>
                  <img className="layer" src={showing.after} alt={`${current.name} graded`} draggable={false} />
                  {clipping && showing.overlay && <img className="layer overlay" src={showing.overlay} alt="" draggable={false} />}
                  <img
                    className="layer before"
                    src={beforeUrl}
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
                  {!showBefore && <span className="tag tag-r">{lookName}{adjusted ? " + your edits" : ""}</span>}
                </div>
              ) : (
                <div className="developing">
                  {error ? (
                    <>
                      <strong>Couldn't open {current.name}</strong>
                      <span>{error}</span>
                    </>
                  ) : (
                    <>
                      <span className="spinner" aria-hidden />
                      <span>{isRaw(current.path) ? "Developing RAW file…" : "Opening photo…"}</span>
                    </>
                  )}
                </div>
              )}
              {loading && showing && <div className="busy" aria-live="polite">Updating</div>}
            </div>
            <div className="stage-foot">
              <span className="foot-name">{current.name}</span>
              <span className="hint">Drag across the photo to compare. Hold B for the original, press J for clipping.</span>
              <button className={`chip toggle ${clipping ? "on" : ""}`} onClick={() => setClipping((c) => !c)} aria-pressed={clipping}>
                {clipping ? "Hide clipping" : "Show clipping"}
              </button>
            </div>
          </>
        )}
      </main>

      {/* -------- controls */}
      <section className="panel" aria-label="Grade settings">
        {/* scopes */}
        <div className="group scopes">
          <div className="row">
            <h2>Check</h2>
            <div className="seg mini" role="radiogroup" aria-label="Show measurements for">
              <button role="radio" aria-checked={scopeView === "after"} className={scopeView === "after" ? "on" : ""} onClick={() => setScopeView("after")}>Graded</button>
              <button role="radio" aria-checked={scopeView === "before"} className={scopeView === "before" ? "on" : ""} onClick={() => setScopeView("before")}>Original</button>
            </div>
          </div>
          {scopes ? (
            <>
              <Histogram data={scopes} clipping={clipping} onToggleClipping={() => setClipping((c) => !c)} />
              <div className="wheel-row">
                <ColorWheel data={scopes} selected={band} onSelect={setBand} />
                <ul className="band-list">
                  {scopes.bands.map((b, i) => (
                    <li key={b.name}>
                      <button className={`band-row ${i === band ? "on" : ""} ${b.over ? "over" : ""}`} onClick={() => setBand(i)} title={`${b.name}: ${(b.share * 100).toFixed(0)}% of the colour in this photo, ${b.saturated.toFixed(0)}% has lost detail`}>
                        <span className="dot" style={{ background: bandColor(b.hue) }} />
                        <span className="band-name">{b.name}</span>
                        <span className="band-bar"><span style={{ width: `${Math.min(100, b.share * 160)}%`, background: b.over ? "#ff5a4e" : bandColor(b.hue, 48) }} /></span>
                        {b.over && <span className="warn-mark" aria-label="too strong">!</span>}
                      </button>
                    </li>
                  ))}
                </ul>
              </div>
              {scopes.bands.some((b) => b.over) ? (
                <p className="scope-note warn">
                  {(() => {
                    const over = scopes.bands.filter((b) => b.over).map((b) => b.name);
                    const many = over.length > 1;
                    const names = many ? `${over.slice(0, -1).join(", ")} and ${over[over.length - 1]}` : over[0];
                    return scopeView === "after"
                      ? `${names} ${many ? "are" : "is"} so strong ${many ? "they're" : "it's"} losing detail. Lower ${many ? "their" : "its"} Saturation in Colour mix.`
                      : `${names} ${many ? "were" : "was"} so strong ${many ? "they were" : "it was"} losing detail. JoshGrade toned ${many ? "them" : "it"} down.`;
                  })()}
                </p>
              ) : (
                <p className="scope-note">No colour is too strong.</p>
              )}
            </>
          ) : (
            <p className="scope-empty">Add a photo to see its exposure and colour.</p>
          )}
        </div>

        {/* looks */}
        <div className="group">
          <h2>Look</h2>
          <div className="looks">
            {looks.map((l) => (
              <button key={l.id} className={`look ${l.id === settings.look ? "on" : ""}`} onClick={() => update((s) => void (s.look = l.id))} title={l.description} aria-pressed={l.id === settings.look}>
                {lookThumbs[l.id] ? <img src={lookThumbs[l.id]} alt="" /> : <span className="look-ph" />}
                <span>{l.name}</span>
              </button>
            ))}
          </div>
          <p className="look-desc">{looks.find((l) => l.id === settings.look)?.description}</p>
          <Slider label="Strength" value={Math.round(settings.strength * 100)} min={0} max={200} suffix="%" onChange={(v) => update((s) => void (s.strength = v / 100))} />
        </div>

        {/* adjustments */}
        <div className="group">
          <div className="row">
            <h2>Light</h2>
            {adjusted && <button className="link" onClick={resetAdjust}>Reset all edits</button>}
          </div>
          <Slider label="Exposure" value={a.exposure} onChange={setAdj("exposure")} />
          <Slider label="Contrast" value={a.contrast} onChange={setAdj("contrast")} />
          <Slider label="Highlights" value={a.highlights} onChange={setAdj("highlights")} />
          <Slider label="Shadows" value={a.shadows} onChange={setAdj("shadows")} />
          <h2 className="sub">Colour</h2>
          <Slider label="Temperature" value={a.temp} onChange={setAdj("temp")} track="linear-gradient(90deg,#4a7fd6,#9a9a9a,#e0b040)" />
          <Slider label="Tint" value={a.tint} onChange={setAdj("tint")} track="linear-gradient(90deg,#4fb35a,#9a9a9a,#c450b8)" />
          <Slider label="Vibrance" value={a.vibrance} onChange={setAdj("vibrance")} />
          <Slider label="Saturation" value={a.saturation} onChange={setAdj("saturation")} />
        </div>

        <div className="group">
          <h2>Colour mix</h2>
          <div className="swatches" role="radiogroup" aria-label="Colour">
            {BANDS.map((b, i) => (
              <button
                key={b.name}
                role="radio"
                aria-checked={i === band}
                aria-label={b.name}
                title={b.name}
                className={`swatch ${i === band ? "on" : ""} ${scopes?.bands[i]?.over ? "over" : ""}`}
                style={{ background: bandColor(b.hue) }}
                onClick={() => setBand(i)}
              />
            ))}
          </div>
          <p className="band-title">
            {BANDS[band].name}
            {autoSat < 0.99 && <span className="auto-note">Auto toned down {Math.round((1 - autoSat) * 100)}%</span>}
          </p>
          <Slider label="Hue" value={a.hue[band]} onChange={setBandAdj("hue")} track={`linear-gradient(90deg,hsl(${BANDS[band].hue - 30},80%,50%),hsl(${BANDS[band].hue},80%,50%),hsl(${BANDS[band].hue + 30},80%,50%))`} />
          <Slider label="Saturation" value={a.sat[band]} onChange={setBandAdj("sat")} track={`linear-gradient(90deg,#8a8a8a,${bandColor(BANDS[band].hue)})`} />
          <Slider label="Luminance" value={a.lum[band]} onChange={setBandAdj("lum")} track={`linear-gradient(90deg,hsl(${BANDS[band].hue},70%,18%),hsl(${BANDS[band].hue},70%,78%))`} />
        </div>

        {showing && (
          <div className="group fixes">
            <h2>What JoshGrade fixed</h2>
            <ul>
              {showing.notes.map((n) => (
                <li key={n} className={n.startsWith("Note:") ? "warn" : ""}>{n.replace(/^Note: /, "")}</li>
              ))}
            </ul>
          </div>
        )}

        {/* saving */}
        <div className="group export">
          <h2>Save</h2>
          <p className="save-hint">Nothing is saved until you click a button below. Your originals are never changed.</p>
          <div className="seg" role="radiogroup" aria-label="File type">
            {(["jpeg", "tiff", "png"] as Format[]).map((f) => (
              <button key={f} role="radio" aria-checked={format === f} className={format === f ? "on" : ""} onClick={() => setFormat(f)}>
                {f === "jpeg" ? "JPEG" : f === "tiff" ? "TIFF 16-bit" : "PNG 16-bit"}
              </button>
            ))}
          </div>
          <label className="check">
            <input type="checkbox" checked={writeLut} onChange={(e) => setWriteLut(e.target.checked)} />
            Also save a LUT (.cube) with each photo
          </label>

          {batch && (
            <div className="progress" aria-live="polite">
              <div className="bar"><span style={{ width: `${(batch.done / batch.total) * 100}%` }} /></div>
              <div className="row small">
                <span>
                  {batch.running ? `Saving ${Math.min(batch.done + 1, batch.total)} of ${batch.total}` : `Saved ${batch.done - batch.failed} of ${batch.total}`}
                  {batch.failed > 0 && `, ${batch.failed} failed`}
                </span>
                {batch.running ? (
                  <button className="link" onClick={() => invoke("cancel_batch")}>Stop</button>
                ) : (
                  <button className="link" onClick={() => revealItemInDir(batch.dir)}>Show in Finder</button>
                )}
              </div>
            </div>
          )}

          <button className="primary wide" disabled={!current || !showing} onClick={saveOne}>Save this photo…</button>
          {photos.length > 1 && (
            <>
              <button className="wide" disabled={batch?.running} onClick={saveAll}>Save all {photos.length} photos…</button>
              <button className="wide quiet" onClick={applyToAll}>Use these settings for all photos</button>
            </>
          )}
          <button className="wide quiet" disabled={!current || !showing} onClick={exportLut}>Export LUT for Photoshop</button>
        </div>
      </section>

      {dragOver && <div className="drop-veil">Drop to add</div>}
      {toast && (
        <div className="toast" role="status">
          <span>{toast.text}</span>
          {toast.reveal && <button className="link dark" onClick={() => revealItemInDir(toast.reveal!)}>Show in Finder</button>}
        </div>
      )}
    </div>
  );
}
