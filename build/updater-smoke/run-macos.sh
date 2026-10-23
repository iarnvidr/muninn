#!/usr/bin/env bash
# Real debug Tauri updater integration. Requires hosted Apple Silicon macOS.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
: "${MUNINN_UPDATER_SMOKE_APP:?set to built debug Muninn.app}"
[[ "$(uname -s)" == Darwin && "$(uname -m)" == arm64 ]] || { echo 'FAIL: Apple Silicon macOS required' >&2; exit 2; }
[[ -d "$MUNINN_UPDATER_SMOKE_APP/Contents/MacOS" ]] || { echo 'FAIL: expected actual .app bundle' >&2; exit 2; }
TMP="$(mktemp -d "${TMPDIR:-/tmp}/muninn-updater-smoke.XXXXXX")"; chmod 700 "$TMP"
cleanup() { [[ "${MUNINN_UPDATER_SMOKE_KEEP:-0}" == 1 ]] || rm -rf "$TMP"; }
trap cleanup EXIT
mkdir -p "$TMP/data" "$TMP/feed" "$TMP/e2e" "$TMP/home"
export HOME="$TMP/home" MUNINN_DATA_DIR="$TMP/data" MUNINN_E2E_DIR="$TMP/e2e"
export MUNINN_NATIVE_UI_SMOKE=1
PORT="$(python3 -c 'import socket;s=socket.socket();s.bind(("127.0.0.1",0));print(s.getsockname()[1]);s.close()')"
export MUNINN_UPDATER_SMOKE_ENDPOINT="http://127.0.0.1:$PORT/latest.json"
APP="$(cd "$(dirname "$MUNINN_UPDATER_SMOKE_APP")" && pwd)/$(basename "$MUNINN_UPDATER_SMOKE_APP")"
PLIST="$APP/Contents/Info.plist"
OLD="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$PLIST")"
[[ "$OLD" =~ ^[0-9]+\.[0-9]+\.[0-9]+([.-][A-Za-z0-9.-]+)?$ ]] || { echo 'FAIL: invalid installed bundle version' >&2; exit 2; }
# Isolated ephemeral minisign key; no key material is printed.
npx --yes @tauri-apps/cli@2.10.1 signer generate --ci --write-keys "$TMP/test.key" >"$TMP/keygen.log" 2>&1
PUB="$(find "$TMP" -maxdepth 1 -type f -name '*.key.pub' -print -quit)"
[[ -s "$PUB" ]] || { echo 'FAIL: signer did not produce public key' >&2; exit 2; }
export MUNINN_UPDATER_SMOKE_PUBKEY="$(cat "$PUB")"
# Tauri updater expects latest.json and a .app.tar.gz archive plus detached sig.
# Create separately versioned bundle copy; payload remains the real debug app.
NEW="0.2.14-smoke.2"
make_archive() {
  local version="$1" out="$2" copy="$TMP/Muninn-$version.app"
  ditto "$APP" "$copy"
  /usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $version" "$copy/Contents/Info.plist"
  codesign --force --deep --sign - "$copy"
  mkdir -p "$out"
  ditto -c -k --sequesterRsrc --keepParent "$copy" "$out/Muninn.app.tar.gz"
  npx --yes @tauri-apps/cli@2.10.1 signer sign --private-key-path "$TMP/test.key" --password '' "$out/Muninn.app.tar.gz" > "$out/signature"
}
mkdir -p "$TMP/old" "$TMP/new"
# Old archive only serves as independently checked version evidence.
cp "$APP/Contents/Info.plist" "$TMP/old/Info.plist"
printf '{"version":"%s","signature":"old-fixture"}\n' "$OLD" > "$TMP/old.json"
make_archive "$NEW" "$TMP/new"
SIG="$(cat "$TMP/new/signature")"
python3 - "$TMP" "$NEW" "$SIG" "$PORT" <<'PY'
import json,pathlib,sys
p=pathlib.Path(sys.argv[1]); version,sig,port=sys.argv[2:]
(p/'new.json').write_text(json.dumps({'version':version,'signature':sig})+'\n')
(p/'feed/latest.json').write_text(json.dumps({'version':version,'notes':'synthetic test','pub_date':None,'platforms':{'darwin-aarch64':{'url':f'http://127.0.0.1:{port}/Muninn.app.tar.gz','signature':sig}}})+'\n')
PY
cp "$TMP/new/Muninn.app.tar.gz" "$TMP/feed/Muninn.app.tar.gz"
python3 "$ROOT/build/updater-smoke/verify_artifacts.py" "$TMP"
# loopback-only server, shared by actual plugin fetches
python3 -m http.server "$PORT" --bind 127.0.0.1 --directory "$TMP/feed" >"$TMP/http.log" 2>&1 & SERVER=$!
trap 'kill "$SERVER" 2>/dev/null || true; cleanup' EXIT
sleep 1
# Run actual installed bundle; launcher itself accesses bridge via E2E command files.
python3 "$ROOT/build/updater-smoke/driver.py" healthy "$TMP" "$APP"
# Tamper archive after healthy run, then switch metadata back to original signature/version.
python3 - "$TMP" "$OLD" <<'PY'
import json,pathlib,sys
p=pathlib.Path(sys.argv[1]); version=sys.argv[2]
archive=p/'feed/Muninn.app.tar.gz'; b=bytearray(archive.read_bytes()); b[len(b)//2]^=1; archive.write_bytes(b)
feed=json.loads((p/'feed/latest.json').read_text()); feed['version']=version; (p/'feed/latest.json').write_text(json.dumps(feed))
(p/'old.json').write_text(json.dumps({'version':version})+'\n')
PY
python3 "$ROOT/build/updater-smoke/driver.py" tampered "$TMP" "$APP"
python3 "$ROOT/build/updater-smoke/verify_evidence.py" "$TMP"
echo 'PASS: native updater smoke; redacted evidence generated in temporary workspace'
