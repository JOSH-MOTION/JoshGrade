#!/usr/bin/env bash
# Build the Windows installer (JoshGrade_*_x64-setup.exe) from Linux, without a Windows PC or GitHub Actions.
# Needs: Rust, Node, clang/lld/llvm 19+, nsis, cmake, ninja, cargo-xwin (cargo install --locked cargo-xwin).
# ffmpeg.exe / ffprobe.exe must be in src-tauri/binaries/ as ffmpeg-x86_64-pc-windows-msvc.exe etc.
# (the npm packages @ffmpeg-installer/win32-x64 and @ffprobe-installer/win32-x64 contain them).
set -euo pipefail
cd "$(dirname "$0")/.."
LLVM=/usr/lib/llvm-19/bin
mkdir -p .xbin
ln -sf $LLVM/clang .xbin/clang-cl; ln -sf $LLVM/lld-link .xbin/lld-link; ln -sf $LLVM/llvm-lib .xbin/llvm-lib; ln -sf $LLVM/llvm-rc .xbin/llvm-rc
export PATH="$PWD/.xbin:$PATH"
# Static C runtime, so it runs without the Visual C++ Redistributable
export RUSTFLAGS="-C target-feature=+crt-static -C link-arg=libucrt.lib -C link-arg=libvcruntime.lib -C link-arg=libcmt.lib -C link-arg=libcpmt.lib"
export CMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded CMAKE_POLICY_DEFAULT_CMP0091=NEW
npx tauri build --runner cargo-xwin --target x86_64-pc-windows-msvc
