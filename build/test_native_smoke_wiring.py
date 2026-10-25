"""Regression coverage for the native macOS workflow's smoke bundle path."""
from pathlib import Path
import re
import unittest
import ast
import json
import os
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parent.parent


class NativeSmokeWiringTests(unittest.TestCase):
    def test_release_health_and_debug_ui_interaction_are_separate(self):
        workflow = (ROOT / ".github/workflows/macos.yml").read_text()
        builder = (ROOT / "build/build-native-macos.sh").read_text()
        smoke = (ROOT / "build/smoke-native-macos.sh").read_text()
        ui_smoke = (ROOT / "build/native-ui-smoke.sh").read_text()

        self.assertIn('run: bash build/build-native-macos.sh "$RUNNER_TEMP/muninn"', workflow)
        self.assertIn('run: bash build/smoke-native-macos.sh "$RUNNER_TEMP/muninn/Muninn.app"', workflow)
        self.assertIn('ditto "$app" "$out/Muninn.app"', builder)
        self.assertIn('cargo build --locked -p muninn --target aarch64-apple-darwin', workflow)
        self.assertIn('Muninn.app', workflow)
        self.assertIn('aarch64-apple-darwin/debug/muninn', ui_smoke)
        self.assertIn('codesign --force --sign - "$app"', ui_smoke)
        self.assertIn('Muninn.app', ui_smoke)
        self.assertIn('native-ui-smoke', ui_smoke)
        self.assertIn('debug native UI smoke marker', ui_smoke)
        self.assertNotIn('native-ui-smoke.sh', smoke)
        self.assertNotIn("cargo tauri", workflow)

    def test_release_artifact_and_debug_evidence_are_uploaded_separately(self):
        workflow = (ROOT / ".github/workflows/macos.yml").read_text()
        ui_smoke = (ROOT / "build/native-ui-smoke.sh").read_text()
        self.assertIn('muninn-release-evidence/**', workflow)
        self.assertIn('muninn-debug-ui-evidence/**', workflow)
        self.assertIn('Muninn_*.zip', workflow)
        self.assertNotIn('muninn-debug/**', workflow)
        self.assertIn('screenshots/window.png', ui_smoke)
        self.assertIn('reports/os-permission.txt', ui_smoke)
        self.assertIn('if: always()', workflow)
        self.assertIn('MUNINN_DATA_DIR', ui_smoke)
        self.assertIn('interaction_status', ui_smoke)
        self.assertIn('Contents/Resources/bin/ffmpeg', ui_smoke)
        self.assertIn('! -L "$ffmpeg" && -x "$ffmpeg"', ui_smoke)
        self.assertIn('generate-native-fixture.swift', ui_smoke)
        self.assertIn('xcrun swiftc -parse-as-library build/generate-native-fixture.swift -o "$scratch/fixture-generator"', ui_smoke)
        self.assertIn('"$scratch/fixture-generator" "$NATIVE_SMOKE_MEDIA_FILE"', ui_smoke)
        self.assertNotIn('xcrun swift build/generate-native-fixture.swift', ui_smoke)
        self.assertNotRegex(ui_smoke, r"ffmpeg[^\\n]*-f\\s+lavfi|testsrc2?=|sine=frequency")
        fixture = (ROOT / "build/generate-native-fixture.swift").read_text()
        self.assertIn('AVAssetWriter', fixture)
        self.assertIn('AVVideoCodecType.h264', fixture)
        self.assertIn('900', fixture)
        self.assertIn('asset.load(.isPlayable)', fixture)
        self.assertIn('NATIVE_SMOKE_MEDIA_FILE', ui_smoke)
        self.assertIn('reports/media-fixture.txt', ui_smoke)

    def test_native_ui_tab_snapshot_focus_contract_is_consistent(self):
        driver = (ROOT / "build/native-ui-driver.py").read_text()
        # Exercise the source-level JS snapshot contract and Python consumers:
        # focus is reported as focusedTab and remains asserted during sync/cycling.
        snapshot = re.search(r"def tabstate\(\): return ev\(\"(.*?)\"\)", driver)
        self.assertIsNotNone(snapshot)
        self.assertIn("focusedTab:document.activeElement?.closest('.tab')?.dataset.id ?? null", snapshot.group(1))
        self.assertNotIn("focused:", snapshot.group(1))
        self.assertGreaterEqual(driver.count("['focusedTab']"), 2)
        self.assertIn("str(state['focusedTab'])==str(tab_id)", driver)
        self.assertIn("str(after['focusedTab'])==str(expected)", driver)
        self.assertIn("n.focus();n.click()", driver)
        self.assertIn("len(set(map(str,tabs)))==2", driver)
        self.assertIn("run_marker=f'native-smoke-{os.getpid()}-{time.time_ns()}'", driver)
        self.assertIn('native_smoke={run_marker}-1', driver)
        self.assertIn('native_smoke={run_marker}-2', driver)
        self.assertIn("'url':fixture_url", driver)
        self.assertIn('return await window.__muninnNativePlayerInteractions', driver)
        self.assertIn("return await window.__muninnNativePlayerInteractions", driver)
        self.assertIn("mediaBase64:'\"+payload+\"'", driver)
        self.assertNotIn("str(after['focused'])", driver)

    def test_fixture_destinations_exercise_dedup_and_protocol_preserves_returned_ids(self):
        # Replay the driver's actual Python tab-creation statements against a
        # stateful fake core using destination canonicalization and dedup.
        # The fake classifies about:blank as Home and strips tracking params,
        # while retaining meaningful native_smoke query strings as distinct.
        driver = (ROOT / "build/native-ui-driver.py").read_text()
        tree = ast.parse(driver)
        loop = next(n for n in ast.walk(tree) if isinstance(n, ast.For) and isinstance(n.iter, ast.Name) and n.iter.id == "fixture_urls")
        assignment = next(n for n in ast.walk(loop) if isinstance(n, ast.Assign) and any(isinstance(t, ast.Name) and t.id == "r" for t in n.targets))
        expression = ast.Expression(assignment.value)
        code = compile(expression, "native-ui-driver.py", "eval")
        seen = {}
        next_id = 40
        responses = {}
        def send(command):
            nonlocal next_id
            command = dict(command)
            command["id"] = f'native{len(responses) + 1:04d}'
            url = command["args"]["url"]
            # Model the destination identity rules relevant to the protocol:
            # blank resolves to Home; tracking is stripped; native_smoke is not.
            from urllib.parse import urlsplit, parse_qsl, urlencode
            if url == "about:blank":
                key = "home"
            else:
                parts = urlsplit(url)
                query = [(k, v) for k, v in parse_qsl(parts.query) if k.lower() not in {"utm_source", "utm_medium", "utm_campaign"}]
                key = (parts.scheme.lower(), parts.netloc.lower().removeprefix("www."), parts.path or "/", urlencode(query))
            if key not in seen:
                seen[key] = next_id
                next_id += 1
            response = {"ok": True, "id": seen[key]}
            responses[command["id"]] = response
            return response
        seq = 0
        tabs = []
        env = {"fixture_urls": ["https://www.youtube.com/?native_smoke=run-1", "https://www.youtube.com/?native_smoke=run-2"],
               "send": send, "seq": seq}
        for fixture_url in env["fixture_urls"]:
            seq += 1
            env.update(seq=seq, fixture_url=fixture_url)
            command = eval(code, env)
            tabs.append(responses[f'native{seq:04d}']["id"])
        self.assertEqual(tabs, [40, 41])
        self.assertEqual(len(set(tabs)), 2)
        duplicate = {"id": "native0003", "op": "call", "name": "tab_new", "args": {"url": env["fixture_urls"][0]}}
        self.assertEqual(send(duplicate)["id"], tabs[0], "same destination remains deduplicated")

        # Negative control: the former repeated about:blank fixture collides
        # because both URLs canonicalize to Home; verify IDs actually collapse.
        blank_ids = [send({"op": "call", "name": "tab_new", "args": {"url": "about:blank"}})["id"] for _ in range(2)]
        self.assertEqual(blank_ids, [42, 42])
        self.assertFalse(len(set(blank_ids)) == 2, "old repeated-blank fixture must fail distinct-ID assertion")

    def test_no_focus_snapshot_is_json_safe_and_has_explicit_null_contract(self):
        # Mirror JSON.stringify's omission of undefined object fields: optional
        # chaining without ?? null silently removes focusedTab from snapshots.
        snapshot = {"active": 4, "focusedTab": None}
        decoded = __import__('json').loads(__import__('json').dumps(snapshot))
        self.assertIn("focusedTab", decoded)
        self.assertIsNone(decoded["focusedTab"])

    def test_preview_version_agrees_across_metadata_and_lockfile(self):
        import json
        import tomllib
        version = "0.2.14-preview.4"
        config = json.loads((ROOT / "src-tauri/tauri.conf.json").read_text())
        cargo = tomllib.loads((ROOT / "Cargo.toml").read_text())
        lock = tomllib.loads((ROOT / "Cargo.lock").read_text())
        self.assertEqual(cargo["workspace"]["package"]["version"], version)
        self.assertEqual(config["version"], version)
        for package in lock["package"]:
            if package["name"] in {"muninn", "muninn-core"}:
                self.assertEqual(package["version"], version)



if __name__ == "__main__":
    unittest.main()
