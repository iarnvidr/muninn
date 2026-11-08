import json
from pathlib import Path
import unittest

from native_info import metadata, configured_bundle


class NativeBundleIdentityTests(unittest.TestCase):
    def test_pr_preview_bundle_has_distinct_identifier_and_no_production_url_handler(self):
        import os
        from unittest.mock import patch
        overlay = '{"identifier":"io.github.iarnvidr.muninn.preview","productName":"Muninn Preview","bundle":{"macOS":{"infoPlist":"Info.preview.plist"}}}'
        with patch.dict(os.environ, {"TAURI_CONFIG": overlay}):
            info = metadata(configured_bundle())
        self.assertEqual(info["CFBundleIdentifier"], "io.github.iarnvidr.muninn.preview")
        self.assertEqual(info["CFBundleDisplayName"], "Muninn Preview")
        self.assertNotIn("CFBundleURLTypes", info)
    def test_native_bundle_preserves_installation_identity_and_local_networking(self):
        root = Path(__file__).resolve().parents[1]
        config = json.loads((root / "src-tauri/tauri.conf.json").read_text())
        info = metadata(config)
        self.assertEqual(info["CFBundleIdentifier"], "io.github.iarnvidr.muninn")
        self.assertEqual(info["CFBundleExecutable"], "muninn")
        self.assertEqual(info["CFBundleShortVersionString"], config["version"])
        self.assertEqual(info["LSMinimumSystemVersion"], "14.0")
        self.assertTrue(info["NSAppTransportSecurity"]["NSAllowsLocalNetworking"])

    def test_app_links_are_registered_in_the_packaged_app(self):
        root = Path(__file__).resolve().parents[1]
        config = json.loads((root / "src-tauri/tauri.conf.json").read_text())
        types = metadata(config)["CFBundleURLTypes"]
        self.assertEqual(types[0]["CFBundleURLSchemes"], ["muninn"])
        self.assertEqual(types[0]["CFBundleURLName"], "io.github.iarnvidr.muninn.open")
