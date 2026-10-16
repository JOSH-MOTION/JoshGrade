import { ReactNode } from "react";

/** Plain-language explanations shown when you hover (or tab to) a term. */
export const TIPS: Record<string, string> = {
  shadows: "Shadows are the darkest parts of the photo: hair, deep folds in clothes, dark backgrounds. If the graph is piled up against the left edge, detail there has turned pure black.",
  midtones: "Midtones are the middle brightness: most skin, clothes and walls. In a well-exposed photo most of the graph sits here. Hump too far right = too bright, too far left = too dark.",
  highlights: "Highlights are the brightest parts: white clothes, sky, lamps, shine on skin. If the graph is piled against the right edge, those areas are blown to pure white and have no detail.",
  histogram: "The histogram shows how much of the photo is at each brightness, from black (left) to white (right). The shaded band is where the midtones of a well-exposed photo usually sit; the white line marks this photo's midtones.",
  wheel: "Every colour in the photo, placed around a circle. Angle = which colour, distance from the centre = how intense. A colour that's so strong it's losing detail turns red with a ! and is toned down automatically.",
  exposure: "How bright the photo is overall, measured from the midtones. About 36–60% is balanced for most photos (a bright studio backdrop can sit a little higher).",
  blown: "Parts of the photo that are pure white, with no detail left. A little on lamps or reflections is fine; large areas on skin or clothes look bad.",
  crushed: "Parts of the photo that are pure black, with no detail left. Fine for a deliberately black background, bad on hair or dark clothes.",
  cast: "Whether things that should be neutral (white walls, grey backdrops, white clothes) have a colour tint. Neutral means the white balance is right.",
  contrast: "The difference between the darks and the lights. Flat photos look grey and dull; very punchy photos lose detail at both ends.",
  colour: "How intense the colours are on average. Muted looks washed out, very rich can look unreal.",
  detail: "How many colours are so intense they've turned into flat blobs with no texture (for example a red dress with no visible folds).",
  skin: "How intense skin tones are. Too high looks sunburnt or orange, too low looks grey or ill.",
  s_auto: "When on, JoshGrade measures each photo and sets these sliders for you. Move any slider to adjust its choice; the small mark under each slider shows what auto chose. Double-click a slider to go back to the auto value.",
  s_exposure: "Overall brightness of the photo, like opening or closing the lens.",
  s_contrast: "Makes darks darker and lights lighter (right) or brings them closer together (left).",
  s_highlights: "Brightness of only the brightest areas. Move left to bring back detail in skies, white clothes and shiny skin.",
  s_shadows: "Brightness of only the darkest areas. Move right to see detail in hair and dark clothes.",
  s_temp: "Warm (yellow) or cool (blue). Use it so whites look white, or to set a mood.",
  s_tint: "Green or magenta. Fixes the green cast from fluorescent or LED light.",
  s_vibrance: "Boosts dull colours more than strong ones and protects skin. The safe way to add colour.",
  s_saturation: "Makes every colour stronger or weaker by the same amount. −100 is black & white.",
  s_strength: "How strongly the look and corrections are applied. 0% is the original photo, 100% is as designed, above 100% exaggerates it.",
  b_hue: "Shifts this colour towards its neighbours, e.g. orange towards red or towards yellow. Useful for skin tones.",
  b_sat: "How intense this one colour is, without touching the others.",
  b_lum: "How bright this one colour is, without touching the others.",
};

export function Tip({ k, children, align = "left" }: { k: string; children: ReactNode; align?: "left" | "right" }) {
  return (
    <span className="tip" tabIndex={0}>
      {children}
      <span className={`tip-box ${align}`} role="tooltip">{TIPS[k]}</span>
    </span>
  );
}
