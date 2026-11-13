import { ReactNode, useCallback, useEffect, useId, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open, save } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { ColorWheel, Histogram, ScopeData, bandColor } from "./Scopes";
import { Tip } from "./Tip";
import { CurveEditor, Curves, freshCurves } from "./CurveEditor";
import { Crop, cropActive, cropToAspect, fitCrop, freshCrop, Viewer } from "./Viewer";
import { type Spot, type SpotKind } from "./Spots";
import { GradeWheel, type Wheel } from "./GradeWheel";
import { Brush, KINDS, Mask, MaskAdjust, MaskKind, freshMaskAdjust, isAi, newMask } from "./Masks";

type Look = { id: string; name: string; description: string };
type Adjust = {
  exposure: number; contrast: number; highlights: number; shadows: number;
  temp: number; tint: number; vibrance: number; saturation: number; whites: number; blacks: number;
  hue: number[]; sat: number[]; lum: number[];
  grading: Grading; param: number[]; calibration: Calibration;
};
type Grading = { shadows: Wheel; midtones: Wheel; highlights: Wheel; global: Wheel; blending: number; balance: number };
type Calibration = { shadows_tint: number; red_hue: number; red_sat: number; green_hue: number; green_sat: number; blue_hue: number; blue_sat: number };
const freshWheel = (): Wheel => ({ hue: 0, sat: 0, lum: 0 });
const freshGrading = (): Grading => ({ shadows: freshWheel(), midtones: freshWheel(), highlights: freshWheel(), global: freshWheel(), blending: 50, balance: 0 });
const freshCalibration = (): Calibration => ({ shadows_tint: 0, red_hue: 0, red_sat: 0, green_hue: 0, green_sat: 0, blue_hue: 0, blue_sat: 0 });
type Detail = {
  sharpen: number; sharpen_radius: number; sharpen_masking: number; noise: number; color_noise: number;
  texture: number; clarity: number; dehaze: number; vignette: number; vignette_midpoint: number; vignette_feather: number; grain: number; grain_size: number;
};
// ---------- which parts of the settings to copy / sync (like Lightroom's Synchronize Settings)
const PARTS = [
  { id: "grade", label: "Auto grade & look", hint: "Auto on/off, the look and its strength" },
  { id: "light", label: "Light & colour", hint: "White balance, exposure, contrast, highlights, shadows, whites, blacks, colour mix" },
  { id: "curve", label: "Tone curve", hint: "" },
  { id: "detail", label: "Presence & detail", hint: "Texture, clarity, dehaze, sharpening, noise reduction" },
  { id: "effects", label: "Vignette & grain", hint: "" },
  { id: "retouch", label: "Retouch", hint: "Blemish removal and skin settings. Each photo finds its own spots." },
  { id: "aimasks", label: "Masks: Person, Sky, Background", hint: "Found again on each photo" },
  { id: "handmasks", label: "Masks: Brush, Linear, Radial", hint: "Same place on every photo" },
  { id: "lens", label: "Lens & transform", hint: "Distortion, fringes, lens vignetting, perspective" },
  { id: "spots", label: "Spot removal & red eye", hint: "Same place on every photo" },
  { id: "crop", label: "Crop & straighten", hint: "" },
] as const;
type Part = (typeof PARTS)[number]["id"];
const DEFAULT_PARTS: Part[] = ["grade", "light", "curve", "detail", "effects", "retouch", "aimasks", "lens"];
const EFFECT_KEYS = ["vignette", "vignette_midpoint", "vignette_feather", "grain", "grain_size"] as const;
/** `target` with the chosen parts taken from `src`. */
function mergeParts<S extends { auto: boolean; look: string; strength: number; adjust: unknown; curves: unknown; detail: Detail; retouch: unknown; masks: Mask[]; crop: unknown; lens: unknown; spots: unknown }>(
  target: S,
  src: S,
  parts: Part[],
): S {
  const t: S = structuredClone(target);
  const c: S = structuredClone(src);
  const has = (p: Part) => parts.includes(p);
  if (has("grade")) Object.assign(t, { auto: c.auto, look: c.look, strength: c.strength });
  if (has("light")) t.adjust = c.adjust;
  if (has("curve")) t.curves = c.curves;
  if (has("detail") || has("effects")) {
    const d = { ...t.detail };
    for (const k of Object.keys(d) as (keyof Detail)[]) {
      const fx = (EFFECT_KEYS as readonly string[]).includes(k);
      if ((fx && has("effects")) || (!fx && has("detail"))) d[k] = c.detail[k];
    }
    t.detail = d;
  }
  if (has("retouch")) t.retouch = c.retouch;
  if (has("aimasks") || has("handmasks")) {
    const pick = (m: Mask) => (isAi(m.kind) ? has("aimasks") : has("handmasks"));
    t.masks = [...c.masks.filter(pick), ...t.masks.filter((m) => !pick(m))];
  }
  if (has("lens")) t.lens = c.lens;
  if (has("spots")) t.spots = c.spots;
  if (has("crop")) t.crop = c.crop;
  return t;
}

const freshDetail = (): Detail => ({
  sharpen: 0, sharpen_radius: 1, sharpen_masking: 0, noise: 0, color_noise: 0, texture: 0, clarity: 0, dehaze: 0,
  vignette: 0, vignette_midpoint: 50, vignette_feather: 50, grain: 0, grain_size: 25,
});
type Retouch = { enabled: boolean; blemishes: number; smooth: number; even: number; shine: number; keep_moles: boolean; keep_freckles: boolean };
type Settings = { auto: boolean; look: string; strength: number; adjust: Adjust; retouch: Retouch; crop: Crop; curves: Curves; detail: Detail; masks: Mask[]; lens: Lens; spots: Spot[] };
type Lens = {
  distortion: number; vignetting: number; vignetting_mid: number; remove_ca: boolean; defringe_purple: number; defringe_green: number;
  vertical: number; horizontal: number; rotate: number; aspect: number; scale: number; x_offset: number; y_offset: number; constrain: boolean;
};
const freshLens = (): Lens => ({
  distortion: 0, vignetting: 0, vignetting_mid: 50, remove_ca: false, defringe_purple: 0, defringe_green: 0,
  vertical: 0, horizontal: 0, rotate: 0, aspect: 0, scale: 100, x_offset: 0, y_offset: 0, constrain: true,
});
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
  exposure: 0, contrast: 0, highlights: 0, shadows: 0, temp: 0, tint: 0, vibrance: 0, saturation: 0, whites: 0, blacks: 0,
  hue: ZERO8(), sat: ZERO8(), lum: ZERO8(),
  grading: freshGrading(), param: [0, 0, 0, 0], calibration: freshCalibration(),
});
const freshRetouch = (): Retouch => ({ enabled: false, blemishes: 1, smooth: 0.45, even: 0.5, shine: 0.35, keep_moles: true, keep_freckles: true });
const freshSettings = (): Settings => ({ auto: true, look: "natural", strength: 1, adjust: freshAdjust(), retouch: freshRetouch(), crop: freshCrop(), curves: freshCurves(), detail: freshDetail(), masks: [], lens: freshLens(), spots: [] });
const clone = <T,>(s: T): T => JSON.parse(JSON.stringify(s));

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
function Slider({ label, tip, value, auto = 0, onChange, min = -100, max = 100, track, suffix = "", step = 1, def }: {
  label: string; tip?: string; value: number; auto?: number; onChange: (offset: number) => void;
  min?: number; max?: number; track?: string; suffix?: string; step?: number; def?: number;
}) {
  const id = `s-${label.replace(/\W/g, "")}-${useId().replace(/:/g, "")}`;
  const shown = Math.max(min, Math.min(max, auto + value));
  const reset = () => onChange(def !== undefined ? def : min < 0 ? 0 : 100 - auto);
  const autoPos = ((Math.max(min, Math.min(max, auto)) - min) / (max - min)) * 100;
  const fmt = (v: number) => `${v > 0 && min < 0 ? "+" : ""}${step < 1 ? v.toFixed(1) : Math.round(v)}${suffix}`;
  const changed = def !== undefined ? Math.abs(value - def) > 1e-6 : value !== 0;
  return (
    <div className="slider">
      <div className="slider-head">
        <label htmlFor={id} onDoubleClick={reset} title="Double-click to go back to auto">
          {tip ? <Tip k={tip}>{label}</Tip> : label}
        </label>
        <output className={changed ? "changed" : auto !== 0 ? "auto" : ""}>
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

/** Fill in anything an older saved setting is missing. */
function normalize(stored: Settings): Settings {
  return {
    ...stored,
    adjust: {
      ...freshAdjust(),
      ...stored.adjust,
      grading: { ...freshGrading(), ...stored.adjust?.grading },
      param: stored.adjust?.param ?? [0, 0, 0, 0],
      calibration: { ...freshCalibration(), ...stored.adjust?.calibration },
    },
    retouch: stored.retouch ?? freshRetouch(),
    crop: stored.crop ?? freshCrop(),
    curves: stored.curves ?? freshCurves(),
    detail: { ...freshDetail(), ...stored.detail },
    masks: (stored.masks ?? []).map((m) => ({ ...m, adjust: { ...freshMaskAdjust(), ...m.adjust } })),
    lens: { ...freshLens(), ...stored.lens },
    spots: stored.spots ?? [],
  };
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
  const settings: Settings = normalize(stored);
  // Retouch engine: the face map model is downloaded once, then everything runs offline.
  const [rModel, setRModel] = useState<{ ready: boolean; size_mb: number } | null>(null);
  const [rDl, setRDl] = useState<{ got: number; total: number } | null>(null);
  const [rErr, setRErr] = useState<string | null>(null);
  useEffect(() => {
    invoke<{ ready: boolean; size_mb: number }>("retouch_model").then(setRModel).catch(() => {});
    const un = listen<{ got: number; total: number }>("retouch-download", (e) => setRDl(e.payload));
    return () => void un.then((f) => f());
  }, []);
  // Mask engine (person + sky): downloaded once too.
  const [mModel, setMModel] = useState<{ ready: boolean; size_mb: number } | null>(null);
  const [dlKind, setDlKind] = useState<"face" | "masks" | null>(null);
  useEffect(() => {
    invoke<{ ready: boolean; size_mb: number }>("mask_model").then(setMModel).catch(() => {});
  }, []);
  const downloadMasks = async () => {
    setRErr(null);
    setDlKind("masks");
    setRDl({ got: 0, total: (mModel?.size_mb ?? 41) * 1e6 });
    try {
      await invoke("mask_download");
      setMModel((m) => ({ size_mb: m?.size_mb ?? 41, ready: true }));
    } catch (e) {
      setRErr(String(e));
    }
    setRDl(null);
    setDlKind(null);
  };
  // What actually gets rendered/saved: retouch / AI masks only once their engine is there.
  const eff = (s: Settings): Settings => {
    let o = s;
    if (s.retouch?.enabled && !rModel?.ready) o = { ...o, retouch: { ...s.retouch, enabled: false } };
    if (!mModel?.ready && (s.masks ?? []).some((m) => isAi(m.kind))) o = { ...o, masks: (s.masks ?? []).filter((m) => !isAi(m.kind)) };
    return o;
  };
  const downloadRetouch = async () => {
    setRErr(null);
    setDlKind("face");
    setRDl({ got: 0, total: (rModel?.size_mb ?? 53) * 1e6 });
    try {
      await invoke("retouch_download");
      setRModel((m) => ({ size_mb: m?.size_mb ?? 53, ready: true }));
    } catch (e) {
      setRErr(String(e));
    }
    setRDl(null);
    setDlKind(null);
  };
  // A photo takes your latest settings the first time you open it, then keeps its own.
  useEffect(() => {
    if (current && !settingsMap[current.path]) setSettingsMap((m) => ({ ...m, [current.path]: { ...clone(lastUsed), crop: freshCrop(), masks: [] } }));
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
        const s = clone({ ...prev, adjust: { ...freshAdjust(), ...prev.adjust }, curves: prev.curves ?? freshCurves(), detail: { ...freshDetail(), ...prev.detail }, crop: prev.crop ?? freshCrop(), retouch: prev.retouch ?? freshRetouch(), masks: prev.masks ?? [] });
        fn(s);
        if (JSON.stringify(s) === JSON.stringify(prev)) return m;
        remember(current.path, prev);
        setLastUsed({ ...clone(s), crop: freshCrop(), masks: [] });
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
  // which parts Copy / Sync carry (remembered, like Lightroom's checkboxes)
  const [parts, setParts] = useState<Part[]>(() => {
    try {
      const p = JSON.parse(localStorage.getItem("joshgrade.syncParts") ?? "null");
      return Array.isArray(p) ? p : DEFAULT_PARTS;
    } catch {
      return DEFAULT_PARTS;
    }
  });
  useEffect(() => {
    try {
      localStorage.setItem("joshgrade.syncParts", JSON.stringify(parts));
    } catch {
      /* not critical */
    }
  }, [parts]);
  const [clip, setClip] = useState<{ s: Settings; parts: Part[] } | null>(null);
  // the "choose what to sync" dialog: copy = put on the clipboard; sync = apply to photos now
  const [syncDlg, setSyncDlg] = useState<null | { mode: "copy" | "sync"; scope: "selected" | "all" }>(null);
  const copySettings = (withParts: Part[] = parts) => {
    setClip({ s: clone(settings), parts: withParts });
    setToast({ text: "Settings copied. Select other photos and press ⌘V (or Paste) to use them." });
  };
  // apply the chosen parts of `src` to some photos (one undo step each)
  const applyParts = (src: Settings, withParts: Part[], paths: string[]) => {
    const targets = paths.filter((p) => p !== current?.path || src !== settings);
    for (const p of targets) remember(p, settingsMap[p] ?? lastUsed);
    setSettingsMap((m) => ({ ...m, ...Object.fromEntries(targets.map((p) => [p, mergeParts(normalize(m[p] ?? { ...clone(lastUsed), crop: freshCrop(), masks: [] }), src, withParts)])) }));
    return targets.length;
  };
  const pasteSettings = (targets?: string[]) => {
    if (!clip) return;
    const n = applyParts(clip.s, clip.parts, targets ?? (current ? [current.path] : []));
    if (n > 1) setToast({ text: `Pasted onto ${n} photos.` });
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
    // a preset keeps the AI masks (person, sky…), which fit any photo; painted / placed ones belong to this photo
    setPresets((p) => [...p.filter((x) => x.name !== name), { name, settings: { ...rest, crop: freshCrop(), masks: (rest.masks ?? []).filter((m) => isAi(m.kind)) } }]);
    setNaming(null);
    setToast({ text: `Saved preset "${name}". It's under Look › Your presets.` });
  };
  const applyPreset = (pr: Preset) => update((s) => void Object.assign(s, { ...clone(pr.settings), crop: s.crop, masks: [...(pr.settings.masks ?? []).map(clone), ...s.masks.filter((m) => !isAi(m.kind))] }));
  const setAdj = (key: keyof Omit<Adjust, "hue" | "sat" | "lum" | "grading" | "param" | "calibration">) => (v: number) => update((s) => void (s.adjust[key] = v));
  const setLens = (key: keyof Omit<Lens, "remove_ca" | "constrain">) => (v: number) => update((s) => void (s.lens[key] = v));
  const autoStraighten = async () => {
    if (!current || !showing) return;
    const a = await invoke<number | null>("auto_level", { path: current.path, settings: eff(settings) }).catch(() => null);
    if (a === null) setToast({ text: "No clear straight lines to level by in this photo." });
    else update((s) => void (s.crop = fitCrop({ ...s.crop, angle: Math.round(a * 10) / 10 }, showing.width, showing.height)));
  };
  const setCal = (key: keyof Calibration) => (v: number) => update((s) => void (s.adjust.calibration[key] = v));
  const setParam = (i: number) => (v: number) => update((s) => void (s.adjust.param[i] = v));
  const [gradeView, setGradeView] = useState<"3way" | "global">("3way");
  // white balance eyedropper: click something that should be grey
  const [picking, setPicking] = useState(false);
  const pickWb = async (x: number, y: number) => {
    setPicking(false);
    if (!current) return;
    const r = await invoke<[number, number] | null>("wb_pick", { path: current.path, settings: eff(settings), x, y }).catch(() => null);
    if (r) update((s) => void Object.assign(s.adjust, { temp: r[0], tint: r[1] }));
    else setToast({ text: "That spot is too dark or too bright to judge. Click something light grey or white that isn't blown out." });
  };
  // spot removal (heal / clone) and red eye
  const [spotTool, setSpotTool] = useState<SpotKind | null>(null);
  const [spotSize, setSpotSize] = useState(0.02);
  const [spotSel, setSpotSel] = useState<number | null>(null);
  const addSpot = async (x: number, y: number) => {
    if (!current || !spotTool) return;
    const kind = spotTool;
    const r = kind === "redeye" ? Math.min(spotSize, 0.03) : spotSize;
    let src: [number, number] = [x, y];
    if (kind !== "redeye") {
      const f = await invoke<[number, number] | null>("spot_source", { path: current.path, settings: eff(settings), x, y, r }).catch(() => null);
      src = f ?? [Math.min(1 - r, x + r * 2.5), y];
    }
    update((s) => void s.spots.push({ kind, x, y, r, sx: src[0], sy: src[1], feather: kind === "redeye" ? 0.3 : 0.5, opacity: 1 }));
    setSpotSel(settings.spots.length);
  };
  const setSpot = (key: "feather" | "opacity" | "r") => (v: number) =>
    spotSel !== null && update((s) => void (s.spots[spotSel] && (s.spots[spotSel][key] = key === "r" ? v / 1000 : v / 100)));
  const setDet = (key: keyof Detail) => (v: number) => update((s) => void (s.detail[key] = v));
  const setBandAdj = (key: "hue" | "sat" | "lum") => (v: number) => update((s) => void (s.adjust[key][band] = v));

  // ---------- layout: hide / resize / swap the side panels, fold groups (remembered)
  type Layout = { strip: boolean; panel: boolean; panelW: number; left: boolean; folded: string[] };
  const [layout, setLayout] = useState<Layout>(() => {
    const d: Layout = { strip: true, panel: true, panelW: 348, left: false, folded: [] };
    try {
      return { ...d, ...JSON.parse(localStorage.getItem("joshgrade.layout") ?? "{}") };
    } catch {
      return d;
    }
  });
  useEffect(() => {
    try {
      localStorage.setItem("joshgrade.layout", JSON.stringify(layout));
    } catch {
      /* not critical */
    }
  }, [layout]);
  const g = (cls: string, id: string) => `group ${cls}${layout.folded.includes(id) ? " folded" : ""}`;
  const fold = (id: string) => setLayout((l) => ({ ...l, folded: l.folded.includes(id) ? l.folded.filter((x) => x !== id) : [...l.folded, id] }));
  const onPanelClick = (e: React.MouseEvent) => {
    const t = e.target as HTMLElement;
    if (t.closest("button, input, label, a, select, .tip-pop")) return;
    const head = t.closest(".group[data-g] > :first-child");
    const grp = head?.parentElement;
    if (grp?.dataset.g) fold(grp.dataset.g);
  };
  const startResize = (e: React.PointerEvent) => {
    e.preventDefault();
    const x0 = e.clientX;
    const w0 = layout.panelW;
    const move = (ev: PointerEvent) => setLayout((l) => ({ ...l, panelW: Math.max(280, Math.min(620, w0 + (layout.left ? ev.clientX - x0 : x0 - ev.clientX))) }));
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };
  const cols = [layout.strip ? "184px" : "0px", "minmax(0, 1fr)", layout.panel ? `${layout.panelW}px` : "0px"];
  const gridStyle: React.CSSProperties = layout.left
    ? { gridTemplateColumns: [cols[2], cols[1], cols[0]].join(" "), gridTemplateAreas: '"top top top" "panel stage strip"' }
    : { gridTemplateColumns: cols.join(" ") };
  const layoutKeys = useRef({ tab: () => {} });
  layoutKeys.current.tab = () => setLayout((l) => (l.strip || l.panel ? { ...l, strip: false, panel: false } : { ...l, strip: true, panel: true }));

  // ---------- masks (local adjustments)
  const [maskSel, setMaskSel] = useState<number | null>(null);
  const [maskOverlay, setMaskOverlay] = useState(true);
  const [brush, setBrush] = useState<Brush>({ size: 0.04, feather: 0.6, flow: 1, erase: false });
  const [overlayUrl, setOverlayUrl] = useState<string | null>(null);
  const selMask = maskSel !== null ? settings.masks[maskSel] : undefined;
  useEffect(() => setMaskSel(null), [current?.path]);
  useEffect(() => setSpotSel(null), [current?.path]);
  const spotSelRef = useRef<number | null>(null);
  spotSelRef.current = spotTool ? spotSel : null;
  useEffect(() => {
    if (maskSel !== null && maskSel >= settings.masks.length) setMaskSel(settings.masks.length ? settings.masks.length - 1 : null);
  }, [settings.masks.length, maskSel]);
  const addMask = (kind: MaskKind) => {
    update((s) => void s.masks.push(newMask(kind, s.masks)));
    setMaskSel(settings.masks.length);
    setMaskOverlay(true);
    setCropMode(false);
  };
  const setMask = (i: number, fn: (m: Mask) => void) => update((s) => void fn(s.masks[i]));
  const setMaskAdj = (key: keyof MaskAdjust) => (v: number) => maskSel !== null && setMask(maskSel, (m) => void (m.adjust[key] = v));
  // red overlay showing where the selected mask is
  const overlayKey = selMask && maskOverlay ? JSON.stringify({ m: { ...selMask, adjust: 0, amount: 0, name: 0, enabled: 0 }, r: settings.crop.rot90, f: settings.crop.flip_h, ready: mModel?.ready }) : "";
  useEffect(() => {
    if (!overlayKey || !current || maskSel === null) {
      setOverlayUrl(null);
      return;
    }
    if (isAi(selMask!.kind) && !mModel?.ready) {
      setOverlayUrl(null);
      return;
    }
    let cancelled = false;
    const t = setTimeout(() => {
      invoke<string>("mask_overlay", { path: current.path, settings: eff(settings), index: maskSel })
        .then((u) => !cancelled && setOverlayUrl(u))
        .catch(() => !cancelled && setOverlayUrl(null));
    }, 60);
    return () => {
      cancelled = true;
      clearTimeout(t);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [overlayKey, current?.path, maskSel]);

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
  // One render at a time, always of the latest settings: while one is being made, slider
  // moves just update what the next one will be (no pile-up of stale renders).
  const reqId = useRef(0);
  const inflight = useRef(false);
  const prefetchTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const queued = useRef<null | (() => void)>(null);
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
    const run = () => {
      inflight.current = true;
      invoke<Preview & { before?: string | null }>("preview", { path, settings: eff(settings), withBefore: !beforeCache.current[path], clipping })
        .then((p) => {
          if (p.before) beforeCache.current[path] = p.before;
          if (id === reqId.current) {
            setBeforeUrl(beforeCache.current[path] ?? null);
            setPreview({ ...p, path });
            setError(null);
            // get the neighbours ready in the background, like Lightroom
            // (after a pause, so it doesn't slow down slider moves)
            if (prefetchTimer.current) clearTimeout(prefetchTimer.current);
            prefetchTimer.current = setTimeout(() => {
              const i = photos.findIndex((x) => x.path === path);
              const near = [photos[i + 1]?.path, photos[i - 1]?.path].filter(Boolean);
              if (near.length) invoke("prefetch", { paths: near }).catch(() => {});
            }, 1500);
          }
        })
        .catch((e) => {
          if (id === reqId.current) setError(String(e));
        })
        .finally(() => {
          if (id === reqId.current) setLoading(false);
          inflight.current = false;
          const next = queued.current;
          queued.current = null;
          next?.();
        });
    };
    if (prefetchTimer.current) clearTimeout(prefetchTimer.current);
    const t = setTimeout(() => {
      if (inflight.current) queued.current = run;
      else run();
    }, 16);
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
  keyActs.current = { undo, copy: () => copySettings(), paste: () => pasteSettings(multi.size > 1 ? [...multi] : undefined) };

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
      if (e.key === "Tab" && !e.shiftKey && (e.target as HTMLElement) === document.body) {
        e.preventDefault();
        layoutKeys.current.tab();
      }
      if (e.key.toLowerCase() === "o" && !e.repeat) setMaskOverlay((o) => !o);
      if (e.key === "[" ) setBrush((b) => ({ ...b, size: Math.max(0.004, b.size / 1.2) }));
      if (e.key === "]") setBrush((b) => ({ ...b, size: Math.min(0.3, b.size * 1.2) }));
      if (e.key.toLowerCase() === "w" && !e.metaKey && !e.ctrlKey) setPicking((p) => !p);
      if (e.key.toLowerCase() === "q" && !e.metaKey && !e.ctrlKey && !e.repeat) {
        setSpotTool((t) => (t ? null : "heal"));
        setMaskSel(null);
      }
      if ((e.key === "Delete" || e.key === "Backspace") && spotSelRef.current !== null) {
        const i = spotSelRef.current;
        update((s) => void s.spots.splice(i, 1));
        setSpotSel(null);
      }
      if (e.key === "Escape") {
        setPicking(false);
        setSpotTool(null);
        setSpotSel(null);
        setCropMode(false);
        setMaskSel(null);
      }
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

  const syncTo = (scope: "selected" | "all", withParts: Part[]) => {
    const paths = scope === "selected" && multi.size > 1 ? [...multi] : photos.map((p) => p.path);
    const n = applyParts(clone(settings), withParts, paths.filter((p) => p !== current?.path));
    setToast({ text: `Synced to ${n} photo${n === 1 ? "" : "s"}. Each one still gets its own automatic correction${withParts.includes("retouch") ? " and finds its own spots" : ""}. ⌘Z on a photo undoes it there.` });
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
    <div className={`app ${dragOver ? "drag-over" : ""} ${layout.left ? "panel-left" : ""} ${layout.strip ? "" : "no-strip"} ${layout.panel ? "" : "no-panel"}`} style={active ? gridStyle : { display: "none" }}>
      <header className="titlebar" data-tauri-drag-region>
        <span className="brand" data-tauri-drag-region>JoshGrade</span>
        {modeSwitch}
        <div className="title-actions">
          <span className="layout-btns" role="group" aria-label="Panels">
            <button className={`icon-btn ${layout.strip ? "on" : ""}`} onClick={() => setLayout((l) => ({ ...l, strip: !l.strip }))} aria-pressed={layout.strip} title="Show / hide the photo list">
              <svg width="18" height="14" viewBox="0 0 18 14" aria-hidden><rect x="0.5" y="0.5" width="17" height="13" rx="2" fill="none" stroke="currentColor" /><rect x={layout.left ? 12 : 1} y="1" width="5" height="12" rx="1" fill="currentColor" opacity=".8" /></svg>
            </button>
            <button className={`icon-btn ${layout.panel ? "on" : ""}`} onClick={() => setLayout((l) => ({ ...l, panel: !l.panel }))} aria-pressed={layout.panel} title="Show / hide the controls (Tab hides both panels)">
              <svg width="18" height="14" viewBox="0 0 18 14" aria-hidden><rect x="0.5" y="0.5" width="17" height="13" rx="2" fill="none" stroke="currentColor" /><rect x={layout.left ? 1 : 11} y="1" width="6" height="12" rx="1" fill="currentColor" opacity=".8" /></svg>
            </button>
            <button className="icon-btn" onClick={() => setLayout((l) => ({ ...l, left: !l.left }))} title={layout.left ? "Move the controls to the right" : "Move the controls to the left"}>
              <svg width="18" height="14" viewBox="0 0 18 14" aria-hidden><path d="M3 5h12l-3-3M15 9H3l3 3" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round" /></svg>
            </button>
          </span>
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
                  overlay={selMask && maskOverlay && overlayUrl ? overlayUrl : clipping ? showing.overlay : null}
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
                  onPick={picking ? pickWb : null}
                  spotTool={spotTool && !picking ? {
                    kind: spotTool, size: spotSize, spots: settings.spots, sel: spotSel, onAdd: addSpot, onSelect: setSpotSel,
                    onChange: (i, sp) => update((s) => void (s.spots[i] = sp)),
                  } : null}
                  maskTool={selMask && maskSel !== null ? { mask: selMask, brush, onChange: (m) => update((s) => void (s.masks[maskSel] = m)) } : null}
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
              ) : selMask ? (
                <span className="hint">
                  {selMask.kind === "brush"
                    ? "Paint on the photo. [ and ] change the brush size. O shows or hides the red mask."
                    : selMask.kind === "linear"
                      ? "Drag the outer dots to place the filter, the middle dot to move it. O shows the mask."
                      : selMask.kind === "radial"
                        ? "Drag the centre to move, the edge dots to resize and turn. O shows the mask."
                        : "Red shows where the change goes. O shows or hides it. Esc when done."}
                </span>
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
      {layout.panel && (
        <div className="panel-resize" style={layout.left ? { left: layout.panelW - 3 } : { right: layout.panelW - 3 }} onPointerDown={startResize} onDoubleClick={() => setLayout((l) => ({ ...l, panelW: 348 }))} title="Drag to resize. Double-click for the normal width." aria-hidden />
      )}
      <section className="panel" aria-label="Grade settings" onClick={onPanelClick}>
        {layout.folded.length > 0 && (
          <button className="link unfold" onClick={() => setLayout((l) => ({ ...l, folded: [] }))}>Open all sections</button>
        )}
        <div className="group workflow">
          <div className="wf-row">
            <button className="chip" onClick={() => undo()} disabled={!canUndo} title="Undo (⌘Z)">↶ Undo</button>
            <button className="chip" onClick={() => undo(true)} disabled={!canRedo} title="Redo (⇧⌘Z)">↷ Redo</button>
            <span className="wf-sep" />
            <button className="chip" onClick={() => setSyncDlg({ mode: "copy", scope: "selected" })} disabled={!current} title="Choose what to copy (⌘C copies the same parts again)">Copy…</button>
            <button className="chip" onClick={() => pasteSettings(multi.size > 1 ? [...multi] : undefined)} disabled={!clip || !current} title="Paste settings (⌘V)">
              Paste{multi.size > 1 ? ` to ${multi.size}` : ""}
            </button>
            <button className="chip" onClick={() => setSyncDlg({ mode: "sync", scope: multi.size > 1 ? "selected" : "all" })} disabled={!current || photos.length < 2} title="Apply this photo's settings to other photos, choosing which parts">
              Sync…
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
        <div className={g("scopes", "scopes")} data-g="scopes">
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
            <button className="chip" onClick={autoStraighten} title="Level the horizon / straighten by the photo's own lines">Auto straighten</button>
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
        <div className={g("", "look")} data-g="look">
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
        <div className={g("retouch", "retouch")} data-g="retouch">
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

        {/* masks */}
        <div className={g("masks", "masks")} data-g="masks">
          <div className="row">
            <h2><Tip k="m_masks">Masks</Tip></h2>
            {selMask && <button className="link" onClick={() => setMaskSel(null)}>Done</button>}
          </div>
          <div className="mask-add" role="group" aria-label="Add a mask">
            {KINDS.map((k) => (
              <button key={k.kind} className="chip" title={k.hint} onClick={() => addMask(k.kind)} disabled={!current}>
                + {k.label}
              </button>
            ))}
          </div>
          {settings.masks.some((m) => isAi(m.kind)) && mModel && !mModel.ready && (
            <div className="r-download">
              <p>Person, Sky and Background masks need their engine: a one-time {mModel.size_mb} MB download. After that they work offline.</p>
              {rDl && dlKind === "masks" ? (
                <div className="r-progress" role="progressbar" aria-valuenow={Math.round((rDl.got / Math.max(1, rDl.total)) * 100)}>
                  <span style={{ width: `${Math.min(100, (rDl.got / Math.max(1, rDl.total)) * 100)}%` }} />
                  <em>{Math.round(rDl.got / 1e6)} / {Math.round(rDl.total / 1e6)} MB</em>
                </div>
              ) : (
                <button className="primary" onClick={downloadMasks} disabled={!!rDl}>Download mask engine</button>
              )}
              {rErr && dlKind === null && <p className="r-err">{rErr}</p>}
            </div>
          )}
          {settings.masks.length > 0 && (
            <ul className="mask-list">
              {settings.masks.map((m, i) => (
                <li key={i} className={`${i === maskSel ? "on" : ""} ${m.enabled ? "" : "off"}`}>
                  <button className="mask-eye" aria-label={m.enabled ? `Hide ${m.name}` : `Show ${m.name}`} title={m.enabled ? "Turn off" : "Turn on"} onClick={() => setMask(i, (x) => void (x.enabled = !x.enabled))}>
                    {m.enabled ? "◉" : "○"}
                  </button>
                  <button className="mask-name" onClick={() => setMaskSel(i === maskSel ? null : i)} aria-pressed={i === maskSel}>
                    <span className={`mask-ico k-${m.kind}`} aria-hidden />
                    {m.name}
                    {m.invert ? " (inverted)" : ""}
                  </button>
                  <button className="x" aria-label={`Delete ${m.name}`} title="Delete" onClick={() => update((s) => void s.masks.splice(i, 1))}>×</button>
                </li>
              ))}
            </ul>
          )}
          {selMask && maskSel !== null && (
            <div className="mask-edit">
              <div className="mask-opts">
                <label className="check"><input type="checkbox" checked={maskOverlay} onChange={(e) => setMaskOverlay(e.target.checked)} /> Show mask (O)</label>
                <label className="check"><input type="checkbox" checked={selMask.invert} onChange={(e) => setMask(maskSel, (m) => void (m.invert = e.target.checked))} /> Invert</label>
                <button className="link" onClick={() => update((s) => void s.masks.splice(maskSel + 1, 0, { ...clone(s.masks[maskSel]), name: `${s.masks[maskSel].name} copy` }))}>Duplicate</button>
              </div>
              {selMask.kind === "brush" && (
                <div className="brush-opts">
                  <div className="seg mini" role="radiogroup" aria-label="Brush mode">
                    <button className={!brush.erase ? "on" : ""} aria-pressed={!brush.erase} onClick={() => setBrush((b) => ({ ...b, erase: false }))}>Paint</button>
                    <button className={brush.erase ? "on" : ""} aria-pressed={brush.erase} onClick={() => setBrush((b) => ({ ...b, erase: true }))}>Erase</button>
                  </div>
                  <Slider label="Size" tip="m_size" value={Math.round(brush.size * 1000) / 10} min={0.5} max={30} step={0.5} def={4} onChange={(v) => setBrush((b) => ({ ...b, size: v / 100 }))} />
                  <Slider label="Feather" tip="m_feather" value={Math.round(brush.feather * 100)} min={0} max={100} def={60} onChange={(v) => setBrush((b) => ({ ...b, feather: v / 100 }))} />
                  <Slider label="Flow" tip="m_flow" value={Math.round(brush.flow * 100)} min={5} max={100} def={100} onChange={(v) => setBrush((b) => ({ ...b, flow: v / 100 }))} />
                  {selMask.strokes.length > 0 && (
                    <button className="link" onClick={() => setMask(maskSel, (m) => void m.strokes.pop())}>Undo last stroke</button>
                  )}
                  {selMask.strokes.length === 0 && <p className="save-hint">Paint on the photo where you want the change.</p>}
                </div>
              )}
              {selMask.kind === "radial" && (
                <Slider label="Feather" tip="m_feather" value={Math.round(selMask.feather)} min={0} max={100} def={50} onChange={(v) => setMask(maskSel, (m) => void (m.feather = v))} />
              )}
              {isAi(selMask.kind) && mModel?.ready && <p className="save-hint">Found automatically. Edges follow hair and outlines.</p>}
              <h2 className="sub">Adjust {selMask.name}</h2>
              <Slider label="Exposure" tip="s_exposure" value={selMask.adjust.exposure} onChange={setMaskAdj("exposure")} />
              <Slider label="Contrast" tip="s_contrast" value={selMask.adjust.contrast} onChange={setMaskAdj("contrast")} />
              <Slider label="Highlights" tip="s_highlights" value={selMask.adjust.highlights} onChange={setMaskAdj("highlights")} />
              <Slider label="Shadows" tip="s_shadows" value={selMask.adjust.shadows} onChange={setMaskAdj("shadows")} />
              <Slider label="Temperature" tip="s_temp" value={selMask.adjust.temp} onChange={setMaskAdj("temp")} track="linear-gradient(90deg,#4a7fd6,#9a9a9a,#e0b040)" />
              <Slider label="Tint" tip="s_tint" value={selMask.adjust.tint} onChange={setMaskAdj("tint")} track="linear-gradient(90deg,#4fb35a,#9a9a9a,#c450b8)" />
              <Slider label="Saturation" tip="s_saturation" value={selMask.adjust.saturation} onChange={setMaskAdj("saturation")} />
              <Slider label="Clarity" tip="d_clarity" value={selMask.adjust.clarity} onChange={setMaskAdj("clarity")} />
              <Slider label="Amount" tip="m_amount" value={Math.round(selMask.amount * 100)} min={0} max={100} def={100} suffix="%" onChange={(v) => setMask(maskSel, (m) => void (m.amount = v / 100))} />
              <button className="link" onClick={() => setMask(maskSel, (m) => void (m.adjust = freshMaskAdjust()))}>Reset this mask's sliders</button>
            </div>
          )}
          {settings.masks.length === 0 && <p className="save-hint">Change one part of the photo only: brighten the person, darken the sky, warm a face.</p>}
        </div>

        {/* adjustments */}
        <div className={g("", "light")} data-g="light">
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
          <Slider label="Whites" tip="s_whites" value={a.whites} auto={auto.whites ?? 0} onChange={setAdj("whites")} />
          <Slider label="Blacks" tip="s_blacks" value={a.blacks} auto={auto.blacks ?? 0} onChange={setAdj("blacks")} />
          <h2 className="sub">Presence</h2>
          <Slider label="Texture" tip="d_texture" value={settings.detail.texture} onChange={setDet("texture")} />
          <Slider label="Clarity" tip="d_clarity" value={settings.detail.clarity} onChange={setDet("clarity")} />
          <Slider label="Dehaze" tip="d_dehaze" value={settings.detail.dehaze} onChange={setDet("dehaze")} />
          <div className="row">
            <h2 className="sub">Colour</h2>
            <button className={`chip wb-pick ${picking ? "on" : ""}`} onClick={() => setPicking((p) => !p)} aria-pressed={picking} title="White balance picker (W): click something in the photo that should be grey or white">
              <svg width="14" height="14" viewBox="0 0 16 16" aria-hidden><path d="M11 2l3 3-1.5 1.5-1-1L5 12H3v-2l6.5-6.5-1-1z" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinejoin="round" /></svg>
              {picking ? "Click a grey spot" : "Pick"}
            </button>
          </div>
          <Slider label="Temperature" tip="s_temp" value={a.temp} auto={auto.temp} onChange={setAdj("temp")} track="linear-gradient(90deg,#4a7fd6,#9a9a9a,#e0b040)" />
          <Slider label="Tint" tip="s_tint" value={a.tint} auto={auto.tint} onChange={setAdj("tint")} track="linear-gradient(90deg,#4fb35a,#9a9a9a,#c450b8)" />
          <Slider label="Vibrance" tip="s_vibrance" value={a.vibrance} auto={auto.vibrance} onChange={setAdj("vibrance")} />
          <Slider label="Saturation" tip="s_saturation" value={a.saturation} onChange={setAdj("saturation")} />
          {adjusted && <button className="link reset-link" onClick={resetAdjust}>Back to auto (undo my changes)</button>}
        </div>

        <div className={g("", "curve")} data-g="curve">
          <div className="row">
            <h2><Tip k="t_curve">Tone curve</Tip></h2>
          </div>
          <CurveEditor curves={settings.curves} onChange={(c) => update((s) => void (s.curves = c))} histogram={showing?.scopes_after.hist_l} />
          <h2 className="sub"><Tip k="t_param">Regions</Tip></h2>
          <Slider label="Highlights" tip="t_param" value={a.param[0]} onChange={setParam(0)} />
          <Slider label="Lights" tip="t_param" value={a.param[1]} onChange={setParam(1)} />
          <Slider label="Darks" tip="t_param" value={a.param[2]} onChange={setParam(2)} />
          <Slider label="Shadows" tip="t_param" value={a.param[3]} onChange={setParam(3)} />
        </div>

        <div className={g("grading", "grading")} data-g="grading">
          <div className="row">
            <h2><Tip k="c_grading">Colour grading</Tip></h2>
            <div className="seg mini" role="radiogroup" aria-label="Wheels">
              <button className={gradeView === "3way" ? "on" : ""} aria-pressed={gradeView === "3way"} onClick={() => setGradeView("3way")}>3-way</button>
              <button className={gradeView === "global" ? "on" : ""} aria-pressed={gradeView === "global"} onClick={() => setGradeView("global")}>Global</button>
            </div>
          </div>
          {gradeView === "3way" ? (
            <>
              <div className="gw-row">
                <GradeWheel label="Midtones" value={a.grading.midtones} onChange={(w) => update((s) => void (s.adjust.grading.midtones = w))} />
              </div>
              <div className="gw-row">
                <GradeWheel label="Shadows" value={a.grading.shadows} onChange={(w) => update((s) => void (s.adjust.grading.shadows = w))} />
                <GradeWheel label="Highlights" value={a.grading.highlights} onChange={(w) => update((s) => void (s.adjust.grading.highlights = w))} />
              </div>
            </>
          ) : (
            <div className="gw-row">
              <GradeWheel label="Global" size={150} value={a.grading.global} onChange={(w) => update((s) => void (s.adjust.grading.global = w))} />
            </div>
          )}
          <Slider label="Blending" tip="c_blending" value={a.grading.blending} min={0} max={100} def={50} onChange={(v) => update((s) => void (s.adjust.grading.blending = v))} />
          <Slider label="Balance" tip="c_balance" value={a.grading.balance} onChange={(v) => update((s) => void (s.adjust.grading.balance = v))} />
        </div>

        <div className={g("", "mix")} data-g="mix">
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
          <div className={g("report", "report")} data-g="report">
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

        {/* detail + effects */}
        <div className={g("", "detail")} data-g="detail">
          <h2>Detail</h2>
          <h2 className="sub">Sharpening</h2>
          <Slider label="Amount" tip="d_sharpen" value={settings.detail.sharpen} min={0} max={150} def={0} onChange={setDet("sharpen")} />
          <Slider label="Radius" tip="d_radius" value={settings.detail.sharpen_radius} min={0.5} max={3} step={0.1} def={1} onChange={setDet("sharpen_radius")} />
          <Slider label="Masking" tip="d_masking" value={settings.detail.sharpen_masking} min={0} max={100} def={0} onChange={setDet("sharpen_masking")} />
          <h2 className="sub">Noise reduction</h2>
          <Slider label="Luminance" tip="d_noise" value={settings.detail.noise} min={0} max={100} def={0} onChange={setDet("noise")} />
          <Slider label="Colour" tip="d_cnoise" value={settings.detail.color_noise} min={0} max={100} def={0} onChange={setDet("color_noise")} />
          <p className="save-hint">Zoom to 100% (Z) to judge sharpening and noise.</p>
        </div>

        <div className={g("", "effects")} data-g="effects">
          <h2>Effects</h2>
          <h2 className="sub">Vignette</h2>
          <Slider label="Amount" tip="d_vignette" value={settings.detail.vignette} onChange={setDet("vignette")} />
          <Slider label="Midpoint" tip="d_vmid" value={settings.detail.vignette_midpoint} min={0} max={100} def={50} onChange={setDet("vignette_midpoint")} />
          <Slider label="Feather" tip="d_vfeather" value={settings.detail.vignette_feather} min={0} max={100} def={50} onChange={setDet("vignette_feather")} />
          <h2 className="sub">Grain</h2>
          <Slider label="Amount" tip="d_grain" value={settings.detail.grain} min={0} max={100} def={0} onChange={setDet("grain")} />
          <Slider label="Size" tip="d_gsize" value={settings.detail.grain_size} min={0} max={100} def={25} onChange={setDet("grain_size")} />
        </div>

        <div className={g("", "spots")} data-g="spots">
          <h2><Tip k="s_spots">Remove</Tip></h2>
          <div className="preset-chips spot-tools">
            {(["heal", "clone", "redeye"] as const).map((k) => (
              <button key={k} className={`chip ${spotTool === k ? "on" : ""}`} aria-pressed={spotTool === k} onClick={() => { setSpotTool((t) => (t === k ? null : k)); setMaskSel(null); setPicking(false); }}>
                {k === "heal" ? "Heal" : k === "clone" ? "Clone" : "Red eye"}
              </button>
            ))}
          </div>
          {spotTool && <p className="hint">{spotTool === "redeye" ? "Click each red pupil." : "Click a spot to remove it. Drag the dashed circle to choose where it copies from."} Q to finish, Delete removes the selected one.</p>}
          <Slider label="Brush size" tip="s_size" value={Math.round(spotSize * 1000)} min={3} max={150} def={20} onChange={(v) => setSpotSize(v / 1000)} />
          {spotSel !== null && settings.spots[spotSel] && (
            <>
              <h2 className="sub">Selected spot</h2>
              <Slider label="Size" tip="s_size" value={Math.round(settings.spots[spotSel].r * 1000)} min={3} max={150} def={20} onChange={setSpot("r")} />
              <Slider label="Feather" tip="s_feather" value={Math.round(settings.spots[spotSel].feather * 100)} min={0} max={100} def={50} onChange={setSpot("feather")} />
              <Slider label="Opacity" tip="s_opacity" value={Math.round(settings.spots[spotSel].opacity * 100)} min={0} max={100} def={100} onChange={setSpot("opacity")} />
            </>
          )}
          {settings.spots.length > 0 && (
            <div className="spot-links">
              {spotSel !== null && settings.spots[spotSel] && <button className="link" onClick={() => { update((s) => void s.spots.splice(spotSel, 1)); setSpotSel(null); }}>Delete this spot</button>}
              <button className="link" onClick={() => { update((s) => void (s.spots = [])); setSpotSel(null); }}>Clear all {settings.spots.length} spot{settings.spots.length > 1 ? "s" : ""}</button>
            </div>
          )}
        </div>

        <div className={g("", "lens")} data-g="lens">
          <h2><Tip k="l_lens">Lens &amp; transform</Tip></h2>
          <label className="check"><input type="checkbox" checked={settings.lens.remove_ca} onChange={(e) => update((s) => void (s.lens.remove_ca = e.target.checked))} /> Remove chromatic aberration</label>
          <Slider label="Distortion" tip="l_distortion" value={settings.lens.distortion} onChange={setLens("distortion")} />
          <Slider label="Lens vignetting" tip="l_vignetting" value={settings.lens.vignetting} onChange={setLens("vignetting")} />
          {settings.lens.vignetting !== 0 && <Slider label="Midpoint" tip="l_vignetting" value={settings.lens.vignetting_mid} min={0} max={100} def={50} onChange={setLens("vignetting_mid")} />}
          <h2 className="sub">Defringe</h2>
          <Slider label="Purple" tip="l_defringe" value={settings.lens.defringe_purple} min={0} max={100} def={0} onChange={setLens("defringe_purple")} />
          <Slider label="Green" tip="l_defringe" value={settings.lens.defringe_green} min={0} max={100} def={0} onChange={setLens("defringe_green")} />
          <h2 className="sub"><Tip k="l_transform">Transform</Tip></h2>
          <Slider label="Vertical" tip="l_vertical" value={settings.lens.vertical} onChange={setLens("vertical")} />
          <Slider label="Horizontal" tip="l_vertical" value={settings.lens.horizontal} onChange={setLens("horizontal")} />
          <Slider label="Rotate" tip="l_rotate" value={settings.lens.rotate} min={-10} max={10} step={0.1} suffix="°" onChange={setLens("rotate")} />
          <Slider label="Aspect" tip="l_aspect" value={settings.lens.aspect} onChange={setLens("aspect")} />
          <Slider label="Scale" tip="l_scale" value={settings.lens.scale} min={50} max={150} def={100} onChange={setLens("scale")} />
          <Slider label="Offset X" tip="l_scale" value={settings.lens.x_offset} onChange={setLens("x_offset")} />
          <Slider label="Offset Y" tip="l_scale" value={settings.lens.y_offset} onChange={setLens("y_offset")} />
          <label className="check"><input type="checkbox" checked={settings.lens.constrain} onChange={(e) => update((s) => void (s.lens.constrain = e.target.checked))} /> Constrain crop (no empty edges)</label>
          <button className="link" onClick={() => update((s) => void (s.lens = freshLens()))}>Reset lens &amp; transform</button>
        </div>

        <div className={g("", "calibration")} data-g="calibration">
          <h2><Tip k="c_calibration">Calibration</Tip></h2>
          <Slider label="Shadows tint" tip="c_shtint" value={a.calibration.shadows_tint} onChange={setCal("shadows_tint")} track="linear-gradient(90deg,#4fb35a,#9a9a9a,#c450b8)" />
          <h2 className="sub">Red primary</h2>
          <Slider label="Hue" tip="c_prim" value={a.calibration.red_hue} onChange={setCal("red_hue")} track="linear-gradient(90deg,#e0407a,#e04040,#e0a040)" />
          <Slider label="Saturation" tip="c_prim" value={a.calibration.red_sat} onChange={setCal("red_sat")} />
          <h2 className="sub">Green primary</h2>
          <Slider label="Hue" tip="c_prim" value={a.calibration.green_hue} onChange={setCal("green_hue")} track="linear-gradient(90deg,#c0d040,#40c050,#40c0a0)" />
          <Slider label="Saturation" tip="c_prim" value={a.calibration.green_sat} onChange={setCal("green_sat")} />
          <h2 className="sub">Blue primary</h2>
          <Slider label="Hue" tip="c_prim" value={a.calibration.blue_hue} onChange={setCal("blue_hue")} track="linear-gradient(90deg,#40b0d0,#4060e0,#9040e0)" />
          <Slider label="Saturation" tip="c_prim" value={a.calibration.blue_sat} onChange={setCal("blue_sat")} />
        </div>

        {/* saving */}
        <div className={g("export", "export")} data-g="export">
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
              <button className="wide quiet" onClick={() => setSyncDlg({ mode: "sync", scope: multi.size > 1 ? "selected" : "all" })}>
                {multi.size > 1 ? `Sync these settings to the ${multi.size} selected photos…` : "Sync these settings to all photos…"}
              </button>
            </>
          )}
          <button className="wide quiet" disabled={!current || !showing} onClick={exportLut}>Export LUT for Photoshop</button>
        </div>
      </section>

      {dragOver && <div className="drop-veil">Drop to add</div>}
      {syncDlg && current && (
        <div className="modal-veil" onClick={(e) => e.target === e.currentTarget && setSyncDlg(null)}>
          <div className="modal sync-dlg" role="dialog" aria-modal="true" aria-labelledby="sync-title" onKeyDown={(e) => e.key === "Escape" && setSyncDlg(null)}>
            <h3 id="sync-title">{syncDlg.mode === "copy" ? "Copy settings" : "Sync settings"}</h3>
            <p className="muted">
              {syncDlg.mode === "copy" ? "Choose what to copy from this photo. Then select other photos and press Paste (⌘V)." : "Choose what to take from this photo to the others."}
            </p>
            <div className="sync-all">
              <button className="link" onClick={() => setParts(PARTS.map((p) => p.id))}>Check all</button>
              <button className="link" onClick={() => setParts([])}>Check none</button>
              <button className="link" onClick={() => setParts(DEFAULT_PARTS)}>Usual</button>
            </div>
            <ul className="sync-parts">
              {PARTS.map((p) => (
                <li key={p.id}>
                  <label className="check">
                    <input type="checkbox" checked={parts.includes(p.id)} onChange={(e) => setParts((x) => (e.target.checked ? [...x, p.id] : x.filter((y) => y !== p.id)))} />
                    <span>
                      {p.label}
                      {p.hint && <small>{p.hint}</small>}
                    </span>
                  </label>
                </li>
              ))}
            </ul>
            {syncDlg.mode === "sync" && multi.size > 1 && (
              <div className="seg mini" role="radiogroup" aria-label="Sync to">
                <button className={syncDlg.scope === "selected" ? "on" : ""} aria-pressed={syncDlg.scope === "selected"} onClick={() => setSyncDlg({ ...syncDlg, scope: "selected" })}>{multi.size} selected</button>
                <button className={syncDlg.scope === "all" ? "on" : ""} aria-pressed={syncDlg.scope === "all"} onClick={() => setSyncDlg({ ...syncDlg, scope: "all" })}>All {photos.length} photos</button>
              </div>
            )}
            <div className="modal-actions">
              <button onClick={() => setSyncDlg(null)}>Cancel</button>
              <button
                className="primary"
                autoFocus
                disabled={!parts.length}
                onClick={() => {
                  if (syncDlg.mode === "copy") copySettings(parts);
                  else syncTo(syncDlg.scope, parts);
                  setSyncDlg(null);
                }}
              >
                {syncDlg.mode === "copy" ? "Copy" : syncDlg.scope === "selected" && multi.size > 1 ? `Sync to ${multi.size - (multi.has(current.path) ? 1 : 0)} photos` : `Sync to ${photos.length - 1} photos`}
              </button>
            </div>
          </div>
        </div>
      )}
      {toast && (
        <div className="toast" role="status">
          <span>{toast.text}</span>
          {toast.reveal && <button className="link dark" onClick={() => revealItemInDir(toast.reveal!)}>Show in Finder</button>}
        </div>
      )}
    </div>
  );
}
