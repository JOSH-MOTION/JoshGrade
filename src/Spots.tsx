import { useRef, useState } from "react";
import { toTurned, toUpright } from "./Masks";

/** Remove (heal / clone) and red-eye spots. Positions are fractions of the upright photo. */
export type SpotKind = "heal" | "clone" | "redeye";
export type Spot = { kind: SpotKind; x: number; y: number; r: number; sx: number; sy: number; feather: number; opacity: number };

type Turn = { rot: number; flip: boolean };

/**
 * Drawn inside the viewer's image layer. Click on the photo to add a spot; drag a spot to
 * move it, drag its source (dashed) to choose where it copies from.
 */
export function SpotCanvas(props: {
  kind: SpotKind; size: number; spots: Spot[]; sel: number | null; Ws: number; Hs: number; turn: Turn; zoom: number;
  toImage: (clientX: number, clientY: number) => number[];
  onAdd: (x: number, y: number) => void; onChange: (i: number, s: Spot) => void; onSelect: (i: number | null) => void;
}) {
  const { Ws, Hs, turn, zoom } = props;
  const long = Math.max(Ws, Hs);
  const [hover, setHover] = useState<number[] | null>(null);
  const drag = useRef<null | { i: number; what: "spot" | "src"; start: number[]; orig: Spot }>(null);
  const px = (u: number[]) => {
    const t = toTurned(u, turn);
    return [t[0] * Ws, t[1] * Hs];
  };
  const up = (e: React.PointerEvent) => toUpright(props.toImage(e.clientX, e.clientY), turn);
  const lw = 1.5 / zoom;

  const begin = (e: React.PointerEvent, i: number, what: "spot" | "src") => {
    e.stopPropagation();
    e.preventDefault();
    (e.currentTarget as Element).setPointerCapture(e.pointerId);
    props.onSelect(i);
    drag.current = { i, what, start: up(e), orig: { ...props.spots[i] } };
  };
  const move = (e: React.PointerEvent) => {
    setHover(props.toImage(e.clientX, e.clientY));
    const d = drag.current;
    if (!d || e.buttons !== 1) return;
    const p = up(e);
    const [dx, dy] = [p[0] - d.start[0], p[1] - d.start[1]];
    const o = d.orig;
    props.onChange(d.i, d.what === "spot" ? { ...o, x: o.x + dx, y: o.y + dy, sx: o.sx + dx, sy: o.sy + dy } : { ...o, sx: o.sx + dx, sy: o.sy + dy });
  };
  const end = () => (drag.current = null);

  const cursorR = props.size * long;
  return (
    <svg
      className="spot-canvas"
      width={Ws}
      height={Hs}
      viewBox={`0 0 ${Ws} ${Hs}`}
      onPointerMove={move}
      onPointerUp={end}
      onPointerLeave={() => setHover(null)}
    >
      <rect
        x={0}
        y={0}
        width={Ws}
        height={Hs}
        fill="transparent"
        style={{ cursor: "none" }}
        onPointerDown={(e) => {
          e.stopPropagation();
          const p = up(e);
          if (p[0] < 0 || p[0] > 1 || p[1] < 0 || p[1] > 1) return;
          props.onAdd(p[0], p[1]);
        }}
      />
      {props.spots.map((s, i) => {
        const c = px([s.x, s.y]);
        const r = s.r * long;
        const on = i === props.sel;
        const src = px([s.sx, s.sy]);
        return (
          <g key={i} className={`spot ${on ? "on" : ""}`}>
            {s.kind !== "redeye" && on && (
              <>
                <line x1={c[0]} y1={c[1]} x2={src[0]} y2={src[1]} stroke="#fff" strokeWidth={lw} strokeDasharray={`${4 / zoom} ${4 / zoom}`} pointerEvents="none" />
                <circle cx={src[0]} cy={src[1]} r={r} className="spot-src" strokeWidth={lw * (on ? 1.4 : 1)} strokeDasharray={`${5 / zoom} ${4 / zoom}`} onPointerDown={(e) => begin(e, i, "src")} />
              </>
            )}
            <circle cx={c[0]} cy={c[1]} r={r} className={`spot-dst k-${s.kind}`} strokeWidth={lw * (on ? 1.6 : 1)} onPointerDown={(e) => begin(e, i, "spot")} />
          </g>
        );
      })}
      {hover && !drag.current && (
        <circle cx={hover[0] * Ws} cy={hover[1] * Hs} r={cursorR} fill="none" stroke="#fff" strokeWidth={lw} pointerEvents="none" style={{ filter: "drop-shadow(0 0 1px rgba(0,0,0,.9))" }} />
      )}
    </svg>
  );
}
