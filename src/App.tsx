import { ReactNode, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open, save } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { ColorWheel, Histogram, ScopeData, bandColor } from "./Scopes";
import { Tip } from "./Tip";
import { Crop, cropActive, cropToAspect, fitCrop, freshCrop, Viewer } from "./Viewer";

type Look = { id: string; name: string; description: string };
type Adjust = {
  exposure: number; contrast: number; highlights: number; shadows: number;
  temp: number; tint: number; vibrance: number; saturation: number;
  hue: number[]; sat: number[]; lum: number[];
};
type Retouch = { enabled: boolean; blemishes: number; smooth: number; even: number; shine: number; keep_moles: boolean; keep_freckles: boolean };
type Settings = { auto: boolean; look: string; strength: number; adjust: Adjust; retouch: Retouch; crop: Crop };
type ReportLine = { key: string; label: string; before: string; after: string; verdict: string; status: "good" | "warn" | "info" };
type Photo = { path: string; name: string; thumb?: string; saved?: string; error?: string };
type Preview = {
  path: string; after: string; overlay?: string | null; width: number; height: number;
  report: ReportLine[]; auto_adjust: Adjust; scopes_before: ScopeData; scopes_after: ScopeData; full_width: number; full_height: number;
  retouch?: { faces: number } | null;
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
const freshRetouch = (): Retouch => ({ enabled: false, blemishes: 1, smooth: 0.45, even: 0.5, shine: 0.35, keep_moles: true, keep_freckles: true });
const freshSettings = (): Settings => ({ auto: true, look: "natural", strength: 1, adjust: freshAdjust(), retouch: freshRetouch(), crop: freshCrop() });
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

const ASPECTS: [string, string][] = [["free", "Free"], ["orig", "Original"], ["1:1", "1:1"], ["4:5", "4:5"], ["5:4", "5:4"], ["2:3", "2:3"], ["3:2", "3:2"], ["16:9", "16:9"], ["9:16", "9:16"]];
function aspectValue(id: string, p: { width: number; height: number; full_width?: number; full_height?: number }): number | null {
  if (id === "free") return null;
  if (id === "orig") return p.width / p.height;
  const [a, b] = id.split(":").map(Number);
  return a / b;
}

/** 0–100% slider for retouch amounts (double-click = default). */
function Amount({ label, tip, value, def, onChange }: { label: string; tip: string; value: number; def: number; onChange: (v: number) => void }) {
  const id = `r-${tip}`;
  return (
    <div className="slider">
      <div className="slider-head">
        <label htmlFor={id} onDoubleClick={() => onChange(def)} title="Double-click for the default">
          <Tip k={tip}>{label}</Tip>
        </label>
        <output className={Math.abs(value - def) > 0.005 ? "changed" : ""}>{Math.round(value * 100)}%</output>
      </div>
      <div className="slider-track">
        <input id={id} type="range" min={0} max={100} step={1} value={Math.round(value * 100)} onChange={(e) => onChange(+e.target.value / 100)} onDoubleClick={() => onChange(def)} />
      </div>
    </div>
  );
}

/** A slider that shows auto + your change. `auto` is what JoshGrade chose; moving the slider
 *  stores only your change on top, so the auto grade is never lost. */
function Slider({ label, tip, value, auto = 0, onChange, min = -100, max = 100, track, suffix = "", step = 1 }: {
  label: string; tip?: string; value: number; auto?: number; onChange: (offset: number) => void;
  min?: number; max?: number; track?: string; suffix?: string; step?: number;
}) {
  const id = `s-${label.replace(/\W/g, "")}-${tip ?? ""}`;
  const shown = Math.max(min, Math.min(max, auto + value));
  const reset = () => onChange(min < 0 ? 0 : 100 - auto);
  const autoPos = ((Math.max(min, Math.min(max, auto)) - min) / (max - min)) * 100;
  const fmt = (v: number) => `${v > 0 && min < 0 ? "+" : ""}${step < 1 ? v.toFixed(1) : Math.round(v)}${suffix}`;
  return (
    <div className="slider">
      <div className="slider-head">
        <label htmlFor={id} onDoubleClick={reset} title="Double-click to go back to auto">
          {tip ? <Tip k={tip}>{label}</Tip> : label}
        </label>
        <output className={value !== 0 ? "changed" : auto !== 0 ? "auto" : ""}>
          {fmt(shown)}
          {auto !== 0 && value === 0 && <span className="auto-tag">auto</span>}
          {value !== 0 && auto !== 0 && <span className="auto-tag">auto {fmt(auto)}</span>}
        </output>
      </div>
      <div className="slider-track">
        <input
          id={id}
          type="range"
          min={min}
          max={max}
          step={step}
          value={shown}
          onChange={(e) => onChange(+e.target.value - auto)}
          onDoubleClick={reset}
          style={track ? ({ "--track": track } as React.CSSProperties) : undefined}
          className={track ? "tracked" : ""}
        />
        {auto !== 0 && <span className="auto-mark" style={{ left: `calc(7px + (100% - 14px) * ${autoPos / 100})` }} title={`Auto: ${fmt(auto)}`} />}
      </div>
    </div>
  );
}

export default function App({ modeSwitch, active = true }: { modeSwitch?: ReactNode; active?: boolean }) {
  // Only the visible workspace reacts to drops and shortcuts.
  const activeRef = useRef(active);
  activeRef.current = active;
  const [looks, setLooks] = useState<Look[]>([]);
  const [photos, setPhotos] = useState<Photo[]>([]);
  const [selected, setSelected] = useState(0);
  const [settingsMap, setSettingsMap] = useState<Record<string, Settings>>({});
  const [multi, setMulti] = useState<Set<string>>(new Set());
  const [lastUsed, setLastUsed] = useState<Settings>(freshSettings);
  const [preview, setPreview] = useState<Preview | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [lookThumbs, setLookThumbs] = useState<Record<string, string>>({});
  const [split, setSplit] = useState(50);
  const [cropMode, setCropMode] = useState(false);
  const [aspect, setAspect] = useState<string>("free");
  const [zoomCmd, setZoomCmd] = useState<{ n: number; to: "fit" | "100" | "in" | "out" } | null>(null);
  const [zoomInfo, setZoomInfo] = useState({ z: 1, z100: 1 });
  const zoomTo = (to: "fit" | "100" | "in" | "out") => setZoomCmd((c) => ({ n: (c?.n ?? 0) + 1, to }));
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
  const stored: Settings = (current && settingsMap[current.path]) || lastUsed;
  const settings: Settings = { ...stored, retouch: stored.retouch ?? freshRetouch(), crop: stored.crop ?? freshCrop() };
  // Retouch engine: the face map model is downloaded once, then everything runs offline.
  const [rModel, setRModel] = useState<{ ready: boolean; size_mb: number } | null>(null);
  const [rDl, setRDl] = useState<{ got: number; total: number } | null>(null);
  const [rErr, setRErr] = useState<string | null>(null);
  useEffect(() => {
    invoke<{ ready: boolean; size_mb: number }>("retouch_model").then(setRModel).catch(() => {});
    const un = listen<{ got: number; total: number }>("retouch-download", (e) => setRDl(e.payload));
    return () => void un.then((f) => f());
  }, []);
  // What actually gets rendered/saved: retouch only once its engine is there.
  const eff = (s: Settings): Settings => (s.retouch?.enabled && !rModel?.ready ? { ...s, retouch: { ...s.retouch, enabled: false } } : s);
  const downloadRetouch = async () => {
    setRErr(null);
    setRDl({ got: 0, total: (rModel?.size_mb ?? 53) * 1e6 });
    try {
      await invoke("retouch_download");
      setRModel((m) => ({ size_mb: m?.size_mb ?? 53, ready: true }));
    } catch (e) {
      setRErr(String(e));
    }
    setRDl(null);
  };
  // A photo takes your latest settings the first time you open it, then keeps its own.
  useEffect(() => {
    if (current && !settingsMap[current.path]) setSettingsMap((m) => ({ ...m, [current.path]: { ...clone(lastUsed), crop: freshCrop() } }));
  }, [current?.path]);

  // ---------- history (undo / redo), per photo
  const history = useRef<Record<string, { past: Settings[]; future: Settings[]; last: number }>>({});
  const remember = (path: string, before: Settings) => {
    const h = (history.current[path] ??= { past: [], future: [], last: 0 });
    const now = Date.now();
    // a slider drag is one step: only record if the last change was a moment ago
    if (now - h.last > 700) {
      h.past.push(clone(before));
      if (h.past.length > 200) h.past.shift();
    }
    h.last = now;
    h.future = [];
  };
  const update = useCallback(
    (fn: (s: Settings) => void) => {
      if (!current) {
        const s = clone(lastUsed);
        fn(s);
        setLastUsed(s);
        return;
      }
      setSettingsMap((m) => {
        const prev = m[current.path] ?? lastUsed;
        const s = clone(prev);
        fn(s);
        if (JSON.stringify(s) === JSON.stringify(prev)) return m;
        remember(current.path, prev);
        setLastUsed({ ...clone(s), crop: freshCrop() });
        return { ...m, [current.path]: s };
      });
    },
    [current, lastUsed],
  );
  const undo = (redo = false) => {
    if (!current) return;
    const h = history.current[current.path];
    if (!h) return;
    const from = redo ? h.future : h.past;
    const to = redo ? h.past : h.future;
    const s = from.pop();
    if (!s) return;
    to.push(clone(settingsMap[current.path] ?? lastUsed));
    h.last = 0;
    setSettingsMap((m) => ({ ...m, [current.path]: s }));
  };
  const canUndo = !!(current && history.current[current.path]?.past.length);
  const canRedo = !!(current && history.current[current.path]?.future.length);

  // ---------- copy / paste settings, presets
  const [clip, setClip] = useState<Settings | null>(null);
  const copySettings = () => {
    setClip(clone(settings));
    setToast({ text: "Settings copied. Select another photo and press ⌘V (or Paste) to use them." });
  };
  const pasteSettings = (targets?: string[]) => {
    if (!clip) return;
    const paths = targets ?? (current ? [current.path] : []);
    for (const p of paths) {
      const prev = settingsMap[p] ?? lastUsed;
      remember(p, prev);
    }
    // the crop belongs to each photo; everything else is pasted
    setSettingsMap((m) => ({ ...m, ...Object.fromEntries(paths.map((p) => [p, { ...clone(clip), crop: (m[p] ?? lastUsed).crop ?? freshCrop() }])) }));
    if (paths.length > 1) setToast({ text: `Pasted onto ${paths.length} photos.` });
  };
  type Preset = { name: string; settings: Settings };
  const [presets, setPresets] = useState<Preset[]>(() => {
    try {
      return JSON.parse(localStorage.getItem("joshgrade.presets") ?? "[]");
    } catch {
      return [];
    }
  });
  useEffect(() => {
    try {
      localStorage.setItem("joshgrade.presets", JSON.stringify(presets));
    } catch {
      /* not critical */
    }
  }, [presets]);
  const [naming, setNaming] = useState<string | null>(null);
  const savePreset = () => {
    const name = (naming ?? "").trim();
    if (!name) return;
    const { crop: _c, ...rest } = clone(settings);
    void _c;
    setPresets((p) => [...p.filter((x) => x.name !== name), { name, settings: { ...rest, crop: freshCrop() } }]);
    setNaming(null);
    setToast({ text: `Saved preset "${name}". It's under Look › Your presets.` });
  };
  const applyPreset = (pr: Preset) => update((s) => void Object.assign(s, { ...clone(pr.settings), crop: s.crop }));
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
      if (!activeRef.current) return;
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
  const settingsKey = JSON.stringify({ ...settings, crop: { r: settings.crop.rot90, f: settings.crop.flip_h } });
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
      invoke<Preview & { before?: string | null }>("preview", { path, settings: eff(settings), withBefore: !beforeCache.current[path], clipping })
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
  }, [current?.path, settingsKey, clipping, rModel?.ready]);

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

  const keyActs = useRef({ undo: (_r: boolean) => {}, copy: () => {}, paste: () => {} });
  keyActs.current = { undo, copy: copySettings, paste: () => pasteSettings(multi.size > 1 ? [...multi] : undefined) };

  // ---------- keyboard: ← → photos, hold B for original, J for clipping
  useEffect(() => {
    const down = (e: KeyboardEvent) => {
      if (!activeRef.current) return;
      if ((e.target as HTMLElement).tagName === "INPUT" && (e.target as HTMLInputElement).type !== "range") return;
      if (e.metaKey || e.ctrlKey) {
        const k = e.key.toLowerCase();
        if (k === "z") {
          e.preventDefault();
          keyActs.current.undo(e.shiftKey);
        } else if (k === "y") {
          e.preventDefault();
          keyActs.current.undo(true);
        } else if (k === "c" && !window.getSelection()?.toString()) {
          e.preventDefault();
          keyActs.current.copy();
        } else if (k === "v") {
          e.preventDefault();
          keyActs.current.paste();
        }
        return;
      }
      if (e.key === "ArrowDown") setSelected((s) => Math.min(s + 1, photos.length - 1));
      if (e.key === "ArrowUp") setSelected((s) => Math.max(s - 1, 0));
      if (e.key.toLowerCase() === "b" && !e.repeat) setShowBefore(true);
      if (e.key.toLowerCase() === "j" && !e.repeat) setClipping((c) => !c);
      if (e.key.toLowerCase() === "c" && !e.repeat) setCropMode((c) => !c);
      if (e.key.toLowerCase() === "z" && !e.repeat) zoomTo("100");
      if (e.key === "Escape") setCropMode(false);
      if (e.key === "Enter" && (e.target as HTMLElement).tagName !== "BUTTON") setCropMode(false);
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

  const applyToAll = (paths?: string[]) => {
    const s = clone(settings);
    const targets = paths ?? photos.map((p) => p.path);
    for (const p of targets) if (settingsMap[p]) remember(p, settingsMap[p]);
    // each photo keeps its own crop
    setSettingsMap((m) => ({ ...m, ...Object.fromEntries(targets.map((p) => [p, { ...clone(s), crop: m[p]?.crop ?? freshCrop() }])) }));
    setToast({ text: `These settings now apply to ${paths ? targets.length : `all ${targets.length}`} photos. Each one still gets its own automatic correction.` });
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
      const out = await invoke<string>("save_photo", { path: current.path, settings: eff(settings), dest, format, writeLut });
      setPhotos((prev) => prev.map((p) => (p.path === current.path ? { ...p, saved: out, error: undefined } : p)));
      setToast({ text: `Saved ${baseName(out)}`, reveal: out });
    } catch (e) {
      setToast({ text: `Couldn't save: ${e}` });
    }
  };

  const toPhotoshop = async () => {
    if (!current) return;
    setToast({ text: "Preparing a full-quality 16-bit TIFF for Photoshop…" });
    try {
      const out = await invoke<string>("open_in_photoshop", { path: current.path, settings: eff(settings) });
      setToast({ text: `Opened ${baseName(out)} in Photoshop.`, reveal: out });
    } catch (e) {
      setToast({ text: String(e) });
    }
  };

  const saveAll = async () => {
    if (!photos.length) return;
    const dir = await open({ directory: true, title: "Choose a folder for the graded photos", defaultPath: dirName(photos[0].path) });
    if (typeof dir !== "string") return;
    setBatch({ done: 0, total: photos.length, running: true, dir, failed: 0 });
    try {
      await invoke("save_batch", {
        req: { items: photos.map((p) => ({ path: p.path, settings: eff(settingsMap[p.path] ?? lastUsed) })), format, write_lut: writeLut, out_dir: dir },
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
  const auto: Adjust = showing?.auto_adjust ?? freshAdjust();

  return (
    <div className={`app ${dragOver ? "drag-over" : ""}`} style={active ? undefined : { display: "none" }}>
      <header className="titlebar" data-tauri-drag-region>
        <span className="brand" data-tauri-drag-region>JoshGrade</span>
        {modeSwitch}
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
                <button
                  className={`strip-item ${multi.has(p.path) ? "multi" : ""}`}
                  onClick={(e) => {
                    if (e.metaKey || e.ctrlKey) {
                      setMulti((m) => {
                        const n = new Set(m.size ? m : current ? [current.path] : []);
                        n.has(p.path) ? n.delete(p.path) : n.add(p.path);
                        return n;
                      });
                    } else if (e.shiftKey) {
                      const [a, b] = [Math.min(selected, i), Math.max(selected, i)];
                      setMulti(new Set(photos.slice(a, b + 1).map((x) => x.path)));
                    } else {
                      setMulti(new Set());
                      setSelected(i);
                    }
                  }}
                  title={`${p.path}\n⌘-click or Shift-click to select several`}
                >
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
            <div className="compare" ref={stageRef}>
              {showing && beforeUrl ? (
                <Viewer
                  after={showing.after}
                  before={beforeUrl}
                  overlay={clipping ? showing.overlay : null}
                  W={showing.width}
                  H={showing.height}
                  fullW={showing.full_width || showing.width}
                  fullH={showing.full_height || showing.height}
                  crop={settings.crop}
                  cropMode={cropMode}
                  aspect={aspectValue(aspect, showing)}
                  onCrop={(c) => update((s) => void (s.crop = c))}
                  split={split}
                  setSplit={setSplit}
                  showBefore={showBefore}
                  tagL="Original"
                  tagR={`${settings.auto ? "Auto + " : ""}${lookName}${adjusted ? " + your edits" : ""}`}
                  fetchTile={(region, maxPx, before) => invoke<string>("preview_region", { path: current.path, settings: eff(settings), region, maxPx, before })}
                  tileKey={`${current.path}|${settingsKey}`}
                  zoomCmd={zoomCmd}
                  onZoom={(z, z100) => setZoomInfo({ z, z100 })}
                />
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
              {cropMode ? (
                <span className="hint">Drag the corners or edges to crop, drag inside to move. Enter or C when done.</span>
              ) : (
                <span className="hint">{zoomInfo.z > 1.01 ? "Drag or scroll to move around. Double-click to fit." : "Drag across the photo to compare. Double-click or Z to zoom to 100%. Hold B for the original."}</span>
              )}
              {!cropMode && (
                <span className="zoom-ctl" role="group" aria-label="Zoom">
                  <button className="chip" onClick={() => zoomTo("out")} aria-label="Zoom out" disabled={zoomInfo.z <= 1.001}>−</button>
                  <button className="chip zoom-val" onClick={() => zoomTo(zoomInfo.z > 1.01 ? "fit" : "100")} title="Fit / 100%">
                    {zoomInfo.z <= 1.001 ? "Fit" : `${Math.round((zoomInfo.z / zoomInfo.z100) * 100)}%`}
                  </button>
                  <button className="chip" onClick={() => zoomTo("in")} aria-label="Zoom in">+</button>
                </span>
              )}
              <button className={`chip toggle ${cropMode ? "on" : ""}`} onClick={() => setCropMode((c) => !c)} aria-pressed={cropMode} title="Crop and straighten (C)">
                {cropMode ? "Done" : "Crop"}
              </button>
              <button className={`chip toggle ${clipping ? "on" : ""}`} onClick={() => setClipping((c) => !c)} aria-pressed={clipping}>
                {clipping ? "Hide clipping" : "Show clipping"}
              </button>
            </div>
          </>
        )}
      </main>

      {/* -------- controls */}
      <section className="panel" aria-label="Grade settings">
        <div className="group workflow">
          <div className="wf-row">
            <button className="chip" onClick={() => undo()} disabled={!canUndo} title="Undo (⌘Z)">↶ Undo</button>
            <button className="chip" onClick={() => undo(true)} disabled={!canRedo} title="Redo (⇧⌘Z)">↷ Redo</button>
            <span className="wf-sep" />
            <button className="chip" onClick={copySettings} disabled={!current} title="Copy settings (⌘C)">Copy</button>
            <button className="chip" onClick={() => pasteSettings(multi.size > 1 ? [...multi] : undefined)} disabled={!clip || !current} title="Paste settings (⌘V)">
              Paste{multi.size > 1 ? ` to ${multi.size}` : ""}
            </button>
            <span className="wf-sep" />
            <button className="chip" onClick={() => setNaming(naming === null ? "" : null)} disabled={!current} title="Save these settings as your own preset">Save preset</button>
          </div>
          {naming !== null && (
            <form className="preset-name" onSubmit={(e) => (e.preventDefault(), savePreset())}>
              <input autoFocus placeholder="Preset name, e.g. Wedding warm" value={naming} onChange={(e) => setNaming(e.target.value)} onKeyDown={(e) => e.key === "Escape" && setNaming(null)} />
              <button className="primary" type="submit" disabled={!naming.trim()}>Save</button>
            </form>
          )}
        </div>

        {/* scopes */}
        <div className="group scopes">
          <div className="row">
            <h2><Tip k="histogram">Check</Tip></h2>
            <div className="seg mini" role="radiogroup" aria-label="Show measurements for">
              <button role="radio" aria-checked={scopeView === "after"} className={scopeView === "after" ? "on" : ""} onClick={() => setScopeView("after")}>Graded</button>
              <button role="radio" aria-checked={scopeView === "before"} className={scopeView === "before" ? "on" : ""} onClick={() => setScopeView("before")}>Original</button>
            </div>
          </div>
          {scopes ? (
            <>
              <Histogram data={scopes} clipping={clipping} onToggleClipping={() => setClipping((c) => !c)} />
              <h2 className="sub wheel-h"><Tip k="wheel">Colour wheel</Tip></h2>
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

        {cropMode && showing && (
          <div className="group crop-group">
            <div className="row">
              <h2>Crop &amp; straighten</h2>
              <button className="link" onClick={() => update((s) => void (s.crop = freshCrop()))} disabled={!cropActive(settings.crop)}>Reset</button>
            </div>
            <label className="field-label" htmlFor="aspect">Aspect ratio</label>
            <div className="aspects" id="aspect" role="radiogroup">
              {ASPECTS.map(([id, label]) => (
                <button key={id} className={aspect === id ? "on" : ""} aria-pressed={aspect === id}
                  onClick={() => {
                    setAspect(id);
                    update((s) => void (s.crop = cropToAspect(s.crop, showing.width, showing.height, aspectValue(id, showing))));
                  }}>{label}</button>
              ))}
            </div>
            <Slider label="Straighten" tip="c_angle" value={Math.round(settings.crop.angle * 10) / 10} min={-45} max={45} step={0.1} suffix="°"
              onChange={(v) => update((s) => void (s.crop = fitCrop({ ...s.crop, angle: v }, showing.width, showing.height)))} />
            <div className="btn-row">
              <button onClick={() => update((s) => void (s.crop = { ...freshCrop(), rot90: (s.crop.rot90 + 3) % 4, flip_h: s.crop.flip_h }))} title="Rotate left">⟲ Rotate left</button>
              <button onClick={() => update((s) => void (s.crop = { ...freshCrop(), rot90: (s.crop.rot90 + 1) % 4, flip_h: s.crop.flip_h }))} title="Rotate right">Rotate right ⟳</button>
            </div>
            <div className="btn-row">
              <button onClick={() => update((s) => void (s.crop = { ...s.crop, flip_h: !s.crop.flip_h, x: 1 - s.crop.x - s.crop.w, angle: -s.crop.angle }))}>Flip</button>
              <button className="primary" onClick={() => setCropMode(false)}>Done</button>
            </div>
          </div>
        )}

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
          {presets.length > 0 && (
            <div className="presets">
              <span className="field-label">Your presets</span>
              <div className="preset-chips">
                {presets.map((pr) => (
                  <span key={pr.name} className="preset-chip">
                    <button onClick={() => applyPreset(pr)} title="Apply this preset">{pr.name}</button>
                    <button className="x" aria-label={`Delete preset ${pr.name}`} onClick={() => setPresets((p) => p.filter((x) => x.name !== pr.name))}>×</button>
                  </span>
                ))}
              </div>
            </div>
          )}
          <Slider label="Strength" tip="s_strength" value={Math.round(settings.strength * 100)} min={0} max={200} suffix="%" onChange={(v) => update((s) => void (s.strength = v / 100))} />
        </div>

        {/* retouch */}
        <div className="group retouch">
          <div className="row">
            <h2>Retouch</h2>
            <label className="switch">
              <Tip k="r_auto" align="right"><span>Auto retouch</span></Tip>
              <input type="checkbox" role="switch" checked={settings.retouch.enabled} onChange={(e) => update((s) => void (s.retouch = { ...(s.retouch ?? freshRetouch()), enabled: e.target.checked }))} />
              <span className="switch-ui" aria-hidden />
            </label>
          </div>
          {settings.retouch.enabled && rModel && !rModel.ready && (
            <div className="r-download">
              <p>Retouch needs its face engine: a one-time {rModel.size_mb} MB download. After that it works offline.</p>
              {rDl ? (
                <div className="r-progress" role="progressbar" aria-valuenow={Math.round((rDl.got / Math.max(1, rDl.total)) * 100)}>
                  <span style={{ width: `${Math.min(100, (rDl.got / Math.max(1, rDl.total)) * 100)}%` }} />
                  <em>{Math.round(rDl.got / 1e6)} / {Math.round(rDl.total / 1e6)} MB</em>
                </div>
              ) : (
                <button className="primary" onClick={downloadRetouch}>Download face engine</button>
              )}
              {rErr && <p className="r-err">{rErr}</p>}
            </div>
          )}
          {settings.retouch.enabled && rModel?.ready && (
            <>
              <p className="r-status">
                {!showing?.retouch ? "Finding faces…" : showing.retouch.faces === 0 ? "No faces found in this photo, so nothing was retouched." : `Retouching ${showing.retouch.faces} face${showing.retouch.faces > 1 ? "s" : ""}. Only skin is touched; eyes, lips, brows, hair and beards are protected.`}
              </p>
              <Amount label="Blemishes" tip="r_blemish" value={settings.retouch.blemishes} def={1} onChange={(v) => update((s) => void (s.retouch.blemishes = v))} />
              <Amount label="Smooth skin" tip="r_smooth" value={settings.retouch.smooth} def={0.45} onChange={(v) => update((s) => void (s.retouch.smooth = v))} />
              <Amount label="Even out redness" tip="r_even" value={settings.retouch.even} def={0.5} onChange={(v) => update((s) => void (s.retouch.even = v))} />
              <Amount label="Calm shine" tip="r_shine" value={settings.retouch.shine} def={0.35} onChange={(v) => update((s) => void (s.retouch.shine = v))} />
              <label className="check"><input type="checkbox" checked={settings.retouch.keep_moles} onChange={(e) => update((s) => void (s.retouch.keep_moles = e.target.checked))} /> <Tip k="r_moles">Keep moles &amp; beauty marks</Tip></label>
              <label className="check"><input type="checkbox" checked={settings.retouch.keep_freckles} onChange={(e) => update((s) => void (s.retouch.keep_freckles = e.target.checked))} /> <Tip k="r_freckles">Keep freckles</Tip></label>
              <p className="save-hint">Hold B to compare with the original.</p>
            </>
          )}
        </div>

        {/* adjustments */}
        <div className="group">
          <div className="row">
            <h2>Light</h2>
            <label className="switch">
              <Tip k="s_auto" align="right"><span>Auto grade</span></Tip>
              <input type="checkbox" role="switch" checked={settings.auto} onChange={(e) => update((s) => void (s.auto = e.target.checked))} />
              <span className="switch-ui" aria-hidden />
            </label>
          </div>
          <Slider label="Exposure" tip="s_exposure" value={a.exposure} auto={auto.exposure} onChange={setAdj("exposure")} />
          <Slider label="Contrast" tip="s_contrast" value={a.contrast} auto={auto.contrast} onChange={setAdj("contrast")} />
          <Slider label="Highlights" tip="s_highlights" value={a.highlights} auto={auto.highlights} onChange={setAdj("highlights")} />
          <Slider label="Shadows" tip="s_shadows" value={a.shadows} auto={auto.shadows} onChange={setAdj("shadows")} />
          <h2 className="sub">Colour</h2>
          <Slider label="Temperature" tip="s_temp" value={a.temp} auto={auto.temp} onChange={setAdj("temp")} track="linear-gradient(90deg,#4a7fd6,#9a9a9a,#e0b040)" />
          <Slider label="Tint" tip="s_tint" value={a.tint} auto={auto.tint} onChange={setAdj("tint")} track="linear-gradient(90deg,#4fb35a,#9a9a9a,#c450b8)" />
          <Slider label="Vibrance" tip="s_vibrance" value={a.vibrance} auto={auto.vibrance} onChange={setAdj("vibrance")} />
          <Slider label="Saturation" tip="s_saturation" value={a.saturation} onChange={setAdj("saturation")} />
          {adjusted && <button className="link reset-link" onClick={resetAdjust}>Back to auto (undo my changes)</button>}
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
            {auto.sat[band] < 0 && <span className="auto-note">Auto toned down {Math.round(-auto.sat[band])}% (was losing detail)</span>}
          </p>
          <Slider label="Hue" tip="b_hue" value={a.hue[band]} onChange={setBandAdj("hue")} track={`linear-gradient(90deg,hsl(${BANDS[band].hue - 30},80%,50%),hsl(${BANDS[band].hue},80%,50%),hsl(${BANDS[band].hue + 30},80%,50%))`} />
          <Slider label="Saturation" tip="b_sat" value={a.sat[band]} auto={auto.sat[band]} onChange={setBandAdj("sat")} track={`linear-gradient(90deg,#8a8a8a,${bandColor(BANDS[band].hue)})`} />
          <Slider label="Luminance" tip="b_lum" value={a.lum[band]} auto={auto.lum[band]} onChange={setBandAdj("lum")} track={`linear-gradient(90deg,hsl(${BANDS[band].hue},70%,18%),hsl(${BANDS[band].hue},70%,78%))`} />
        </div>

        {showing && (
          <div className="group report">
            <h2>Report: original → now</h2>
            <p className="report-hint">Measured from your photo with the current settings. It updates every time you change something.</p>
            <ul className="report-list">
              {showing.report.map((l) => (
                <li key={l.key} className={l.status}>
                  <div className="r-top">
                    <Tip k={l.key}>{l.label}</Tip>
                    <span className="r-vals">
                      <span className="before">{l.before}</span>
                      <span className="arrow" aria-label="now">→</span>
                      <span className="after">{l.after}</span>
                    </span>
                  </div>
                  <div className="r-verdict"><span className={`dot-status ${l.status}`} aria-hidden />{l.verdict}</div>
                </li>
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
          <button className="wide" disabled={!current || !showing} onClick={toPhotoshop}>Open in Photoshop</button>
          <p className="save-hint">Sends a full-quality 16-bit TIFF (graded and retouched) to Photoshop, saved in a "JoshGrade for Photoshop" folder next to the original.</p>
          {photos.length > 1 && (
            <>
              <button className="wide" disabled={batch?.running} onClick={saveAll}>Save all {photos.length} photos…</button>
              {multi.size > 1 ? (
                <button className="wide quiet" onClick={() => applyToAll([...multi])}>Use these settings for the {multi.size} selected photos</button>
              ) : (
                <button className="wide quiet" onClick={() => applyToAll()}>Use these settings for all photos</button>
              )}
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
