#!/usr/bin/env python3
"""Regression checks for native updater smoke harness bundle and tamper wiring."""
import pathlib
import re

ROOT = pathlib.Path(__file__).resolve().parents[1]
script = (ROOT / "build/updater-smoke/run-macos.sh").read_text()
assert '"$TMP/healthy/Muninn.app"' in script
assert '"$TMP/tampered/Muninn.app"' in script
assert 'mkdir -p "$TMP/data" "$TMP/feed" "$TMP/e2e" "$TMP/home" "$TMP/healthy" "$TMP/tampered"' in script
assert re.search(r'for bundle in .*healthy/Muninn\.app.*tampered/Muninn\.app', script)
assert 'Set :CFBundleShortVersionString $OLD' in script
assert 'codesign --force --deep --sign - "$bundle"' in script
assert "archive=p/'feed/Muninn.app.tar.gz'" in script
assert "source=p/'new/Muninn.app.tar.gz'" in script
assert "b[len(b)//2]^=1; archive.write_bytes(b)" in script
assert "feed['version']=version" not in script
assert "tampered-feed" not in script and "replace('/Muninn.app.tar.gz'" not in script
assert 'driver.py" healthy "$TMP" "$TMP/healthy/Muninn.app"' in script
assert 'driver.py" tampered "$TMP" "$TMP/tampered/Muninn.app"' in script
assert 'healthy-result tampered-result evidence' in script
print('PASS: updater smoke wiring preserves real .app roots and serves tamper bytes at signed in-feed URL')
