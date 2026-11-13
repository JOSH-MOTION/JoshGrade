import { useRef } from "react";

/** One colour-grading wheel: drag the dot (angle = hue, distance = strength). */
export type Wheel = { hue: number; sat: number; lum: number };

export function GradeWheel({ label, value, onChange, size = 104 }: { label: string; value: Wheel; onChange: (w: Wheel) => void; size?: number }) {
  const ref = useRef<HTMLDivElement>(null);
  const r = size / 2;
  // hue 0 = red at the top-right like Lightroom's wheels (angle measured counter-clockwise from the right)
  const ang = (value.hue * Math.PI) / 180;
  const rad = (value.sat / 100) * (r - 6);
  const dot = [r + Math.cos(ang) * rad, r - Math.sin(ang) * rad];
  const set = (e: React.PointerEvent) => {
    const b = ref.current!.getBoundingClientRect();
    const x = e.clientX - b.left - r;
    const y = -(e.clientY - b.top - r);
    let hue = (Math.atan2(y, x) * 180) / Math.PI;
    if (hue < 0) hue += 360;
    let sat = Math.min(100, (Math.hypot(x, y) / (r - 6)) * 100);
    // fine control: Shift keeps the hue and changes only the strength
    if (e.shiftKey) hue = value.hue;
    if (sat < 3) sat = 0;
    onChange({ ...value, hue: Math.round(hue), sat: Math.round(sat) });
  };
  const active = value.sat > 0 || value.lum !== 0;
  return (
    <div className="gw">
      <div className="gw-head">
        <span>{label}</span>
        {active && (
          <button className="link" onClick={() => onChange({ hue: value.hue, sat: 0, lum: 0 })} aria-label={`Reset ${label}`}>
            Reset
          </button>
        )}
      </div>
      <div
        ref={ref}
        className="gw-pad"
        style={{ width: size, height: size }}
        role="slider"
        tabIndex={0}
        aria-label={`${label} colour`}
        aria-valuetext={value.sat ? `hue ${value.hue}°, strength ${value.sat}` : "none"}
        onPointerDown={(e) => {
          (e.currentTarget as Element).setPointerCapture(e.pointerId);
          set(e);
        }}
        onPointerMove={(e) => e.buttons === 1 && set(e)}
        onDoubleClick={() => onChange({ ...value, sat: 0 })}
        onKeyDown={(e) => {
          const d = e.shiftKey ? 10 : 2;
          if (e.key === "ArrowUp") onChange({ ...value, sat: Math.min(100, value.sat + d) });
          else if (e.key === "ArrowDown") onChange({ ...value, sat: Math.max(0, value.sat - d) });
          else if (e.key === "ArrowLeft") onChange({ ...value, hue: (value.hue + d + 360) % 360 });
          else if (e.key === "ArrowRight") onChange({ ...value, hue: (value.hue - d + 360) % 360 });
          else return;
          e.preventDefault();
        }}
      >
        <span className="gw-dot" style={{ left: dot[0], top: dot[1], background: value.sat ? `hsl(${value.hue}deg 80% 55%)` : "#ddd" }} />
      </div>
      <input
        className="gw-lum"
        type="range"
        min={-100}
        max={100}
        value={value.lum}
        onChange={(e) => onChange({ ...value, lum: +e.target.value })}
        onDoubleClick={() => onChange({ ...value, lum: 0 })}
        aria-label={`${label} brightness`}
        title={`Brightness ${value.lum > 0 ? "+" : ""}${value.lum}`}
      />
    </div>
  );
}
