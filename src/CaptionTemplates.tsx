import { CSSProperties, useEffect, useState } from "react";

export type Mode = "static" | "highlight" | "highlight_scale" | "highlight_cycle" | "karaoke" | "build" | "single";
export type Anim = "none" | "fade" | "slow_fade" | "pop" | "bounce" | "slide_up" | "neon" | "glitch";
export type Template = {
  id: string; name: string; category: string; description: string;
  font: string; bold: boolean; italic: boolean; uppercase: boolean; size: number;
  color: string; highlight: string; cycle: string[]; mode: Mode; anim: Anim;
  outline: string; outline_w: number; shadow: number; shadow_color: string; shadow_alpha: number;
  box_color: string | null; box_alpha: number; glow: string; spacing: number; tilt: number;
  position: string; char_w: number; em: number; max_words: number; max_lines: number; max_chars: number;
};
export type Word = { text: string; start: number; end: number; prob: number };
export type Cue = { start: number; end: number; text: string; words: Word[] };

const rgba = (hex: string, a: number) => {
  const v = parseInt(hex.replace("#", ""), 16);
  return `rgba(${(v >> 16) & 255},${(v >> 8) & 255},${v & 255},${a})`;
};

/** A ring of shadows = an outline that works everywhere (no text-stroke quirks). */
function outlineShadow(color: string, r: number) {
  if (r <= 0.2) return [];
  const steps = 16;
  return Array.from({ length: steps }, (_, i) => {
    const a = (i / steps) * Math.PI * 2;
    return `${(Math.cos(a) * r).toFixed(1)}px ${(Math.sin(a) * r).toFixed(1)}px 0 ${color}`;
  });
}

/** Same word timing rule as the burn-in: use recognised words if the text still matches, else spread by length. */
function timedWords(cue: Cue) {
  const toks: { w: string; br: boolean }[] = [];
  cue.text.split("\n").forEach((line, li) => line.split(/\s+/).filter(Boolean).forEach((w, wi) => toks.push({ w, br: li > 0 && wi === 0 })));
  if (cue.words.length === toks.length) return toks.map((t, i) => ({ ...t, start: cue.words[i].start, end: cue.words[i].end }));
  const total = toks.reduce((a, t) => a + t.w.length + 1, 0) || 1;
  let t = cue.start;
  const dur = Math.max(0.1, cue.end - cue.start);
  return toks.map((tk) => {
    const d = (dur * (tk.w.length + 1)) / total;
    const r = { ...tk, start: t, end: t + d };
    t += d;
    return r;
  });
}

/**
 * Renders one caption the way the template burns it in.
 * `frameH`/`frameW` = size of the displayed video picture in px; `scale` = user size multiplier.
 */
export function CaptionView({ tpl, cue, t, frameW, frameH, scale, position, cueKey }: {
  tpl: Template; cue: Cue; t: number; frameW: number; frameH: number; scale: number; position: string; cueKey: string | number;
}) {
  const words = timedWords(cue);
  const assPx = Math.max(6, (tpl.size * scale / 100) * Math.min(frameW, frameH));
  const fontPx = assPx * tpl.em;
  const pos = position || tpl.position;
  let active = -1;
  words.forEach((w, i) => { if (t >= w.start) active = i; });
  if (active < 0) active = 0;

  const shadows: string[] = [];
  // Neon's thin "tube" outline is the glow colour, like the burn-in.
  if (!tpl.box_color) shadows.push(...outlineShadow(tpl.anim === "neon" ? tpl.glow : tpl.outline, tpl.outline_w * assPx));
  if (tpl.shadow > 0 && !tpl.box_color) shadows.push(`${(tpl.shadow * assPx).toFixed(1)}px ${(tpl.shadow * assPx).toFixed(1)}px 0 ${rgba(tpl.shadow_color, tpl.shadow_alpha / 255)}`);
  if (tpl.anim === "neon") shadows.push(`0 0 ${assPx * 0.08}px ${tpl.glow}`, `0 0 ${assPx * 0.2}px ${tpl.glow}`, `0 0 ${assPx * 0.4}px ${tpl.glow}`, `0 0 ${assPx * 0.7}px ${tpl.glow}`);

  const text: CSSProperties = {
    fontFamily: `"${tpl.font}", Arial, sans-serif`,
    fontWeight: tpl.bold ? 700 : 400,
    fontStyle: tpl.italic ? "italic" : "normal",
    fontSize: fontPx,
    lineHeight: 1.18,
    color: tpl.color,
    textTransform: tpl.uppercase ? "uppercase" : "none",
    letterSpacing: tpl.spacing * assPx,
    textShadow: shadows.join(","),
    whiteSpace: "pre-line",
    textAlign: tpl.mode === "build" ? "left" : "center",
  };
  if (tpl.box_color) {
    Object.assign(text, {
      background: rgba(tpl.box_color, tpl.box_alpha / 255),
      padding: `${assPx * 0.06}px ${assPx * 0.22}px`,
      boxDecorationBreak: "clone",
      WebkitBoxDecorationBreak: "clone",
      borderRadius: tpl.id === "bubble" ? assPx * 0.12 : 2,
    } as CSSProperties);
  }

  const wordEls = words.map((w, i) => {
    const sp = i > 0 ? (w.br ? "\n" : " ") : "";
    let style: CSSProperties | undefined;
    let cls = "";
    let content: string | null = w.w;
    switch (tpl.mode) {
      case "highlight":
        if (i === active) style = { color: tpl.highlight };
        break;
      case "highlight_scale":
      case "highlight_cycle": {
        if (i === active) {
          const c = tpl.mode === "highlight_cycle" && tpl.cycle.length ? tpl.cycle[active % tpl.cycle.length] : tpl.highlight;
          // Real size change (the line reflows, like the burn-in), plus a quick pop.
          style = { color: c, display: "inline-block", fontSize: "1.12em", lineHeight: 1 };
          cls = "w-pop";
        }
        break;
      }
      case "karaoke": {
        const next = words[i + 1]?.start ?? cue.end - 0.15;
        const p = Math.max(0, Math.min(1, (t - w.start) / Math.max(0.05, next - w.start)));
        if (p >= 1) style = { color: tpl.highlight };
        else if (p > 0) {
          // Partly sung: a gold copy laid over the word, revealed left to right.
          return (
            <span key={i}>
              {sp}
              <span style={{ position: "relative", display: "inline-block" }}>
                {w.w}
                <span aria-hidden style={{ position: "absolute", left: 0, top: 0, color: tpl.highlight, clipPath: `inset(0 ${(1 - p) * 100}% 0 0)` }}>{w.w}</span>
              </span>
            </span>
          );
        }
        break;
      }
      case "build":
        if (i > active) content = null;
        break;
      case "single":
        if (i !== active) content = null;
        break;
    }
    if (content === null) return null;
    return (
      <span key={`${i}-${tpl.mode === "highlight_scale" || tpl.mode === "highlight_cycle" ? (i === active ? "a" : "n") : ""}`}>
        {tpl.mode === "single" ? "" : sp}
        <span className={cls} style={style}>{content}</span>
      </span>
    );
  });

  const posStyle: CSSProperties =
    pos === "top" ? { top: frameH * 0.06 }
      : pos === "middle" ? { top: "50%", transform: "translateY(-50%)" }
        : pos === "lower" ? { bottom: frameH * 0.25 }
          : { bottom: frameH * 0.06 };
  const animKey = tpl.mode === "single" ? `${cueKey}-${active}` : `${cueKey}`;
  return (
    <div className="tpl-anchor" style={posStyle}>
      <div key={animKey} className={`tpl-anim a-${tpl.anim}`} style={{ transform: tpl.tilt ? `rotate(${tpl.tilt}deg)` : undefined }}>
        <span style={text}>{wordEls}</span>
      </div>
    </div>
  );
}

const SAMPLE: Cue = {
  start: 0, end: 2.6, text: "Make it look good",
  words: [
    { text: "Make", start: 0.1, end: 0.5, prob: 1 },
    { text: "it", start: 0.55, end: 0.75, prob: 1 },
    { text: "look", start: 0.8, end: 1.3, prob: 1 },
    { text: "good", start: 1.35, end: 2.2, prob: 1 },
  ],
};

/** Gallery tile: plays the template's animation on a loop while hovered (or selected). */
export function TemplateTile({ tpl, selected, onPick }: { tpl: Template; selected: boolean; onPick: () => void }) {
  const [hover, setHover] = useState(false);
  const [t, setT] = useState(1.0);
  const [loop, setLoop] = useState(0);
  const playing = hover || selected;
  useEffect(() => {
    if (!playing) {
      setT(1.0);
      return;
    }
    const start = performance.now();
    let raf = 0;
    const tick = () => {
      const e = ((performance.now() - start) / 1000) % 3.0;
      setT(e);
      setLoop(Math.floor((performance.now() - start) / 3000));
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [playing]);
  const cue = tpl.mode === "single" || tpl.max_words === 2 ? { ...SAMPLE, text: "Make it look good" } : SAMPLE;
  return (
    <button
      className={`tpl-tile ${selected ? "on" : ""}`}
      onClick={onPick}
      onMouseEnter={() => setHover(true)}
      onMouseLeave={() => setHover(false)}
      onFocus={() => setHover(true)}
      onBlur={() => setHover(false)}
      title={tpl.description}
      aria-pressed={selected}
    >
      <span className="tpl-stage">
        {t <= 2.6 && (
          <CaptionView
            tpl={tpl}
            cue={cue}
            t={t}
            frameW={104}
            frameH={62}
            // tiles show every template at a readable size, whatever its real size
            scale={(tpl.mode === "single" ? 30 : 22) / ((tpl.size / 100) * 62)}
            position="middle"
            cueKey={loop}
          />
        )}
      </span>
      <span className="tpl-name">{tpl.name}</span>
    </button>
  );
}
