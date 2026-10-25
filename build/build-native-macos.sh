#!/usr/bin/env bash
# Native Apple Silicon packaging; no Docker, private SDK download or Tauri CLI.
set -euo pipefail
cd "$(dirname "$0")/.."
if [[ "$(uname -s)" != Darwin || "$(uname -m)" != arm64 ]]; then
  echo "Run this script on an Apple Silicon Mac with Xcode Command Line Tools." >&2
  exit 2
fi
out="${1:-$PWD/dist}"
mkdir -p "$out"
out="$(cd "$out" && pwd)"
export MACOSX_DEPLOYMENT_TARGET=14.0
cargo test --locked --workspace
node --test src-tauri/test/*.test.js ui/test/*.test.js
python3 -m unittest discover -s build -p 'test_*.py'
cargo build --release --locked -p muninn --features custom-protocol --target aarch64-apple-darwin
stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
# Fetch only the requested architecture; no Intel downloads or lipo required.
python3 build/fetch_tools.py "$stage/tools" --arch arm64
app="$stage/Muninn.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources/bin"
cp "${CARGO_TARGET_DIR:-target}/aarch64-apple-darwin/release/muninn" "$app/Contents/MacOS/muninn"
cp src-tauri/icons/icon.icns "$app/Contents/Resources/icon.icns"
cp "$stage/tools/arm64/"* "$app/Contents/Resources/bin/"
cp "$stage/tools/NOTICE.txt" "$app/Contents/Resources/bin/NOTICE.txt"
cp -R licenses "$app/Contents/Resources/licenses"
cp LICENSE "$app/Contents/Resources/licenses/Muninn-Apache-2.0.txt"
cp ui/fonts/*-OFL.txt "$app/Contents/Resources/licenses/"
python3 build/license_inventory.py --collect "$app/Contents/Resources/licenses/rust" > "$app/Contents/Resources/licenses/rust-packages.json"
cp THIRD_PARTY_NOTICES.md "$app/Contents/Resources/THIRD_PARTY_NOTICES.md"
python3 build/native_info.py "$app"
# Apple's lipo takes the input file first: `lipo <file> -verify_arch <arch>`.
# (llvm-lipo also accepts the reverse order, so Linux never caught it.)
for tool in "$app/Contents/Resources/bin/"*; do
  [[ -x "$tool" ]] || continue
  codesign --force --sign - "$tool"
  lipo "$tool" -verify_arch arm64
done
codesign --force --sign - "$app"
codesign --verify --deep --strict --verbose=2 "$app" 2>&1 | tee "$out/verify-macos.txt"
lipo "$app/Contents/MacOS/muninn" -verify_arch arm64
version="$(python3 -c 'import json; print(json.load(open("src-tauri/tauri.conf.json"))["version"])')"
archive="Muninn_${version}_macos-arm64.zip"
ditto -c -k --sequesterRsrc --keepParent "$app" "$out/$archive"
(cd "$out" && shasum -a 256 "$archive" > "$archive.sha256")
echo "$out/$archive"
