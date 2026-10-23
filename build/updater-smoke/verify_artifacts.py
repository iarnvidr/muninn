#!/usr/bin/env python3
"""Validate generated harness fixture structure without trusting a mocked updater."""
import json, pathlib, sys
root = pathlib.Path(sys.argv[1])
for v in ('old', 'new'):
    meta = json.loads((root / f'{v}.json').read_text())
    archive = root / f'{v}.app.tar.gz'
    if not archive.is_file() or archive.stat().st_size < 1024:
        raise SystemExit(f'FAIL: missing/implausible {v} actual app archive')
    if not meta.get('version') or not meta.get('signature'):
        raise SystemExit(f'FAIL: missing Tauri version/signature in {v} metadata')
if json.loads((root/'old.json').read_text())['version'] == json.loads((root/'new.json').read_text())['version']:
    raise SystemExit('FAIL: versions are not distinct')
