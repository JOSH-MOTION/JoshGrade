import { useRef, useState } from "react";

/** Local adjustments, like Lightroom's Masking panel. Positions are fractions of the upright photo. */
export type MaskAdjust = { exposure: number; contrast: number; highlights: number; shadows: number; temp: number; tint: number; saturation: number; clarity: number };
export type Stroke = { pts: number[][]; size: number; feather: number; flow: number; erase: boolean };
export type MaskKind = "subject" | "sky" | "background" | "brush" | "linear" | "radial";
export type Mask = {
  kind: MaskKind; name: string; enabled: boolean; invert: boolean; amount: number; adjust: MaskAdjust;
  strokes: Stroke[]; a: number[]; b: number[]; angle: number; feather: number;
};
export type Brush = { size: number; feather: number; flow: number; erase: boolean };

export const freshMaskAdjust = (): MaskAdjust => ({ exposure: 0, contrast: 0, highlights: 0, shadows: 0, temp: 0, tint: 0, saturation: 0, clarity: 0 });
export const KINDS: { kind: MaskKind; label: string; ai?: boolean; hint: string }[] = [
  { kind: "subject", label: "Person", ai: true, hint: "Finds the people in the photo" },
  { kind: "sky", label: "Sky", ai: true, hint: "Finds the sky" },
  { kind: "background", label: "Background", ai: true, hint: "Everything except the people" },
  { kind: "brush", label: "Brush", hint: "Paint where you want the change" },
  { kind: "linear", label: "Linear", hint: "A soft graduated filter, e.g. to darken a bright sky" },
  { kind: "radial", label: "Radial", hint: "A soft oval, e.g. to light a face" },
];
export const isAi = (k: MaskKind) => k === "subject" || k === "sky" || k === "background";

export function newMask(kind: MaskKind, existing: Mask[]): Mask {
  const base = KINDS.find((k) => k.kind === kind)!.label;
  const n = existing.filter((m) => m.kind === kind).length;
  return {
    kind, name: n ? `${base} ${n + 1}` : base, enabled: true, invert: false, amount: 1, adjust: freshMaskAdjust(), strokes: [],
    a: kind === "radial" ? [0.5, 0.45] : [0.5, 0.15],
    b: kind === "radial" ? [0.22, 0.3] : [0.5, 0.55],
    angle: 0, feather: 50,
  };
}

// ---------- coordinates: upright (stored) <-> turned (what the viewer shows)
type Turn = { rot: number; flip: boolean };
export function toTurned([a, b]: number[], t: Turn): number[] {
  const r = ((t.rot % 4) + 4) % 4;
  let [x, y] = r === 0 ? [a, b] : r === 1 ? [1 - b, a] : r === 2 ? [1 - a, 1 - b] : [b, 1 - a];
  if (t.flip) x = 1 - x;
  return [x, y];
}
export function toUpright([x0, y]: number[], t: Turn): number[] {
  const x = t.flip ? 1 - x0 : x0;
  const r = ((t.rot % 4) + 4) % 4;
  return r === 0 ? [x, y] : r === 1 ? [y, 1 - x] : r === 2 ? [1 - x, 1 - y] : [1 - y, x];
}

/**
 * Drawn inside the viewer's image layer (so it zooms and rotates with the photo).
 * `Ws`×`Hs` = the layer's size in px; `toImage` turns a pointer position into turned 0..1 coords.
 */
export function MaskCanvas(props: {
  mask: Mask; brush: Brush; onChange: (m: Mask) => void; Ws: number; Hs: number; turn: Turn; zoom: number;
  toImage: (clientX: number, clientY: number) => number[];
}) {
  const { mask: m, Ws, Hs, turn, zoom } = props;
  const [cursor, setCursor] = useState<number[] | null>(null);
  const [live, setLive] = useState<number[][] | null>(null);
  const drag = useRef<null | { what: string; start: number[]; m: Mask }>(null);
  // upright pixel frame (same scale as the layer)
  const [Wu, Hu] = turn.rot % 2 ? [Hs, Ws] : [Ws, Hs];
  const px = (u: number[]) => {
    const t = toTurned(u, turn);
    return [t[0] * Ws, t[1] * Hs];
  };
  const up = (e: React.PointerEvent) => toUpright(props.toImage(e.clientX, e.clientY), turn);
  const hr = 7 / zoom; // handle radius, constant on screen
  const lw = 1.5 / zoom;
  const long = Math.max(Ws, Hs);

  const onDown = (e: React.PointerEvent, what: string) => {
    e.stopPropagation();
    e.preventDefault();
    (e.currentTarget as Element).setPointerCapture(e.pointerId);
    const p = up(e);
    drag.current = { what, start: p, m: structuredClone(m) };
    if (what === "paint") setLive([p]);
  };
  const onMove = (e: React.PointerEvent) => {
    const p = up(e);
    if (m.kind === "brush") setCursor(props.toImage(e.clientX, e.clientY));
    const d = drag.current;
    if (!d || e.buttons !== 1) return;
    if (d.what === "paint") return setLive((l) => (l ? [...l, p] : [p]));
    const dx = p[0] - d.start[0];
    const dy = p[1] - d.start[1];
    const n = structuredClone(d.m);
    if (d.what === "move") {
      n.a = [d.m.a[0] + dx, d.m.a[1] + dy];
      if (m.kind === "linear") n.b = [d.m.b[0] + dx, d.m.b[1] + dy];
    } else if (d.what === "a") n.a = p;
    else if (d.what === "b") n.b = p;
    else if (d.what === "rx" || d.what === "ry") {
      // in upright pixels, relative to the centre
      const vx = (p[0] - n.a[0]) * Wu;
      const vy = (p[1] - n.a[1]) * Hu;
      const len = Math.hypot(vx, vy);
      if (d.what === "rx") {
        n.angle = (Math.atan2(vy, vx) * 180) / Math.PI;
        n.b = [Math.max(0.01, len / Wu), n.b[1]];
      } else {
        n.angle = (Math.atan2(-vx, vy) * 180) / Math.PI;
        n.b = [n.b[0], Math.max(0.01, len / Hu)];
      }
    }
    props.onChange(n);
  };
  const onUp = () => {
    const d = drag.current;
    drag.current = null;
    if (d?.what === "paint" && live && live.length) {
      const b = props.brush;
      props.onChange({ ...m, strokes: [...m.strokes, { pts: live.map((p) => [+p[0].toFixed(4), +p[1].toFixed(4)]), size: b.size, feather: b.feather, flow: b.flow, erase: b.erase }] });
    }
    setLive(null);
  };

  const common = { onPointerMove: onMove, onPointerUp: onUp };
  let body: React.ReactNode = null;
  if (m.kind === "brush") {
    const r = props.brush.size * long;
    const c = cursor ? [cursor[0] * Ws, cursor[1] * Hs] : null;
    body = (
      <>
        <rect x={0} y={0} width={Ws} height={Hs} fill="transparent" onPointerDown={(e) => onDown(e, "paint")} onPointerLeave={() => setCursor(null)} style={{ cursor: "none" }} />
        {live && live.length > 0 && (
          <polyline points={live.map((p) => px(p).join(",")).join(" ")} fill="none" stroke={props.brush.erase ? "rgba(80,160,255,.45)" : "rgba(255,45,70,.45)"} strokeWidth={r * 2} strokeLinecap="round" strokeLinejoin="round" pointerEvents="none" />
        )}
        {c && (
          <g pointerEvents="none">
            <circle cx={c[0]} cy={c[1]} r={r} fill="none" stroke="#fff" strokeWidth={lw} />
            <circle cx={c[0]} cy={c[1]} r={r * (1 - props.brush.feather)} fill="none" stroke="rgba(255,255,255,.55)" strokeWidth={lw} strokeDasharray={`${4 / zoom} ${4 / zoom}`} />
            <line x1={c[0] - 4 / zoom} y1={c[1]} x2={c[0] + 4 / zoom} y2={c[1]} stroke="#fff" strokeWidth={lw} />
            {props.brush.erase && <line x1={c[0] - r * 0.35} y1={c[1]} x2={c[0] + r * 0.35} y2={c[1]} stroke="#fff" strokeWidth={lw * 2} />}
          </g>
        )}
      </>
    );
  } else if (m.kind === "linear") {
    // three lines across the photo: full effect, middle, none
    const A = [m.a[0] * Wu, m.a[1] * Hu];
    const B = [m.b[0] * Wu, m.b[1] * Hu];
    const d = [B[0] - A[0], B[1] - A[1]];
    const L = Math.hypot(d[0], d[1]) || 1;
    const perp = [(-d[1] / L) * (Wu + Hu) * 2, (d[0] / L) * (Wu + Hu) * 2];
    const line = (t: number, key: string, dash?: boolean) => {
      const c = [A[0] + d[0] * t, A[1] + d[1] * t];
      const p1 = px([(c[0] - perp[0]) / Wu, (c[1] - perp[1]) / Hu]);
      const p2 = px([(c[0] + perp[0]) / Wu, (c[1] + perp[1]) / Hu]);
      return <line key={key} x1={p1[0]} y1={p1[1]} x2={p2[0]} y2={p2[1]} stroke="#fff" strokeWidth={lw} strokeDasharray={dash ? `${5 / zoom} ${5 / zoom}` : undefined} />;
    };
    const mid = px([(A[0] + d[0] / 2) / Wu, (A[1] + d[1] / 2) / Hu]);
    body = (
      <>
        <g pointerEvents="none" style={{ filter: "drop-shadow(0 0 1px rgba(0,0,0,.8))" }}>
          {line(0, "a")}
          {line(0.5, "m", true)}
          {line(1, "b")}
        </g>
        <circle className="m-hdl" cx={px(m.a)[0]} cy={px(m.a)[1]} r={hr} onPointerDown={(e) => onDown(e, "a")} {...common} />
        <circle className="m-hdl" cx={px(m.b)[0]} cy={px(m.b)[1]} r={hr} onPointerDown={(e) => onDown(e, "b")} {...common} />
        <circle className="m-hdl centre" cx={mid[0]} cy={mid[1]} r={hr * 1.2} onPointerDown={(e) => onDown(e, "move")} {...common} />
      </>
    );
  } else if (m.kind === "radial") {
    const th = (m.angle * Math.PI) / 180;
    const [cx, cy] = [m.a[0] * Wu, m.a[1] * Hu];
    const [rx, ry] = [m.b[0] * Wu, m.b[1] * Hu];
    const at = (u: number, v: number) => px([(cx + u * rx * Math.cos(th) - v * ry * Math.sin(th)) / Wu, (cy + u * rx * Math.sin(th) + v * ry * Math.cos(th)) / Hu]);
    const ring = (k: number) => Array.from({ length: 73 }, (_, i) => at(k * Math.cos((i / 72) * 2 * Math.PI), k * Math.sin((i / 72) * 2 * Math.PI)).join(",")).join(" ");
    const f = Math.min(1, Math.max(0.01, m.feather / 100));
    const hx = at(1, 0);
    const hy = at(0, 1);
    const c = px(m.a);
    body = (
      <>
        <g pointerEvents="none" style={{ filter: "drop-shadow(0 0 1px rgba(0,0,0,.8))" }}>
          <polyline points={ring(1)} fill="none" stroke="#fff" strokeWidth={lw} />
          <polyline points={ring(1 - f)} fill="none" stroke="rgba(255,255,255,.6)" strokeWidth={lw} strokeDasharray={`${5 / zoom} ${5 / zoom}`} />
        </g>
        <circle className="m-hdl centre" cx={c[0]} cy={c[1]} r={hr * 1.2} onPointerDown={(e) => onDown(e, "move")} {...common} />
        <circle className="m-hdl" cx={hx[0]} cy={hx[1]} r={hr} onPointerDown={(e) => onDown(e, "rx")} {...common} />
        <circle className="m-hdl" cx={hy[0]} cy={hy[1]} r={hr} onPointerDown={(e) => onDown(e, "ry")} {...common} />
      </>
    );
  }
  if (!body) return null;
  return (
    <svg className={`mask-canvas ${m.kind}`} width={Ws} height={Hs} viewBox={`0 0 ${Ws} ${Hs}`} onPointerMove={onMove} onPointerUp={onUp} onPointerLeave={() => setCursor(null)}>
      {body}
    </svg>
  );
}
