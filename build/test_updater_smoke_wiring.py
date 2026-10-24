#!/usr/bin/env python3
"""Regression checks for native updater smoke harness bundle and tamper wiring."""
import pathlib
import re

try:
    import yaml
except ImportError:
    yaml = None

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
# Inspect the actual workflow step structure rather than accepting a stray runner path.
workflow_text = (ROOT / ".github/workflows/macos.yml").read_text()
if yaml:
    workflow = yaml.safe_load(workflow_text)
    steps = workflow["jobs"]["package"]["steps"]
    by_id = {step.get("id"): step for step in steps}
else:
    # Narrow fallback parser for environments without PyYAML.
    steps = []
    current = None
    for line in workflow_text.splitlines():
        if re.match(r"^      - (?:id:|name:)", line):
            if current is not None:
                steps.append(current)
            current = {"raw": line}
        elif current is not None:
            current["raw"] += "\n" + line
    if current is not None:
        steps.append(current)
    by_id = {}
    for step in steps:
        match = re.search(r"^      - id: ([^\n]+)", step["raw"], re.M)
        if match:
            by_id[match.group(1)] = step
prepare = by_id.get("updater-smoke")
assert prepare is not None, "separate updater-smoke workflow step is missing"
debug_prep = by_id.get("prepare-debug-smoke")
assert debug_prep is not None, "debug bundle preparation step id is missing"
expected_condition = "${{ !cancelled() && steps['prepare-debug-smoke'].outcome == 'success' }}"
ui_name = "Native UI interaction smoke (debug bundle only)"
if yaml:
    ui = next(step for step in steps if step.get("name") == ui_name)
    assert prepare.get("if") == expected_condition
    assert ui.get("if") == expected_condition
    assert "continue-on-error" not in prepare and "continue-on-error" not in ui
else:
    assert expected_condition in prepare["raw"]
    ui = next(step for step in steps if ui_name in step["raw"])
    assert expected_condition in ui["raw"]
    assert not re.search(r"^\s*continue-on-error:", prepare["raw"], re.M)
    assert not re.search(r"^\s*continue-on-error:", ui["raw"], re.M)
if yaml:
    assert "bash build/updater-smoke/run-macos.sh" in prepare["run"]
    assert prepare["env"]["MUNINN_UPDATER_SMOKE_APP"] == "${{ runner.temp }}/muninn-debug-smoke/Muninn.app"
    assert prepare["env"]["MUNINN_UPDATER_SMOKE_EVIDENCE_DIR"] == "${{ runner.temp }}/muninn-updater-evidence"
    names = [step.get("name", "") for step in steps]
    assert names.index("Prepare isolated debug bundle for native smokes") < names.index(prepare["name"])
    assert names.index("Native UI interaction smoke (debug bundle only)") > names.index(prepare["name"])
    upload = next(step for step in steps if step.get("id") == "upload")
    assert "${{ runner.temp }}/muninn-updater-evidence/**" in upload["with"]["path"]
else:
    raw = prepare["raw"]
    assert "bash build/updater-smoke/run-macos.sh" in raw
    assert "MUNINN_UPDATER_SMOKE_APP: ${{ runner.temp }}/muninn-debug-smoke/Muninn.app" in raw
    assert "MUNINN_UPDATER_SMOKE_EVIDENCE_DIR: ${{ runner.temp }}/muninn-updater-evidence" in raw
    assert "${{ runner.temp }}/muninn-updater-evidence/**" in workflow_text
assert 'ditto "$RUNNER_TEMP/muninn/Muninn.app" "$app"' in workflow_text
assert 'cp "$debug_binary" "$app/Contents/MacOS/$exe"' in workflow_text
print('PASS: updater smoke wiring preserves real .app roots and serves tamper bytes at signed in-feed URL')
print('PASS: independent updater-smoke step runs after debug preparation, before native UI, with isolated evidence artifact')
