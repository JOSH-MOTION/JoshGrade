# JoshGrade

Offline automatic photo correction and colour grading for Mac.

Drop in photos (JPEG, PNG, TIFF, WebP, HEIC or camera RAW, including Canon CR3). JoshGrade measures each one, fixes what's wrong, applies a look, and saves the result. Nothing is uploaded; everything runs on your laptop.

## What it fixes on its own

Each photo is measured and corrected using standard editing rules:

| Problem | Rule | What it does |
|---|---|---|
| Colour cast (yellow, blue, green, magenta) | Neutral things should be neutral | Finds whites and greys in the photo and balances them. Cross-checks the whole scene, so a blue sky or a yellow wall isn't mistaken for a cast. |
| Too bright / too dark | Midtones should sit near mid-grey | Brings exposure down or up, but won't brighten a photo that's meant to be dark (stage, night, dark backgrounds) or darken a bright, airy one. |
| Hazy, washed out | Use the full range | Sets a true black and a clean white. |
| Crushed shadows | Keep detail in the darks | Opens up shadows gently. |
| Flat or harsh | Healthy separation | Adds contrast to flat photos, softens very harsh ones. |
| Bright highlights clipping | Highlights roll off, never clip | Smooth highlight recovery for skies, windows and white clothes. |
| Dull or garish colour | Vibrance, not saturation | Lifts muted colour, calms loud colour. |
| Skin | Skin is protected | Detects skin tones and keeps them out of colour shifts, so faces never go orange or grey. |

The app lists exactly what it changed for each photo, under **What JoshGrade fixed**.

## Looks

On top of the correction, pick a look: **Normal** (only the automatic correction, no styling), **Natural**, **Vivid**, **Warm**, **Cinematic**, **Moody**, **Bright & Airy**, **Film**, **Black & White**. Strength goes from 0% (original) to 200%. Double-click any slider to reset it.

## Checking a photo is correct

The **Check** panel measures the photo, so you can see whether it's right instead of guessing:

- **Histogram**: red, green, blue and brightness from shadows (left) to highlights (right). The corner triangles light up when shadows are crushed to black (blue) or highlights are blown to white (red).
- **Exposure verdict**: *Underexposed*, *Exposure looks right* or *Overexposed*, judged from the midtones.
- **Show clipping** (or press **J**): paints blown highlights red and crushed shadows blue on the photo.
- **Colour wheel**: every colour in the photo plotted around a circle. Angle is the colour, distance from the centre is how intense it is. Each of the 8 colour bands has a spoke. A colour that's so strong it's losing detail turns its spoke **red** and gets a **!**. JoshGrade tones those colours down automatically, and the wheel shows how much.
- Switch between **Graded** and **Original** to compare the measurements.
- Hover over **Shadows**, **Midtones**, **Highlights** (or anywhere on the histogram), any slider name or any report line for a plain-language explanation.

The **Report** at the bottom measures the original and the current grade the same way (exposure, blown highlights, crushed shadows, colour cast, contrast, colour intensity, colours losing detail, skin) and updates live as you change things.

## Fine-tuning

With **Auto grade** on, JoshGrade sets the sliders for you: the number shows what it chose (marked *auto*, with a small blue tick under the slider). Move a slider to adjust from there; double-click to go back to the auto value. Turn **Auto grade** off to see the photo without the automatic correction.

Everything can be adjusted:

- **Light**: Exposure, Contrast, Highlights, Shadows
- **Colour**: Temperature, Tint, Vibrance, Saturation
- **Colour mix**: Hue, Saturation and Luminance for Red, Orange, Yellow, Green, Aqua, Blue, Purple and Magenta. Click a colour on the wheel to jump to it.

Each photo keeps its own settings. **Use these settings for all photos** copies the current ones to every photo, and each photo still gets its own automatic correction.

## Saving

Nothing is saved until you click **Save this photo…** (you pick the name and folder) or **Save all photos…** (you pick a folder). Originals are never changed, and existing files are never overwritten.

## Using it with Photoshop

Click **Export LUT for Photoshop** to save the current photo's grade as a `.cube` file. In Photoshop:

1. Layer › New Adjustment Layer › Color Lookup
2. 3DLUT File › Load 3D LUT… and pick the `.cube`

The grade is now a non-destructive layer you can mask or lower in opacity. The LUT works in Premiere, DaVinci Resolve, Final Cut and Lightroom (via Profiles) too. It assumes an sRGB document.

Tick **Also save a LUT for each photo** to get one `.cube` next to every graded file.

## Install

**Easiest (builds it on your Mac):** open Terminal, paste this line and press Return:

```bash
curl -fsSL https://raw.githubusercontent.com/JOSH-MOTION/JoshGrade/main/install-mac.sh | bash
```

It installs what's needed (Apple developer tools, Rust, Node.js), builds JoshGrade and puts it in Applications. Run the same line again any time to update.

**Download instead:** once GitHub Actions is enabled on this account, every update also builds a ready-made .dmg:

1. Go to the [Releases page](https://github.com/JOSH-MOTION/JoshGrade/releases/latest) and download **JoshGrade_…_universal.dmg**. It runs on Apple Silicon and Intel Macs.
2. Open the .dmg and drag **JoshGrade** into **Applications**.
3. First launch only: macOS says it can't verify the developer, because the app isn't registered with Apple. Open **System Settings › Privacy & Security**, scroll down and click **Open Anyway**.

Share the same .dmg link with anyone who wants it. Nobody needs to install Rust or anything else.

## How new versions are built

Every push to `main` builds the Mac app on GitHub (see `.github/workflows/build-mac.yml`) and publishes a new release with the .dmg. You can also start a build by hand from the **Actions** tab › **Build Mac app** › **Run workflow**.

To change the version number, edit `version` in `src-tauri/tauri.conf.json`.

### Building on your own Mac (optional, for development)

Needs Rust (`curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`) and Node.js 18+. Then:

```bash
npm install
npm run app:dev   # run with live reload
npm run app       # build JoshGrade.app + .dmg into target/release/bundle
```

## Command line

There's also a command-line version for big batches:

```bash
npm run cli
./target/release/joshgrade-cli --look cinematic --strength 0.8 --out ~/Desktop/Graded ~/Pictures/Wedding
./target/release/joshgrade-cli --looks          # list looks
```

Options: `--look ID`, `--strength 0..1.5`, `--out DIR`, `--format jpeg|tiff|png`, `--lut`.

## Output

- **JPEG** (quality 95). When the original is a JPEG, the camera data (EXIF: date, camera, lens, orientation) and colour profile are copied across.
- **TIFF 16-bit** / **PNG 16-bit** for further editing without banding. Recommended for RAW files.
- Single photos are saved where you choose. "Save all" names files `name_graded.jpg` in the folder you pick. Originals are never touched.

## How it's built

```
crates/grade-core   the engine (Rust): decode, analyse, grade, export LUT
  decode.rs         JPEG/PNG/TIFF/WebP via `image`; RAW via `rawler` (incl. Canon CR3)
  analyze.rs        the editing rules – read this to see every decision
  grade.rs          turns the decisions into one per-pixel colour transform
  looks.rs          the looks (each is a handful of numbers; easy to add more)
  lut.rs            .cube export
crates/grade-cli    command-line tool
src-tauri           desktop app backend (Tauri 2)
src                 desktop app interface (React + TypeScript)
```

Tauri uses the Mac's built-in WebKit instead of bundling Chrome, so the app is a few MB rather than 100+ MB.

Because every correction is a single per-pixel transform, the on-screen preview, the saved photo and the exported LUT always match.

## Limits

- Blown-out areas (pure white in the original) can't be recovered; the app tells you when a photo has a lot of them.
- Corrections are global (whole-photo). It doesn't do local edits like brushing a face brighter.
- HEIC (iPhone) files open on Mac only (through macOS's built-in converter).
- RAW support comes from `rawler` (LGPL-2.1). A few very new camera models may not be supported yet.

## Next step: the trained grade

Version 2 adds a small AI model trained on MIT-Adobe FiveK (5,000 photos each edited by professional retouchers) that predicts a per-photo LUT. It plugs into `grade.rs` as an extra stage and runs offline.
