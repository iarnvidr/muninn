#!/usr/bin/env bash
# Launch the just-built ad-hoc app in the hosted Apple Silicon login session.
# This checks app startup + its loopback health/UI endpoints; it is not a UI automation test.
set -euo pipefail
app="${1:?usage: smoke-native-macos.sh /path/to/Muninn.app}"
[[ "$(uname -s)" == Darwin && "$(uname -m)" == arm64 ]] || { echo 'requires hosted Apple Silicon macOS' >&2; exit 2; }
[[ -d "$app/Contents/MacOS" ]] || { echo "app bundle missing: $app" >&2; exit 2; }
evidence="${SMOKE_EVIDENCE_DIR:-$RUNNER_TEMP/macos-smoke-evidence}"
mkdir -p "$evidence"
app="$(cd "$(dirname "$app")" && pwd)/$(basename "$app")"
plutil -lint "$app/Contents/Info.plist" | tee "$evidence/plist.txt"
/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$app/Contents/Info.plist" > "$evidence/executable-name.txt"
/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$app/Contents/Info.plist" > "$evidence/version.txt"
/usr/bin/lipo "$app/Contents/MacOS/$(cat "$evidence/executable-name.txt")" -verify_arch arm64
/usr/bin/codesign --verify --deep --strict "$app"
# App is purpose-built for developer-hosted interactive session; never import accounts or trigger login.
open -n "$app"
cleanup() { /usr/bin/osascript -e 'tell application "Muninn" to quit' >/dev/null 2>&1 || true; }
trap cleanup EXIT
python3 - "$evidence" <<'PY'
import json,sys,time,urllib.request
out=sys.argv[1]
base='http://127.0.0.1:47871'
def get(path):
    with urllib.request.urlopen(base+path,timeout=3) as r:
        return r.status,r.headers,r.read()
last=None
for _ in range(40):
    try:
        status,headers,body=get('/health')
        if status==200: break
    except Exception as e: last=e; time.sleep(.5)
else: raise SystemExit(f'app did not expose health endpoint: {last}')
health=json.loads(body)
assert health.get('ok') is True and health.get('app')=='muninn',health
assert health.get('version')==open(out+'/version.txt').read().strip(),health
status,headers,body=get('/ui/')
assert status==200 and len(body)>100 and b'Muninn' in body, (status,len(body))
open(out+'/health.json','w').write(json.dumps(health,indent=2)+'\n')
open(out+'/ui-response.json','w').write(json.dumps({'status':status,'content_type':headers.get('Content-Type'),'bytes':len(body)},indent=2)+'\n')
print('Native app launch, health/version, embedded UI endpoint passed')
PY
cat > "$evidence/limits.txt" <<'EOF'
This smoke opens the ad-hoc signed Muninn.app in the hosted macOS user session and checks arm64 executable, signature, bundle version, loopback /health and embedded /ui/ response. It does not automate or assert visible native window geometry, WebKit DOM/accessibility, playlist/keyboard interactions, login/import, or media playback. Geometry and UI behavior remain covered by existing unit/UI tests; native AVFoundation playback is not established here.
EOF
