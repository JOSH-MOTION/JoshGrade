import { useEffect, useRef, useState, type ReactNode } from "react";
import { MaskCanvas, type Brush, type Mask } from "./Masks";

export type Crop = { rot90: number; flip_h: boolean; angle: number; x: number; y: number; w: number; h: number };
export const freshCrop = (): Crop => ({ rot90: 0, flip_h: false, angle: 0, x: 0, y: 0, w: 1, h: 1 });
export const cropActive = (c: Crop) => c.rot90 % 4 !== 0 || c.flip_h || Math.abs(c.angle) > 0.01 || c.x > 0.0005 || c.y > 0.0005 || c.w < 0.9995 || c.h < 0.9995;

type Tile = { key: string; region: [number, number, number, number]; url: string };

/** Does a crop frame (normalised, in a W×H photo) sit fully inside the photo rotated by `angle`? */
function inside(c: Crop, W: number, H: number, k = 1) {
  const t = (-c.angle * Math.PI) / 180;
  const [sn, cs] = [Math.sin(t), Math.cos(t)];
  const cx = (c.x + c.w / 2) * W;
  const cy = (c.y + c.h / 2) * H;
  for (const [dx, dy] of [[-0.5, -0.5], [0.5, -0.5], [0.5, 0.5], [-0.5, 0.5]]) {
    const px = cx + dx * c.w * W * k - W / 2;
    const py = cy + dy * c.h * H * k - H / 2;
    const sx = px * cs - py * sn + W / 2;
    const sy = px * sn + py * cs + H / 2;
    if (sx < -0.5 || sy < -0.5 || sx > W + 0.5 || sy > H + 0.5) return false;
  }
  return true;
}

/** Shrink the crop about its centre until it fits inside the (straightened) photo. */
export function fitCrop(c: Crop, W: number, H: number): Crop {
  let n = { ...c, w: Math.min(c.w, 1), h: Math.min(c.h, 1) };
  n.x = Math.max(0, Math.min(1 - n.w, n.x));
  n.y = Math.max(0, Math.min(1 - n.h, n.y));
  if (inside(n, W, H)) return n;
  // first try pulling it towards the centre, then shrink
  for (let i = 0; i < 40 && !inside(n, W, H); i++) {
    const k = 0.98;
    const cx = n.x + n.w / 2;
    const cy = n.y + n.h / 2;
    const ncx = cx + (0.5 - cx) * 0.15;
    const ncy = cy + (0.5 - cy) * 0.15;
    n = { ...n, w: n.w * k, h: n.h * k, x: ncx - (n.w * k) / 2, y: ncy - (n.h * k) / 2 };
  }
  return n;
}

/** Largest crop of the given aspect (width/height in pixels) that fits, centred where `c` is. */
export function cropToAspect(c: Crop, W: number, H: number, aspect: number | null): Crop {
  if (!aspect) return c;
  const cx = c.x + c.w / 2;
  const cy = c.y + c.h / 2;
  let w = 1;
  let h = (W / aspect) / H;
  if (h > 1) {
    h = 1;
    w = (H * aspect) / W;
  }
  return fitCrop({ ...c, w, h, x: cx - w / 2, y: cy - h / 2 }, W, H);
}

export function Viewer(props: {
  after: string; before: string; overlay?: string | null; W: number; H: number; fullW: number; fullH: number;
  crop: Crop; cropMode: boolean; aspect: number | null; onCrop: (c: Crop) => void;
  split: number; setSplit: (n: number) => void; showBefore: boolean; tagL: string; tagR: string;
  fetchTile: (region: [number, number, number, number], maxPx: number, before: boolean) => Promise<string>;
  tileKey: string; zoomCmd: { n: number; to: "fit" | "100" | "in" | "out" } | null; onZoom: (z: number, z100: number) => void;
  /** Editing a mask: draw its handles / brush on the photo. */
  maskTool?: { mask: Mask; brush: Brush; onChange: (m: Mask) => void } | null;
  /** Picking a point (white balance eyedropper, spot tools): x, y as fractions of the shown photo. */
  onPick?: ((x: number, y: number) => void) | null;
}) {
  const { W, H, crop, cropMode } = props;
  const box = useRef<HTMLDivElement>(null);
  const [size, setSize] = useState({ w: 0, h: 0 });
  const [zoom, setZoom] = useState(1);
  const [pan, setPan] = useState({ x: 0, y: 0 });
  const [tiles, setTiles] = useState<{ after?: Tile; before?: Tile }>({});
  const drag = useRef<null | { mode: string; x: number; y: number; c: Crop; pan: { x: number; y: number } }>(null);

  useEffect(() => {
    const el = box.current;
    if (!el) return;
    const ro = new ResizeObserver(([e]) => setSize({ w: e.contentRect.width, h: e.contentRect.height }));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // what the frame shows (preview pixels)
  const fx = cropMode ? 0 : crop.x * W;
  const fy = cropMode ? 0 : crop.y * H;
  const fw = cropMode ? W : crop.w * W;
  const fh = cropMode ? H : crop.h * H;
  const pad = cropMode ? 40 : 0;
  const s = Math.max(0.0001, Math.min((size.w - pad) / fw, (size.h - pad) / fh));
  const frameW = Math.floor(fw * s);
  const frameH = Math.floor(fh * s);
  const z100 = props.fullW / (W * s);
  const maxZoom = Math.max(2, z100 * 2);

  useEffect(() => props.onZoom(zoom, z100), [zoom, z100]); // eslint-disable-line react-hooks/exhaustive-deps

  // reset zoom on a new photo or when cropping
  useEffect(() => {
    setZoom(1);
    setPan({ x: 0, y: 0 });
    setTiles({});
  }, [props.tileKey.split("|")[0], cropMode]); // eslint-disable-line react-hooks/exhaustive-deps

  const clampPan = (p: { x: number; y: number }, z: number) => {
    const mx = ((z - 1) * frameW) / 2;
    const my = ((z - 1) * frameH) / 2;
    return { x: Math.max(-mx, Math.min(mx, p.x)), y: Math.max(-my, Math.min(my, p.y)) };
  };
  const zoomAt = (nz: number, mx: number, my: number) => {
    nz = Math.max(1, Math.min(maxZoom, nz));
    const cx = frameW / 2;
    const cy = frameH / 2;
    const lx = cx + (mx - cx - pan.x) / zoom;
    const ly = cy + (my - cy - pan.y) / zoom;
    setPan(clampPan({ x: mx - cx - (lx - cx) * nz, y: my - cy - (ly - cy) * nz }, nz));
    setZoom(nz);
  };

  useEffect(() => {
    const c = props.zoomCmd;
    if (!c || cropMode) return;
    if (c.to === "fit") {
      setZoom(1);
      setPan({ x: 0, y: 0 });
    } else if (c.to === "100") zoomAt(zoom > 1.01 && Math.abs(zoom - z100) < 0.01 ? 1 : z100, frameW / 2, frameH / 2);
    else zoomAt(zoom * (c.to === "in" ? 1.5 : 1 / 1.5), frameW / 2, frameH / 2);
  }, [props.zoomCmd?.n]); // eslint-disable-line react-hooks/exhaustive-deps

  // sharp full-resolution piece of what's on screen, when zoomed past the preview's detail
  useEffect(() => {
    if (cropMode || zoom * s <= 1.02 || !frameW) {
      setTiles({});
      return;
    }
    const t = setTimeout(async () => {
      const cx = frameW / 2;
      const cy = frameH / 2;
      const th = (-props.crop.angle * Math.PI) / 180;
      const [sn, cs] = [Math.sin(th), Math.cos(th)];
      const pts = [[0, 0], [frameW, 0], [frameW, frameH], [0, frameH]].map(([px, py]) => {
        const lx = cx + (px - cx - pan.x) / zoom;
        const ly = cy + (py - cy - pan.y) / zoom;
        // into the (rotated) image layer
        const ix = lx + fx * s - (W * s) / 2;
        const iy = ly + fy * s - (H * s) / 2;
        const ux = ix * cs - iy * sn + (W * s) / 2;
        const uy = ix * sn + iy * cs + (H * s) / 2;
        return [ux / (W * s), uy / (H * s)];
      });
      const xs = pts.map((p) => p[0]);
      const ys = pts.map((p) => p[1]);
      const x0 = Math.max(0, Math.min(...xs) - 0.02);
      const y0 = Math.max(0, Math.min(...ys) - 0.02);
      const x1 = Math.min(1, Math.max(...xs) + 0.02);
      const y1 = Math.min(1, Math.max(...ys) + 0.02);
      const region: [number, number, number, number] = [x0, y0, x1 - x0, y1 - y0];
      const dpr = window.devicePixelRatio || 1;
      const maxPx = Math.min(3200, Math.ceil(Math.max((x1 - x0) * W, (y1 - y0) * H) * s * zoom * dpr));
      const key = `${props.tileKey}|${region.map((v) => v.toFixed(3)).join(",")}|${maxPx}`;
      try {
        const [a, b] = await Promise.all([props.fetchTile(region, maxPx, false), props.fetchTile(region, maxPx, true)]);
        setTiles({ after: { key, region, url: a }, before: { key, region, url: b } });
      } catch {
        /* keep the soft preview */
      }
    }, 280);
    return () => clearTimeout(t);
  }, [zoom, pan.x, pan.y, props.tileKey, frameW, frameH, cropMode]); // eslint-disable-line react-hooks/exhaustive-deps

  // pointer -> position in the shown (turned) photo, 0..1
  const toImage = (clientX: number, clientY: number) => {
    const r = box.current?.querySelector(".vframe")?.getBoundingClientRect();
    if (!r) return [0, 0];
    const cx = frameW / 2;
    const cy = frameH / 2;
    const lx = cx + (clientX - r.left - cx - pan.x) / zoom;
    const ly = cy + (clientY - r.top - cy - pan.y) / zoom;
    const th = (-crop.angle * Math.PI) / 180;
    const [sn, cs] = [Math.sin(th), Math.cos(th)];
    const ix = lx + fx * s - (W * s) / 2;
    const iy = ly + fy * s - (H * s) / 2;
    return [(ix * cs - iy * sn + (W * s) / 2) / (W * s), (ix * sn + iy * cs + (H * s) / 2) / (H * s)];
  };
  const tool = !cropMode && props.maskTool ? props.maskTool : null;

  const layer = (src: string, tile?: Tile, extra?: string | null, child?: ReactNode) => (
    <div
      className="img-layer"
      style={{ left: -fx * s, top: -fy * s, width: W * s, height: H * s, transform: crop.angle ? `rotate(${crop.angle}deg)` : undefined }}
    >
      <img src={src} alt="" draggable={false} />
      {tile && (
        <img
          className="tile"
          src={tile.url}
          alt=""
          draggable={false}
          style={{ left: `${tile.region[0] * 100}%`, top: `${tile.region[1] * 100}%`, width: `${tile.region[2] * 100}%`, height: `${tile.region[3] * 100}%` }}
        />
      )}
      {extra && <img src={extra} alt="" draggable={false} className="overlay" />}
      {child}
    </div>
  );
  const zoomStyle = { transform: `translate(${pan.x}px, ${pan.y}px) scale(${zoom})` };

  // ---- pointer handling: crop editing, panning, or the compare divider
  const onDown = (e: React.PointerEvent) => {
    if (props.onPick && !cropMode) {
      const [u, v] = toImage(e.clientX, e.clientY);
      if (u >= 0 && u <= 1 && v >= 0 && v <= 1) props.onPick(u, v);
      return;
    }
    const t = e.target as HTMLElement;
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    const mode = t.dataset.h ?? (cropMode ? (t.closest(".crop-rect") ? "move" : "") : zoom > 1.001 && !t.closest(".divider") ? "pan" : tool ? "" : "split");
    drag.current = { mode, x: e.clientX, y: e.clientY, c: { ...crop }, pan: { ...pan } };
    if (mode === "split") moveSplit(e.clientX);
  };
  const moveSplit = (clientX: number) => {
    const r = box.current?.querySelector(".vframe")?.getBoundingClientRect();
    if (r) props.setSplit(Math.max(0, Math.min(100, ((clientX - r.left) / r.width) * 100)));
  };
  const onMove = (e: React.PointerEvent) => {
    const d = drag.current;
    if (!d || e.buttons !== 1) return;
    if (d.mode === "split") return moveSplit(e.clientX);
    if (d.mode === "pan") return setPan(clampPan({ x: d.pan.x + e.clientX - d.x, y: d.pan.y + e.clientY - d.y }, zoom));
    if (!cropMode || !d.mode) return;
    const dx = (e.clientX - d.x) / (W * s);
    const dy = (e.clientY - d.y) / (H * s);
    let c = { ...d.c };
    if (d.mode === "move") {
      c.x = Math.max(0, Math.min(1 - c.w, c.x + dx));
      c.y = Math.max(0, Math.min(1 - c.h, c.y + dy));
      if (!inside(c, W, H)) return; // can't slide off the photo
      return props.onCrop(c);
    }
    const m = d.mode;
    let { x, y, w, h } = c;
    const min = 0.03;
    if (m.includes("l")) { const nx = Math.min(x + w - min, Math.max(0, x + dx)); w += x - nx; x = nx; }
    if (m.includes("r")) w = Math.max(min, Math.min(1 - x, w + dx));
    if (m.includes("t")) { const ny = Math.min(y + h - min, Math.max(0, y + dy)); h += y - ny; y = ny; }
    if (m.includes("b")) h = Math.max(min, Math.min(1 - y, h + dy));
    if (props.aspect) {
      // keep the ratio, driven by the dominant edge
      const want = props.aspect;
      const cur = (w * W) / (h * H);
      if (m === "l" || m === "r" || (m.length === 2 && cur > want)) {
        const nh = (w * W) / want / H;
        if (m.includes("t")) y += h - nh;
        else if (m.length === 1) y += (h - nh) / 2;
        h = nh;
      } else {
        const nw = (h * H * want) / W;
        if (m.includes("l")) x += w - nw;
        else if (m.length === 1) x += (w - nw) / 2;
        w = nw;
      }
    }
    c = { ...c, x, y, w, h };
    if (x < -0.001 || y < -0.001 || x + w > 1.001 || y + h > 1.001 || !inside(c, W, H)) return;
    props.onCrop(c);
  };
  const onUp = () => {
    drag.current = null;
  };

  return (
    <div
      className={`viewer ${cropMode ? "cropping" : ""} ${zoom > 1.001 ? "zoomed" : ""} ${tool ? `masking m-${tool.mask.kind}` : ""} ${props.onPick ? "picking" : ""}`}
      ref={box}
      onPointerDown={onDown}
      onPointerMove={onMove}
      onPointerUp={onUp}
      onWheel={(e) => {
        if (cropMode) return;
        const r = box.current?.querySelector(".vframe")?.getBoundingClientRect();
        if (!r) return;
        if (!e.ctrlKey && !e.metaKey && zoom > 1.001) {
          // two-finger scroll / wheel pans when zoomed in
          setPan((p) => clampPan({ x: p.x - e.deltaX, y: p.y - e.deltaY }, zoom));
          return;
        }
        // pinch (ctrl+wheel on a Mac trackpad), cmd+wheel, or wheel while fitted: zoom
        zoomAt(zoom * Math.exp(-e.deltaY * (e.ctrlKey ? 0.01 : 0.002)), e.clientX - r.left, e.clientY - r.top);
      }}
      onDoubleClick={(e) => {
        if (cropMode) return;
        const r = box.current?.querySelector(".vframe")?.getBoundingClientRect();
        if (r) zoomAt(zoom > 1.01 ? 1 : z100, e.clientX - r.left, e.clientY - r.top);
      }}
    >
      <div className="vframe" style={{ width: frameW, height: frameH }}>
        <div className="zoom-wrap" style={zoomStyle}>
          {layer(
            props.after,
            tiles.after,
            props.overlay,
            tool && <MaskCanvas mask={tool.mask} brush={tool.brush} onChange={tool.onChange} Ws={W * s} Hs={H * s} turn={{ rot: crop.rot90, flip: crop.flip_h }} zoom={zoom} toImage={toImage} />,
          )}
        </div>
        {!cropMode && (!tool || props.showBefore) && (
          <div className="before-clip" style={{ clipPath: props.showBefore ? "none" : `inset(0 ${100 - props.split}% 0 0)` }}>
            <div className="zoom-wrap" style={zoomStyle}>{layer(props.before, tiles.before)}</div>
          </div>
        )}
        {!cropMode && !tool && !props.showBefore && (
          <div className="divider" style={{ left: `${props.split}%` }} role="slider" tabIndex={0} aria-label="Before and after divider" aria-valuenow={Math.round(props.split)}
            onKeyDown={(e) => {
              if (e.key === "ArrowLeft") props.setSplit(Math.max(0, props.split - 5));
              if (e.key === "ArrowRight") props.setSplit(Math.min(100, props.split + 5));
            }}>
            <span className="knob" />
          </div>
        )}
        {!cropMode && !tool && <span className="tag tag-l">{props.tagL}</span>}
        {!cropMode && !tool && !props.showBefore && <span className="tag tag-r">{props.tagR}</span>}
        {cropMode && (
          <div className="crop-ui">
            <div className="crop-rect" style={{ left: crop.x * W * s, top: crop.y * H * s, width: crop.w * W * s, height: crop.h * H * s }}>
              <span className="third v1" /><span className="third v2" /><span className="third h1" /><span className="third h2" />
              {["tl", "t", "tr", "r", "br", "b", "bl", "l"].map((h) => <span key={h} className={`hdl h-${h}`} data-h={h} />)}
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
