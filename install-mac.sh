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

say "Installing JoshGrade. This takes about 15-25 minutes the first time. Keep this window open."

# 1. Apple command line tools (gives git and the compiler)
if ! xcode-select -p >/dev/null 2>&1; then
  say "Step 1/5: Apple developer tools are needed. A window will pop up - click Install."
  xcode-select --install || true
  echo
  echo "When that install finishes, paste the same command into Terminal again."
  exit 0
fi
say "Step 1/5: Apple developer tools are installed."

# 2. Rust
if [ -f "$HOME/.cargo/env" ]; then . "$HOME/.cargo/env"; fi
if ! command -v cargo >/dev/null 2>&1; then
  say "Step 2/5: Installing Rust..."
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
  . "$HOME/.cargo/env"
else
  say "Step 2/5: Rust is installed ($(rustc --version))."
fi

# 3. Node.js (kept private to JoshGrade if you don't already have it)
if ! command -v npm >/dev/null 2>&1 && [ -x "$TOOLS/node/bin/npm" ]; then
  export PATH="$TOOLS/node/bin:$PATH"
fi
if ! command -v npm >/dev/null 2>&1; then
  say "Step 3/5: Installing Node.js..."
  case "$(uname -m)" in arm64) NARCH=arm64 ;; *) NARCH=x64 ;; esac
  mkdir -p "$TOOLS"
  curl -fsSL "https://nodejs.org/dist/v$NODE_VERSION/node-v$NODE_VERSION-darwin-$NARCH.tar.gz" -o "$TOOLS/node.tgz"
  rm -rf "$TOOLS/node" && mkdir -p "$TOOLS/node"
  tar -xzf "$TOOLS/node.tgz" -C "$TOOLS/node" --strip-components 1
  rm -f "$TOOLS/node.tgz"
  export PATH="$TOOLS/node/bin:$PATH"
else
  say "Step 3/5: Node.js is installed ($(node --version))."
fi

# 4. Get the code
if [ -d "$SRC/.git" ]; then
  say "Step 4/5: Updating JoshGrade code..."
  git -C "$SRC" pull --ff-only
else
  say "Step 4/5: Downloading JoshGrade code..."
  git clone --depth 1 "$REPO" "$SRC"
fi
cd "$SRC"

# 5. Build
say "Step 5/5: Building JoshGrade (the long part - lots of text will scroll by, that's normal)..."
npm ci --no-audit --no-fund
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
