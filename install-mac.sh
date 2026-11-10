#!/bin/bash
# JoshGrade one-line installer for Mac.
# Builds JoshGrade from source on this Mac and puts it in Applications.
#
#   curl -fsSL https://raw.githubusercontent.com/JOSH-MOTION/JoshGrade/main/install-mac.sh | bash
#
# Safe to run again: it updates the code and rebuilds.
set -e

REPO="https://github.com/JOSH-MOTION/JoshGrade.git"
SRC="$HOME/JoshGrade"
NODE_VERSION="20.18.1"
TOOLS="$HOME/.joshgrade-tools"

say()  { printf "\n\033[1;34m==>\033[0m \033[1m%s\033[0m\n" "$1"; }
fail() { printf "\n\033[1;31mProblem:\033[0m %s\n" "$1"; exit 1; }

say "Installing JoshGrade. This takes about 20-30 minutes the first time. Keep this window open."

# 1. Apple command line tools (gives git and the compiler)
if ! xcode-select -p >/dev/null 2>&1; then
  say "Step 1/6: Apple developer tools are needed. A window will pop up - click Install."
  xcode-select --install || true
  echo
  echo "When that install finishes, paste the same command into Terminal again."
  exit 0
fi
say "Step 1/6: Apple developer tools are installed."

# 2. Rust
if [ -f "$HOME/.cargo/env" ]; then . "$HOME/.cargo/env"; fi
if ! command -v cargo >/dev/null 2>&1; then
  say "Step 2/6: Installing Rust..."
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
  . "$HOME/.cargo/env"
else
  say "Step 2/6: Rust is installed ($(rustc --version))."
fi

# 3. Node.js (kept private to JoshGrade if you don't already have it)
if ! command -v npm >/dev/null 2>&1 && [ -x "$TOOLS/node/bin/npm" ]; then
  export PATH="$TOOLS/node/bin:$PATH"
fi
if ! command -v npm >/dev/null 2>&1; then
  say "Step 3/6: Installing Node.js..."
  case "$(uname -m)" in arm64) NARCH=arm64 ;; *) NARCH=x64 ;; esac
  mkdir -p "$TOOLS"
  curl -fsSL "https://nodejs.org/dist/v$NODE_VERSION/node-v$NODE_VERSION-darwin-$NARCH.tar.gz" -o "$TOOLS/node.tgz"
  rm -rf "$TOOLS/node" && mkdir -p "$TOOLS/node"
  tar -xzf "$TOOLS/node.tgz" -C "$TOOLS/node" --strip-components 1
  rm -f "$TOOLS/node.tgz"
  export PATH="$TOOLS/node/bin:$PATH"
else
  say "Step 3/6: Node.js is installed ($(node --version))."
fi

# 4. Get the code
if [ -d "$SRC/.git" ]; then
  say "Step 4/6: Updating JoshGrade code..."
  git -C "$SRC" pull --ff-only
else
  say "Step 4/6: Downloading JoshGrade code..."
  git clone --depth 1 "$REPO" "$SRC"
fi
cd "$SRC"

# 5. Video tools: ffmpeg/ffprobe (bundled inside the app) and cmake (needed to build the caption engine)
say "Step 5/6: Getting the video tools (ffmpeg)..."
TRIPLE="$(rustc -vV | sed -n 's/host: //p')"
case "$(uname -m)" in arm64) FARCH=arm64 ;; *) FARCH=amd64 ;; esac
BIN="$SRC/src-tauri/binaries"
mkdir -p "$BIN"
get_tool() { # name
  local name="$1" dest="$BIN/$1-$TRIPLE"
  if [ -x "$dest" ] && "$dest" -version >/dev/null 2>&1; then return 0; fi
  local tmp; tmp="$(mktemp -d)"
  if curl -fsSL "https://ffmpeg.martin-riedl.de/redirect/latest/macos/$FARCH/release/$name.zip" -o "$tmp/$name.zip" \
     || { [ "$FARCH" = amd64 ] && curl -fsSL "$(curl -fsSL "https://evermeet.cx/ffmpeg/info/$name/release" | sed -n 's/.*"zip":{"url":"\([^"]*\)".*/\1/p')" -o "$tmp/$name.zip"; }; then
    unzip -o -q "$tmp/$name.zip" -d "$tmp" && mv "$tmp/$name" "$dest" && chmod +x "$dest"
  fi
  rm -rf "$tmp"
  [ -x "$dest" ] || fail "Couldn't download $name. Check your internet connection and run the command again."
}
get_tool ffmpeg
get_tool ffprobe
echo "ffmpeg ready: $("$BIN/ffmpeg-$TRIPLE" -version | head -1)"

if ! command -v cmake >/dev/null 2>&1; then
  if [ -x "$TOOLS/cmake/CMake.app/Contents/bin/cmake" ]; then
    export PATH="$TOOLS/cmake/CMake.app/Contents/bin:$PATH"
  else
    echo "Getting cmake (build tool for the caption engine)..."
    mkdir -p "$TOOLS/cmake"
    curl -fsSL "https://github.com/Kitware/CMake/releases/download/v3.31.6/cmake-3.31.6-macos-universal.tar.gz" | tar -xz -C "$TOOLS/cmake" --strip-components 1
    export PATH="$TOOLS/cmake/CMake.app/Contents/bin:$PATH"
  fi
fi
command -v cmake >/dev/null 2>&1 || fail "Couldn't set up cmake. Check your internet connection and run the command again."

# 6. Build
# Face engine for Auto retouch (53 MB, once). The app can also download it later.
FACE_DIR="$HOME/Library/Application Support/app.joshgrade.desktop/face-models"
if [ ! -f "$FACE_DIR/face_parsing_resnet18.onnx" ]; then
  echo "Getting the face engine for Auto retouch (53 MB)..."
  mkdir -p "$FACE_DIR"
  curl -fsSL "https://huggingface.co/yakhyo/uniface-weights/resolve/main/parsing_resnet18.onnx" -o "$FACE_DIR/face_parsing.part" \
    && mv "$FACE_DIR/face_parsing.part" "$FACE_DIR/face_parsing_resnet18.onnx" \
    || { rm -f "$FACE_DIR/face_parsing.part"; echo "(Couldn't get it now - the app will offer to download it.)"; }
fi
# Person + sky engines for Masks (41 MB, once).
for pair in "subject_modnet.onnx https://huggingface.co/Xenova/modnet/resolve/main/onnx/model.onnx" \
            "scene_segformer_b0.onnx https://huggingface.co/Xenova/segformer-b0-finetuned-ade-512-512/resolve/main/onnx/model.onnx"; do
  set -- $pair
  if [ ! -f "$FACE_DIR/$1" ]; then
    echo "Getting the mask engine ($1)..."
    curl -fsSL "$2" -o "$FACE_DIR/$1.part" && mv "$FACE_DIR/$1.part" "$FACE_DIR/$1" \
      || { rm -f "$FACE_DIR/$1.part"; echo "(Couldn't get it now - the app will offer to download it.)"; }
  fi
done

say "Step 6/6: Building JoshGrade (the long part - lots of text will scroll by, that's normal)..."
npm ci --no-audit --no-fund
# Build the caption engine for a common processor baseline (not just this Mac's chip),
# so the .dmg also runs on friends' Macs.
export GGML_NATIVE=OFF
if [ "$(uname -m)" = "x86_64" ]; then
  export GGML_AVX=ON GGML_AVX2=ON GGML_FMA=ON GGML_F16C=ON
fi
# Clear the previous build so a failed build can never reinstall the old app.
rm -rf "$SRC/target/release/bundle/macos/JoshGrade.app" "$SRC/target/release/bundle/dmg"
npx tauri build || echo "(The installer .dmg step had an issue; the app itself is checked next.)"

APP="$SRC/target/release/bundle/macos/JoshGrade.app"
[ -d "$APP" ] || fail "The build didn't produce JoshGrade.app. Scroll up for the first red 'error' line and send it to Claude."

DEST="/Applications"
if ! { rm -rf "$DEST/JoshGrade.app" && cp -R "$APP" "$DEST/"; } 2>/dev/null; then
  DEST="$HOME/Applications"; mkdir -p "$DEST"
  rm -rf "$DEST/JoshGrade.app"; cp -R "$APP" "$DEST/"
fi

DMG=$(ls "$SRC"/target/release/bundle/dmg/*.dmg 2>/dev/null | head -1 || true)

say "Done! JoshGrade is in $DEST."
if [ -n "$DMG" ]; then
  echo "Installer to share with others: $DMG"
  open -R "$DMG"
fi
open "$DEST/JoshGrade.app"
