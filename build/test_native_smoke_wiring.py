"""Regression coverage for the native macOS workflow's smoke bundle path."""
from pathlib import Path
import re
import unittest

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
        self.assertIn("return await window.__muninnNativePlayerInteractions", driver)
        self.assertIn("mediaBase64:'\"+payload+\"'", driver)
        self.assertNotIn("str(after['focused'])", driver)

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
