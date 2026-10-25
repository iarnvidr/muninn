#!/usr/bin/env bash
# Package the verified app and corresponding sources as one public download.
set -euo pipefail
app="$(cd "$1" && pwd)"
sources="$(cd "$2" && pwd)"
image_dir="$(dirname "$3")"
mkdir -p "$image_dir"
image="$(cd "$image_dir" && pwd)/$(basename "$3")"
[[ "$image" == *.dmg ]]
test -f "$app/Contents/Info.plist"
test -f "$sources/manifest.json"
codesign --verify --deep --strict "$app"
version=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$app/Contents/Info.plist")
stage="$(mktemp -d)"
mount="$stage/mount"
attached=0
cleanup() {
  if [[ "$attached" == 1 ]]; then
    hdiutil detach "$mount" >/dev/null || return
  fi
  rm -rf "$stage"
}
trap cleanup EXIT
mkdir -p "$stage/payload" "$mount"
ditto "$app" "$stage/payload/Muninn.app"
ditto "$sources" "$stage/payload/Corresponding Source"
ln -s /Applications "$stage/payload/Applications"
hdiutil create -volname Muninn -srcfolder "$stage/payload" -fs HFS+ -format UDZO -ov "$image"
hdiutil verify "$image"
hdiutil attach "$image" -readonly -nobrowse -mountpoint "$mount"
attached=1
codesign --verify --deep --strict "$mount/Muninn.app"
test "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$mount/Muninn.app/Contents/Info.plist")" = "$version"
test -L "$mount/Applications"
test "$(readlink "$mount/Applications")" = /Applications
python3 - "$mount/Corresponding Source" <<'PY'
import hashlib, json, pathlib, sys
root = pathlib.Path(sys.argv[1])
manifest = json.loads((root / 'manifest.json').read_text())
assert manifest['format'] == 'muninn-corresponding-source-v1'
assert manifest['sources'], 'No corresponding sources in DMG'
for source in manifest['sources']:
    assert hashlib.sha256((root / source['asset']).read_bytes()).hexdigest() == source['sha256']
assert (root / manifest['build_recipe']).is_file()
for notice in manifest['license_notices']:
    assert (root / notice).is_file()
PY
hdiutil detach "$mount"
attached=0
echo "$image"
