#!/usr/bin/env python3
"""Require native driver assertions; output only booleans, no private details."""
import json, pathlib, sys
root = pathlib.Path(sys.argv[1])
required = ('healthy-result.json', 'tampered-result.json')
try:
    results = [json.loads((root / f).read_text()) for f in required]
except (OSError, json.JSONDecodeError) as error:
    raise SystemExit(f'FAIL: missing or invalid native evidence: {error}')
checks = (
    results[0].get('signature_accepted') is True,
    results[0].get('replacement_installed') is True,
    results[0].get('restart_requested') is True,
    results[0].get('restart_process_exited') is True,
    results[1].get('tamper_rejected') is True,
)
if not all(checks):
    raise SystemExit('FAIL: required signature/install/restart/tamper assertions absent')
(root/'evidence.json').write_text(json.dumps({key: value for result in results for key, value in result.items()}, sort_keys=True) + '\n')
