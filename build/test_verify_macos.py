import hashlib
from pathlib import Path
import plistlib
import struct
import tempfile
import unittest
from unittest.mock import patch

from verify_macos import verify


class MacPackageVerificationTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.app = Path(self.temp.name) / "Muninn.app"
        self.contents = self.app / "Contents"
        for directory in ("MacOS", "Resources", "_CodeSignature"):
            (self.contents / directory).mkdir(parents=True)
        self.info = self.contents / "Info.plist"
        self.info.write_bytes(plistlib.dumps({
            "CFBundleIdentifier": "io.github.iarnvidr.muninn",
            "CFBundleExecutable": "muninn",
            "CFBundleName": "Muninn", "CFBundleShortVersionString": "0.2.0",
            "LSMinimumSystemVersion": "14.0",
            "NSAppTransportSecurity": {"NSAllowsLocalNetworking": True},
        }))
        (self.contents / "Resources/bin").mkdir()
        for name in ("yt-dlp", "ffmpeg", "deno"):
            tool = self.contents / "Resources/bin" / name
            tool.write_bytes(b"tool-" + name.encode())
            tool.chmod(0o755)
        self.binary = self.contents / "MacOS/muninn"
        self.binary.write_bytes(b"A" * 8192)
        self.icon = self.contents / "Resources/icon.icns"
        self.icon.write_bytes(b"icon")
        self.resources = self.contents / "_CodeSignature/CodeResources"
        self.resources.write_bytes(plistlib.dumps({"files2": {
            "Resources/icon.icns": {"hash2": hashlib.sha256(b"icon").digest()},
        }}))
        digest = hashlib.sha256
        self.cd = struct.pack(
            ">9I4B", 0xFADE0C02, 200, 0x20400, 2, 136, 0, 3, 2, 8192, 32, 2, 0, 12,
        ) + digest(self.resources.read_bytes()).digest() + bytes(32) + digest(self.info.read_bytes()).digest()
        self.cd += digest(b"A" * 4096).digest() * 2
        mock = patch("verify_macos.subprocess.check_output", side_effect=self.command)
        mock.start()
        self.addCleanup(mock.stop)

    def command(self, args, **kwargs):
        if args[0] == "llvm-lipo-14":
            # yt-dlp is universal2; everything else is thin here.
            return "arm64 x86_64\n" if args[-1].endswith("yt-dlp") else "arm64\n"
        return self.cd

    def test_accepts_matching_code_and_bundle_seals(self):
        verify(self.app, ["aarch64"])

    def test_requires_the_bundled_helpers(self):
        (self.contents / "Resources/bin/ffmpeg").unlink()
        with self.assertRaisesRegex(ValueError, "missing bundled tool: ffmpeg"):
            verify(self.app, ["aarch64"])

    def test_rejects_a_non_executable_helper(self):
        (self.contents / "Resources/bin/deno").chmod(0o644)
        with self.assertRaisesRegex(ValueError, "not executable: deno"):
            verify(self.app, ["aarch64"])

    def test_rejects_an_old_minimum_system_version(self):
        info = plistlib.loads(self.info.read_bytes())
        info["LSMinimumSystemVersion"] = "11.0"
        self.info.write_bytes(plistlib.dumps(info))
        with self.assertRaisesRegex(ValueError, "macOS 14"):
            verify(self.app, ["aarch64"])

    def test_rejects_modified_executable(self):
        self.binary.write_bytes(b"B" + b"A" * 8191)
        with self.assertRaisesRegex(ValueError, "code hash mismatch"):
            verify(self.app, ["aarch64"])

    def test_rejects_modified_info_plist(self):
        info = plistlib.loads(self.info.read_bytes())
        info["CFBundleShortVersionString"] = "9.9.9"
        self.info.write_bytes(plistlib.dumps(info))
        with self.assertRaisesRegex(ValueError, "bundle seal mismatch"):
            verify(self.app, ["aarch64"])

    def test_rejects_modified_resource(self):
        self.icon.write_bytes(b"modified icon")
        with self.assertRaisesRegex(ValueError, "resource hash mismatch"):
            verify(self.app, ["aarch64"])

    def test_rejects_replaced_resource_manifest(self):
        resources = plistlib.loads(self.resources.read_bytes())
        resources["modified"] = True
        self.resources.write_bytes(plistlib.dumps(resources))
        with self.assertRaisesRegex(ValueError, "bundle seal mismatch"):
            verify(self.app, ["aarch64"])

    def test_rejects_missing_intel_slice(self):
        with self.assertRaisesRegex(ValueError, "expected architectures"):
            verify(self.app, ["aarch64", "x86_64"])


if __name__ == "__main__":
    unittest.main()
