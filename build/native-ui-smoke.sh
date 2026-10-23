#!/usr/bin/env bash
# Hosted macOS WKWebView interaction smoke; requires a DEBUG .app bundle.
set -euo pipefail
app="${1:?usage: native-ui-smoke.sh /path/to/release/Muninn.app [evidence-dir]}"
evidence="${2:-${NATIVE_UI_EVIDENCE_DIR:-${RUNNER_TEMP:-${TMPDIR:-/tmp}}/muninn-debug-ui-evidence}}"
[[ "$(uname -s)" == Darwin && "$(uname -m)" == arm64 ]] || { echo 'requires hosted Apple Silicon macOS' >&2; exit 2; }
[[ -d "$app/Contents/MacOS" ]] || { echo "debug app bundle missing: $app" >&2; exit 2; }
mkdir -p "$evidence/reports" "$evidence/screenshots" "$evidence/logs"
evidence="$(cd "$evidence" && pwd)"
app="$(cd "$(dirname "$app")" && pwd)/$(basename "$app")"
# e2e.rs is cfg(debug_assertions)-only. Fail closed if the bridge marker is absent.
exe="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$app/Contents/Info.plist")"
binary="$app/Contents/MacOS/$exe"
[[ -x "$binary" ]] || { echo "bundle executable missing: $binary" >&2; exit 2; }
# Use the release bundle only as a metadata/resource template; substitute the debug executable.
debug_binary="${CARGO_TARGET_DIR:-target}/aarch64-apple-darwin/debug/muninn"
[[ -x "$debug_binary" ]] || { echo "debug executable missing: $debug_binary" >&2; exit 2; }
cp "$debug_binary" "$binary"
codesign --force --sign - "$app"
codesign --verify --deep --strict "$app"
if ! python3 -c 'import subprocess,sys; sys.exit(0 if b"native-ui-smoke" in subprocess.check_output(["strings", sys.argv[1]]) else 1)' "$binary"; then
  echo 'bundle executable does not contain the debug native UI smoke marker' >&2
  exit 2
fi
# Keep this run isolated from the developer/runner account's application data.
scratch="$(mktemp -d "${TMPDIR:-/tmp}/muninn-native-ui-smoke.XXXXXX")"
export HOME="$scratch/home"
export MUNINN_DATA_DIR="$scratch/data"
mkdir -p "$HOME" "$MUNINN_DATA_DIR"
export MUNINN_E2E_DIR="$scratch/e2e"
export MUNINN_NATIVE_UI_SMOKE=1
# AVFoundation generates the synthetic fixture: bundled FFmpeg builds may omit
# lavfi/testsrc/sine. Keep the pinned bundled FFmpeg solely for decode validation.
export NATIVE_SMOKE_MEDIA_FILE="$scratch/native-smoke-30s.mp4"
xcrun swift build/generate-native-fixture.swift "$NATIVE_SMOKE_MEDIA_FILE" | tee "$evidence/reports/media-fixture.txt"
[[ -s "$NATIVE_SMOKE_MEDIA_FILE" ]] || { echo 'AVFoundation did not produce native smoke media' >&2; exit 2; }
ffmpeg="$app/Contents/Resources/bin/ffmpeg"
[[ -f "$ffmpeg" && ! -L "$ffmpeg" && -x "$ffmpeg" ]] || { echo "bundled ffmpeg missing/not executable: $ffmpeg" >&2; exit 2; }
if ! "$ffmpeg" -hide_banner -loglevel error -v error -i "$NATIVE_SMOKE_MEDIA_FILE" -map 0:v:0 -f null -; then
  echo 'bundled ffmpeg could not decode AVFoundation native smoke fixture' >&2
  exit 2
fi
# Invoke directly so the controlled environment reaches the app process.
"$binary" --native-ui-smoke >"$evidence/logs/open.log" 2>&1 &
app_pid=$!
cleanup() { kill "$app_pid" >/dev/null 2>&1 || true; wait "$app_pid" >/dev/null 2>&1 || true; rm -rf "$scratch"; }
trap cleanup EXIT
sleep 5
set +e
python3 build/native-ui-driver.py >"$evidence/reports/interactions.txt" 2>&1
interaction_status=$?
set -e
cat "$evidence/reports/interactions.txt"
set +e
osascript -e 'tell application "System Events" to key code 123' >"$evidence/logs/accessibility.stdout.log" 2>"$evidence/logs/accessibility.stderr.log"
key_status=$?
set -e
printf 'System Events key attempt status=%s\n' "$key_status" > "$evidence/reports/os-permission.txt"
printf 'stdout: ' >> "$evidence/reports/os-permission.txt"
cat "$evidence/logs/accessibility.stdout.log" >> "$evidence/reports/os-permission.txt"
printf 'stderr: ' >> "$evidence/reports/os-permission.txt"
cat "$evidence/logs/accessibility.stderr.log" >> "$evidence/reports/os-permission.txt"
set +e
screencapture -x "$evidence/screenshots/window.png" 2>"$evidence/logs/screencapture.stderr.log"
screenshot_status=$?
set -e
printf 'Screenshot status=%s\n' "$screenshot_status" > "$evidence/reports/screenshot.txt"
printf 'Debug bundle: %s\n' "$app" | tee "$evidence/reports/run.txt"
if [[ "$interaction_status" != 0 ]]; then exit "$interaction_status"; fi
if [[ "$key_status" != 0 ]]; then exit "$key_status"; fi
if [[ "$screenshot_status" != 0 ]]; then exit "$screenshot_status"; fi
