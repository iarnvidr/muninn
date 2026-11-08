#!/usr/bin/env bash
# Build a second, separately identified app from the same tested PR source tree.
# Never modify the native candidate consumed by release promotion.
set -euo pipefail
cd "$(dirname "$0")/.."
[[ "$(uname -s)" == Darwin && "$(uname -m)" == arm64 ]] || { echo 'Apple Silicon Mac required' >&2; exit 2; }
stable="${1:?usage: build-preview-macos.sh stable-root preview-root corresponding-sources}"
out="${2:?preview-root required}"
sources="${3:?corresponding-sources required}"
pr="${MUNINN_BUILD_PR:?PR number required}"
head="${MUNINN_BUILD_PR_HEAD:?PR head SHA required}"
[[ "$pr" =~ ^[1-9][0-9]*$ && "$head" =~ ^[0-9a-f]{40}$ ]] || { echo 'Invalid PR provenance' >&2; exit 2; }
[[ -d "$stable/Muninn.app" && -d "$sources" ]] || { echo 'Missing verified native app/source inputs' >&2; exit 2; }
mkdir -p "$out"
app="$out/Muninn Preview.app"
ditto "$stable/Muninn.app" "$app"

# The Tauri context controls runtime app_data_dir; Info.plist must match it.
# The preview-specific plist intentionally registers no muninn:// handler.
export TAURI_CONFIG='{"identifier":"io.github.iarnvidr.muninn.preview","productName":"Muninn Preview","bundle":{"macOS":{"infoPlist":"Info.preview.plist"}}}'
export MUNINN_BUILD_CHANNEL=pr-preview
cargo build --release --locked -p muninn --features custom-protocol --target aarch64-apple-darwin
cp "${CARGO_TARGET_DIR:-target}/aarch64-apple-darwin/release/muninn" "$app/Contents/MacOS/muninn"
python3 build/native_info.py "$app"
test "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$app/Contents/Info.plist")" = io.github.iarnvidr.muninn.preview
test "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleDisplayName' "$app/Contents/Info.plist")" = 'Muninn Preview'
if /usr/libexec/PlistBuddy -c 'Print :CFBundleURLTypes' "$app/Contents/Info.plist" >/dev/null 2>&1; then
  echo 'Preview must not register the production URL scheme' >&2
  exit 2
fi
codesign --force --sign - "$app"
codesign --verify --deep --strict "$app"
lipo "$app/Contents/MacOS/muninn" -verify_arch arm64

name="Muninn_PR${pr}_${head:0:12}_macos-arm64"
ditto -c -k --sequesterRsrc --keepParent "$app" "$out/$name.zip"
(cd "$out" && shasum -a 256 "$name.zip" > "$name.zip.sha256")
bash build/package-dmg.sh "$app" "$sources" "$out/$name.dmg"
echo "Isolated PR build: $out/$name.dmg"
