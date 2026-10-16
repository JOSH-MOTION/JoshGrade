import { useEffect, useRef, useState } from "react";
import { TIPS, Tip } from "./Tip";

export type Band = { name: string; hue: number; share: number; strength: number; saturated: number; clipped: number; over: boolean };
export type ScopeData = {
  hist_r: number[];
  hist_g: number[];
  hist_b: number[];
  hist_l: number[];
  clip_high: number;
  clip_low: number;
  median: number;
  exposure: "under" | "good" | "over";
  wheel: number[];
  bands: Band[];
};

export const bandColor = (hue: number, l = 56) => `hsl(${hue}, 82%, ${l}%)`;

/** Fit the canvas to its CSS size at device resolution. */
function useCanvas(draw: (ctx: CanvasRenderingContext2D, w: number, h: number) => void, deps: unknown[]) {
  const ref = useRef<HTMLCanvasElement>(null);
  useEffect(() => {
    const c = ref.current;
    if (!c) return;
    const dpr = window.devicePixelRatio || 1;
    const { width, height } = c.getBoundingClientRect();
    c.width = Math.round(width * dpr);
    c.height = Math.round(height * dpr);
    const ctx = c.getContext("2d")!;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, width, height);
    draw(ctx, width, height);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, deps);
  return ref;
}

export function Histogram({ data, clipping, onToggleClipping }: { data: ScopeData; clipping: boolean; onToggleClipping: () => void }) {
  const ref = useCanvas(
    (ctx, w, h) => {
      const n = data.hist_l.length;
      // Scale to the tallest interior bin so a few pure-black/white pixels don't flatten the graph.
      const interior = (a: number[]) => a.slice(2, n - 2);
      const peak = Math.max(1, ...interior(data.hist_r), ...interior(data.hist_g), ...interior(data.hist_b), ...interior(data.hist_l));
      const y = (v: number) => h - Math.min(1, v / peak) * (h - 4);
      const area = (a: number[], color: string) => {
        ctx.beginPath();
        ctx.moveTo(0, h);
        a.forEach((v, i) => ctx.lineTo((i / (n - 1)) * w, y(v)));
        ctx.lineTo(w, h);
        ctx.closePath();
        ctx.fillStyle = color;
        ctx.fill();
      };
      // zone dividers (shadows | midtones | highlights) and the ideal midtone band
      ctx.fillStyle = "rgba(143,199,154,0.09)";
      ctx.fillRect(0.36 * w, 0, 0.24 * w, h);
      ctx.strokeStyle = "rgba(255,255,255,0.08)";
      for (const g of [1 / 3, 2 / 3]) {
        ctx.beginPath();
        ctx.moveTo(g * w, 0);
        ctx.lineTo(g * w, h);
        ctx.stroke();
      }
      ctx.globalCompositeOperation = "lighter";
      area(data.hist_r, "rgba(230,60,60,0.55)");
      area(data.hist_g, "rgba(60,200,80,0.5)");
      area(data.hist_b, "rgba(70,110,255,0.6)");
      ctx.globalCompositeOperation = "source-over";
      ctx.beginPath();
      data.hist_l.forEach((v, i) => (i ? ctx.lineTo((i / (n - 1)) * w, y(v)) : ctx.moveTo(0, y(v))));
      ctx.strokeStyle = "rgba(255,255,255,0.85)";
      ctx.lineWidth = 1.2;
      ctx.stroke();
      // this photo's midtones
      const mx = data.median * w;
      ctx.setLineDash([3, 3]);
      ctx.strokeStyle = "#fff";
      ctx.beginPath();
      ctx.moveTo(mx, 0);
      ctx.lineTo(mx, h);
      ctx.stroke();
      ctx.setLineDash([]);
    },
    [data],
  );

  // Hover: explain the zone under the pointer and how much of the photo is in it.
  const [hover, setHover] = useState<{ zone: "shadows" | "midtones" | "highlights"; x: number } | null>(null);
  const zoneShare = (zone: string) => {
    const n = data.hist_l.length;
    const total = data.hist_l.reduce((a, b) => a + b, 0) || 1;
    const [a, b] = zone === "shadows" ? [0, n / 3] : zone === "midtones" ? [n / 3, (2 * n) / 3] : [(2 * n) / 3, n];
    return (data.hist_l.slice(Math.floor(a), Math.floor(b)).reduce((x, y) => x + y, 0) / total) * 100;
  };
  const mid = data.median * 100;
  const midWord = mid < 25 ? "too dark" : mid < 36 ? "a little dark" : mid <= 60 ? "balanced" : mid <= 70 ? "a little bright" : "too bright";
  const verdict =
    mid < 25 ? { cls: "under", text: "Too dark" } : mid < 36 ? { cls: "under soft", text: "A little dark" }
    : mid <= 60 ? { cls: "good", text: "Exposure looks right" } : mid <= 70 ? { cls: "over soft", text: "A little bright" } : { cls: "over", text: "Too bright" };

  const hi = data.clip_high >= 0.3;
  const lo = data.clip_low >= 0.3;
  return (
    <div className="histo">
      <div
        className="histo-frame"
        onMouseMove={(e) => {
          const r = e.currentTarget.getBoundingClientRect();
          const f = (e.clientX - r.left) / r.width;
          setHover({ zone: f < 1 / 3 ? "shadows" : f < 2 / 3 ? "midtones" : "highlights", x: e.clientX - r.left });
        }}
        onMouseLeave={() => setHover(null)}
      >
        <canvas ref={ref} className="histo-canvas" aria-label="Histogram" />
        <span className="mid-label" style={{ left: `${Math.min(80, Math.max(2, mid))}%` }}>Midtones {mid.toFixed(0)}%</span>
        {hover && (
          <div className={`histo-tip ${hover.x > 200 ? "right" : ""}`} role="tooltip">
            <strong>{hover.zone[0].toUpperCase() + hover.zone.slice(1)}</strong> · {zoneShare(hover.zone).toFixed(0)}% of this photo
            <p>{TIPS[hover.zone]}</p>
            {hover.zone === "midtones" && <p className="mid-now">This photo's midtones are at {mid.toFixed(0)}%: {midWord}. Balanced is about 36–60%.</p>}
          </div>
        )}
        <button className={`clip-tri left ${lo ? "lit" : ""}`} onClick={onToggleClipping} title={`Shadows crushed to black: ${data.clip_low.toFixed(1)}%. Click to show on the photo.`} aria-label="Show crushed shadows" />
        <button className={`clip-tri right ${hi ? "lit" : ""}`} onClick={onToggleClipping} title={`Highlights blown to white: ${data.clip_high.toFixed(1)}%. Click to show on the photo.`} aria-label="Show blown highlights" />
      </div>
      <div className="histo-scale">
        <Tip k="shadows">Shadows</Tip>
        <Tip k="midtones">Midtones</Tip>
        <Tip k="highlights" align="right">Highlights</Tip>
      </div>
      <div className="histo-status">
        <Tip k="exposure"><span className={`verdict ${verdict.cls}`}>{verdict.text}</span></Tip>
        {hi && <span className="chip over">{data.clip_high.toFixed(1)}% blown out</span>}
        {lo && <span className="chip under">{data.clip_low.toFixed(1)}% crushed</span>}
        <button className={`chip toggle ${clipping ? "on" : ""}`} onClick={onToggleClipping} aria-pressed={clipping}>
          {clipping ? "Hide clipping" : "Show clipping"}
        </button>
      </div>
    </div>
  );
}

/** Colour wheel: where the photo's colours sit (angle = hue, distance from centre = intensity),
 *  with one spoke per colour band. Bands that are too strong glow and get a warning. */
export function ColorWheel({ data, selected, onSelect }: { data: ScopeData; selected: number; onSelect: (i: number) => void }) {
  const ref = useCanvas(
    (ctx, w, h) => {
      const size = Math.min(w, h);
      const cx = w / 2;
      const cy = h / 2;
      const R = size / 2 - 10;
      // hue ring
      for (let a = 0; a < 360; a += 2) {
        ctx.beginPath();
        const r0 = ((a - 90 - 1.2) * Math.PI) / 180;
        const r1 = ((a - 90 + 1.2) * Math.PI) / 180;
        ctx.arc(cx, cy, R + 6, r0, r1);
        ctx.strokeStyle = `hsl(${a}, 80%, 52%)`;
        ctx.lineWidth = 5;
        ctx.stroke();
      }
      // dial
      ctx.beginPath();
      ctx.arc(cx, cy, R, 0, Math.PI * 2);
      ctx.fillStyle = "#151515";
      ctx.fill();
      ctx.strokeStyle = "rgba(255,255,255,0.08)";
      for (const f of [0.33, 0.66]) {
        ctx.beginPath();
        ctx.arc(cx, cy, R * f, 0, Math.PI * 2);
        ctx.stroke();
      }
      // density of the photo's colours
      const N = Math.round(Math.sqrt(data.wheel.length));
      const cell = (R * 2) / N;
      for (let j = 0; j < N; j++) {
        for (let i = 0; i < N; i++) {
          const v = data.wheel[j * N + i];
          if (!v) continue;
          const x = (i / (N - 1)) * 2 - 1;
          const y = (j / (N - 1)) * 2 - 1;
          const r = Math.hypot(x, y);
          if (r > 1) continue;
          const hue = ((Math.atan2(x, -y) * 180) / Math.PI + 360) % 360;
          ctx.fillStyle = `hsla(${hue}, ${Math.min(100, 30 + r * 70)}%, ${55 + (1 - r) * 30}%, ${Math.min(1, v / 200)})`;
          ctx.fillRect(cx - R + i * cell, cy - R + j * cell, cell + 0.6, cell + 0.6);
        }
      }
      // spokes
      data.bands.forEach((b, i) => {
        if (b.share < 0.01) return;
        const a = ((b.hue - 90) * Math.PI) / 180;
        const len = R * Math.min(1, b.strength);
        ctx.beginPath();
        ctx.moveTo(cx, cy);
        ctx.lineTo(cx + Math.cos(a) * len, cy + Math.sin(a) * len);
        ctx.strokeStyle = b.over ? "#ff5a4e" : i === selected ? "#ffffff" : "rgba(255,255,255,0.45)";
        ctx.lineWidth = b.over ? 3 : i === selected ? 2 : 1.2;
        ctx.stroke();
        if (b.over) {
          ctx.beginPath();
          ctx.arc(cx + Math.cos(a) * len, cy + Math.sin(a) * len, 5, 0, Math.PI * 2);
          ctx.fillStyle = "#ff5a4e";
          ctx.shadowColor = "#ff5a4e";
          ctx.shadowBlur = 10;
          ctx.fill();
          ctx.shadowBlur = 0;
        }
      });
    },
    [data, selected],
  );

  const click = (e: React.MouseEvent<HTMLCanvasElement>) => {
    const r = e.currentTarget.getBoundingClientRect();
    const x = e.clientX - r.left - r.width / 2;
    const y = e.clientY - r.top - r.height / 2;
    const hue = ((Math.atan2(x, -y) * 180) / Math.PI + 360) % 360;
    let best = 0;
    let bd = 999;
    data.bands.forEach((b, i) => {
      const d = Math.min(Math.abs(hue - b.hue), 360 - Math.abs(hue - b.hue));
      if (d < bd) {
        bd = d;
        best = i;
      }
    });
    onSelect(best);
  };

  return <canvas ref={ref} className="wheel-canvas" onClick={click} aria-label="Colour wheel" />;
}
