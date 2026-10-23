import json
from pathlib import Path
import unittest

from native_info import metadata


class NativeBundleIdentityTests(unittest.TestCase):
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
