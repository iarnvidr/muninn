import hashlib
import json
from pathlib import Path
import tempfile
import unittest
from repack_legacy_release import restore_sources


class LegacySourceRestore(unittest.TestCase):
    def fixture(self, root, path="manifest.json", digest=None):
        data = b'{"format":"muninn-corresponding-source-v1"}'
        (root / "source-manifest.json").write_bytes(data)
        (root / "source-assets-manifest.json").write_text(json.dumps([{
            "asset_name": "source-manifest.json", "original_path": path,
            "sha256": digest or hashlib.sha256(data).hexdigest(),
        }]))

    def test_restores_exact_manifest(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.fixture(root)
            restored = restore_sources(root)
            self.assertEqual((restored / "manifest.json").read_bytes(), (root / "source-manifest.json").read_bytes())

    def test_rejects_modified_source(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.fixture(root, digest="0" * 64)
            with self.assertRaisesRegex(ValueError, "digest mismatch"):
                restore_sources(root)

    def test_rejects_path_escape(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.fixture(root, path="../escape")
            with self.assertRaisesRegex(ValueError, "unsafe"):
                restore_sources(root)
            self.assertFalse((root / "escape").exists())
