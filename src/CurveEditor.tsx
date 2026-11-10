import { useRef, useState } from "react";

export type Curves = { rgb: number[][]; r: number[][]; g: number[][]; b: number[][] };
export const freshCurves = (): Curves => ({ rgb: [], r: [], g: [], b: [] });
const STRAIGHT = [[0, 0], [1, 1]];
type Ch = keyof Curves;
const CH: [Ch, string, string][] = [["rgb", "RGB", "#e8e8e8"], ["r", "Red", "#ff5a5a"], ["g", "Green", "#4fd26a"], ["b", "Blue", "#5a8cff"]];

/** Monotone cubic (same maths as the engine) sampled for drawing. */
function sample(points: number[][], n = 128): number[][] {
  const p = [...points].sort((a, b) => a[0] - b[0]);
  if (p.length < 2) return [[0, 0], [1, 1]];
  const k = p.length;
  const d = p.slice(0, -1).map((q, i) => (p[i + 1][1] - q[1]) / Math.max(1e-6, p[i + 1][0] - q[0]));
  const m = p.map((_, i) => (i === 0 ? d[0] : i === k - 1 ? d[k - 2] : d[i - 1] * d[i] <= 0 ? 0 : (d[i - 1] + d[i]) / 2));
  for (let i = 0; i < k - 1; i++) {
    if (Math.abs(d[i]) < 1e-9) {
      m[i] = m[i + 1] = 0;
    } else {
      const a = m[i] / d[i];
      const b = m[i + 1] / d[i];
      const s = a * a + b * b;
      if (s > 9) {
        const t = 3 / Math.sqrt(s);
        m[i] = t * a * d[i];
        m[i + 1] = t * b * d[i];
      }
    }
  }
  const out: number[][] = [];
  for (let j = 0; j <= n; j++) {
    const x = j / n;
    let y: number;
    if (x <= p[0][0]) y = p[0][1];
    else if (x >= p[k - 1][0]) y = p[k - 1][1];
    else {
      let i = 0;
      while (i < k - 2 && x > p[i + 1][0]) i++;
      const h = Math.max(1e-6, p[i + 1][0] - p[i][0]);
      const t = (x - p[i][0]) / h;
      const t2 = t * t, t3 = t2 * t;
      y = (2 * t3 - 3 * t2 + 1) * p[i][1] + (t3 - 2 * t2 + t) * h * m[i] + (-2 * t3 + 3 * t2) * p[i + 1][1] + (t3 - t2) * h * m[i + 1];
    }
    out.push([x, Math.max(0, Math.min(1, y))]);
  }
  return out;
}

export function CurveEditor({ curves, onChange, histogram }: { curves: Curves; onChange: (c: Curves) => void; histogram?: number[] }) {
  const [ch, setCh] = useState<Ch>("rgb");
  const svg = useRef<SVGSVGElement>(null);
  const drag = useRef<number | null>(null);
  const pts = curves[ch].length >= 2 ? curves[ch] : STRAIGHT;
  const S = 240;
  const toXY = (e: React.PointerEvent) => {
    const r = svg.current!.getBoundingClientRect();
    return [Math.max(0, Math.min(1, (e.clientX - r.left) / r.width)), Math.max(0, Math.min(1, 1 - (e.clientY - r.top) / r.height))];
  };
  const set = (p: number[][]) => onChange({ ...curves, [ch]: p });
  const color = CH.find((c) => c[0] === ch)![2];
  const path = sample(pts).map(([x, y], i) => `${i ? "L" : "M"}${(x * S).toFixed(1)},${((1 - y) * S).toFixed(1)}`).join("");
  const hist = histogram && histogram.length ? histogram : null;
  const hmax = hist ? Math.max(...hist) || 1 : 1;

  return (
    <div className="curve-editor">
      <div className="seg mini curve-ch" role="radiogroup" aria-label="Curve channel">
        {CH.map(([id, label, col]) => (
          <button key={id} className={ch === id ? "on" : ""} onClick={() => setCh(id)} aria-pressed={ch === id} style={ch === id ? { color: col } : undefined}>
            {label}
            {curves[id].length >= 2 && JSON.stringify(curves[id]) !== JSON.stringify(STRAIGHT) ? " •" : ""}
          </button>
        ))}
      </div>
      <svg
        ref={svg}
        viewBox={`0 0 ${S} ${S}`}
        className="curve-svg"
        onPointerDown={(e) => {
          (e.currentTarget as Element).setPointerCapture(e.pointerId);
          const [x, y] = toXY(e);
          const hit = pts.findIndex((p) => Math.abs(p[0] - x) < 0.04 && Math.abs(p[1] - y) < 0.06);
          if (hit >= 0) {
            drag.current = hit;
          } else {
            const n = [...pts, [x, y]].sort((a, b) => a[0] - b[0]);
            drag.current = n.findIndex((p) => p[0] === x && p[1] === y);
            set(n);
          }
        }}
        onPointerMove={(e) => {
          if (drag.current === null || e.buttons !== 1) return;
          const [x, y] = toXY(e);
          const i = drag.current;
          const n = pts.map((p) => [...p]);
          const lo = i > 0 ? n[i - 1][0] + 0.01 : 0;
          const hi = i < n.length - 1 ? n[i + 1][0] - 0.01 : 1;
          // end points slide only vertically… unless dragged inwards (black / white point)
          n[i] = [Math.max(lo, Math.min(hi, x)), y];
          // dragging a middle point far outside the box removes it
          if (i > 0 && i < n.length - 1 && (e.clientY < svg.current!.getBoundingClientRect().top - 24 || e.clientY > svg.current!.getBoundingClientRect().bottom + 24)) {
            n.splice(i, 1);
            drag.current = null;
          }
          set(n);
        }}
        onPointerUp={() => (drag.current = null)}
        onDoubleClick={(e) => {
          const [x, y] = toXY(e as unknown as React.PointerEvent);
          const hit = pts.findIndex((p) => Math.abs(p[0] - x) < 0.04 && Math.abs(p[1] - y) < 0.06);
          if (hit > 0 && hit < pts.length - 1) set(pts.filter((_, j) => j !== hit));
        }}
        role="img"
        aria-label={`${ch.toUpperCase()} tone curve`}
      >
        {hist && (
          <path
            d={`M0,${S} ` + hist.map((v, i) => `L${((i / (hist.length - 1)) * S).toFixed(1)},${(S - (v / hmax) * S * 0.9).toFixed(1)}`).join(" ") + ` L${S},${S} Z`}
            className="curve-hist"
          />
        )}
        {[0.25, 0.5, 0.75].map((g) => (
          <g key={g} className="curve-grid">
            <line x1={g * S} y1={0} x2={g * S} y2={S} />
            <line x1={0} y1={g * S} x2={S} y2={g * S} />
          </g>
        ))}
        <line x1={0} y1={S} x2={S} y2={0} className="curve-diag" />
        <path d={path} fill="none" stroke={color} strokeWidth={2} />
        {pts.map((p, i) => (
          <circle key={i} cx={p[0] * S} cy={(1 - p[1]) * S} r={5} fill="#1d1d20" stroke={color} strokeWidth={2} />
        ))}
      </svg>
      <div className="curve-foot">
        <span className="muted">Click to add a point, drag to move, double-click to remove.</span>
        <button className="link" onClick={() => set([])} disabled={curves[ch].length < 2}>Reset</button>
      </div>
    </div>
  );
}
