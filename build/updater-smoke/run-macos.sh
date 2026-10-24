#!/usr/bin/env bash
# Real debug Tauri updater integration. Requires hosted Apple Silicon macOS.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
: "${MUNINN_UPDATER_SMOKE_APP:?set to built debug Muninn.app}"
[[ "$(uname -s)" == Darwin && "$(uname -m)" == arm64 ]] || { echo 'FAIL: Apple Silicon macOS required' >&2; exit 2; }
[[ -d "$MUNINN_UPDATER_SMOKE_APP/Contents/MacOS" ]] || { echo 'FAIL: expected actual .app bundle' >&2; exit 2; }
TMP="$(mktemp -d "${TMPDIR:-/tmp}/muninn-updater-smoke.XXXXXX")"; chmod 700 "$TMP"
# Persist only the final boolean evidence document when requested; fixture/key/tmp data stay private and ephemeral.
EVIDENCE_DIR="${MUNINN_UPDATER_SMOKE_EVIDENCE_DIR:-}"
cleanup() {
  local rc=$?
  if [[ -n "$EVIDENCE_DIR" && -s "$TMP/evidence.json" ]]; then
    mkdir -p "$EVIDENCE_DIR"; chmod 700 "$EVIDENCE_DIR"
    install -m 600 "$TMP/evidence.json" "$EVIDENCE_DIR/evidence.json"
  fi
  [[ "${MUNINN_UPDATER_SMOKE_KEEP:-0}" == 1 ]] || rm -rf "$TMP"
  return "$rc"
}
trap cleanup EXIT
mkdir -p "$TMP/data" "$TMP/feed" "$TMP/e2e" "$TMP/home"
# Keep the original debug app untouched; healthy install targets its own copy.
ditto "$MUNINN_UPDATER_SMOKE_APP" "$TMP/healthy-app"
ditto "$MUNINN_UPDATER_SMOKE_APP" "$TMP/tampered-app"
export HOME="$TMP/home" MUNINN_DATA_DIR="$TMP/data" MUNINN_E2E_DIR="$TMP/e2e"
export MUNINN_NATIVE_UI_SMOKE=1
PORT="$(python3 -c 'import socket;s=socket.socket();s.bind(("127.0.0.1",0));print(s.getsockname()[1]);s.close()')"
export MUNINN_UPDATER_SMOKE_ENDPOINT="http://127.0.0.1:$PORT/latest.json"
APP="$(cd "$(dirname "$MUNINN_UPDATER_SMOKE_APP")" && pwd)/$(basename "$MUNINN_UPDATER_SMOKE_APP")"
PLIST="$APP/Contents/Info.plist"
# Use isolated copies, never mutate or launch the caller's source bundle.
OLD="0.2.14-smoke.1"
NEW="0.2.14-smoke.2"

# Isolated ephemeral minisign key; no key material is printed.
npx --yes @tauri-apps/cli@2.10.1 signer generate --ci --write-keys "$TMP/test.key" >"$TMP/keygen.log" 2>&1
PUB="$(find "$TMP" -maxdepth 1 -type f -name '*.key.pub' -print -quit)"
[[ -s "$PUB" ]] || { echo 'FAIL: signer did not produce public key' >&2; exit 2; }
export MUNINN_UPDATER_SMOKE_PUBKEY="$(cat "$PUB")"
# Produce genuine gzip/tar updater payloads rooted at Muninn.app.
make_archive() {
  local version="$1" out="$2" copy="$TMP/Muninn.app"
  rm -rf "$copy"; ditto "$APP" "$copy"
  /usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $version" "$copy/Contents/Info.plist"
  codesign --force --deep --sign - "$copy"
  mkdir -p "$out"
  tar -czf "$out/Muninn.app.tar.gz" -C "$TMP" Muninn.app
  # CLI emits signature in the requested output file; never scrape stdout.
  npx --yes @tauri-apps/cli@2.10.1 signer sign --private-key-path "$TMP/test.key" --password '' "$out/Muninn.app.tar.gz" >"$TMP/sign.log" 2>&1
  [[ -s "$out/Muninn.app.tar.gz.sig" ]] || { echo 'FAIL: signer did not emit detached .sig file' >&2; exit 2; }
  cp "$out/Muninn.app.tar.gz.sig" "$out/signature"
}
mkdir -p "$TMP/old" "$TMP/new"
make_archive "$OLD" "$TMP/old"
make_archive "$NEW" "$TMP/new"
SIG="$(cat "$TMP/new/Muninn.app.tar.gz.sig")"
OLD_SIG="$(cat "$TMP/old/Muninn.app.tar.gz.sig")"

python3 - "$TMP" "$OLD" "$OLD_SIG" "$NEW" "$SIG" "$PORT" <<'PY'
import json,pathlib,sys
p=pathlib.Path(sys.argv[1]); old,old_sig,version,sig,port=sys.argv[2:]
(p/'old.json').write_text(json.dumps({'version':old,'signature':old_sig})+'\n')
(p/'new.json').write_text(json.dumps({'version':version,'signature':sig})+'\n')
(p/'feed/latest.json').write_text(json.dumps({'version':version,'notes':'synthetic test','pub_date':None,'platforms':{'darwin-aarch64':{'url':f'http://127.0.0.1:{port}/Muninn.app.tar.gz','signature':sig}}})+'\n')
PY
cp "$TMP/new/Muninn.app.tar.gz" "$TMP/feed/Muninn.app.tar.gz"
export MUNINN_UPDATER_SMOKE_CURRENT_VERSION="$OLD"
python3 "$ROOT/build/updater-smoke/verify_artifacts.py" "$TMP"
# loopback-only server, shared by actual plugin fetches
python3 -m http.server "$PORT" --bind 127.0.0.1 --directory "$TMP/feed" >"$TMP/http.log" 2>&1 & SERVER=$!
trap 'kill "$SERVER" 2>/dev/null || true; cleanup' EXIT
sleep 1
# Run actual installed bundle; launcher itself accesses bridge via E2E command files.
python3 "$ROOT/build/updater-smoke/driver.py" healthy "$TMP" "$TMP/healthy-app"
# Tamper distinct copy, keep healthy candidate/evidence untouched, and retain genuine NEW signature/version.
python3 - "$TMP" "$NEW" <<'PY'
import json,pathlib,sys
p=pathlib.Path(sys.argv[1]); version=sys.argv[2]
archive=p/'tampered-feed/Muninn.app.tar.gz'; archive.parent.mkdir(parents=True,exist_ok=True)
source=p/'new/Muninn.app.tar.gz'; b=bytearray(source.read_bytes()); b[len(b)//2]^=1; archive.write_bytes(b)
feed=json.loads((p/'feed/latest.json').read_text()); feed['version']=version; feed['platforms']['darwin-aarch64']['url']=feed['platforms']['darwin-aarch64']['url'].replace('/Muninn.app.tar.gz','/../tampered-feed/Muninn.app.tar.gz'); (p/'feed/latest.json').write_text(json.dumps(feed))
PY
python3 "$ROOT/build/updater-smoke/driver.py" tampered "$TMP" "$TMP/tampered-app"
python3 "$ROOT/build/updater-smoke/verify_evidence.py" "$TMP"
echo 'PASS: native updater smoke; boolean-only evidence generated'
