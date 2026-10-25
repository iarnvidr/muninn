import hashlib
import io
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from pathlib import Path
import sys
sys.path.insert(0, str(Path(__file__).resolve().parent))
import release_sources as rs


class Response:
    def __init__(self, data):
        self.data = data

    def __enter__(self):
        return self

    def __exit__(self, *args):
        return False

    def read(self):
        return self.data


class ReleaseSourcesTests(unittest.TestCase):
    def test_import_and_pins_do_not_download(self):
        with patch.object(rs.urllib.request, "urlopen", side_effect=AssertionError("network")):
            pins = rs.pinned_sources()
        self.assertEqual([p["name"] for p in pins], ["ffmpeg", "dav1d"])

    def test_hash_mismatch_refuses_asset_and_manifest(self):
        with tempfile.TemporaryDirectory() as tmp:
            dest = Path(tmp) / "out"
            with patch.object(rs, "pinned_sources", return_value=[{
                "name": "ffmpeg", "version": "1", "url": "https://example.invalid/a.tar.xz",
                "sha256": "0" * 64,
            }]):
                with self.assertRaisesRegex(ValueError, "SHA-256 mismatch"):
                    rs.prepare(dest, opener=lambda *a, **k: Response(b"bad"))
            self.assertFalse(dest.exists())
            self.assertEqual(list(Path(tmp).glob(".out.staging-*")), [])

    def test_second_archive_hash_failure_leaves_no_destination(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "root"
            (root / "build").mkdir(parents=True)
            (root / "licenses").mkdir()
            payloads = {"a.tar.xz": b"first", "b.tar.xz": b"second"}
            sources = [{"name": n, "version": "1", "url": "https://example/" + f,
                        "sha256": hashlib.sha256(payloads[f]).hexdigest()}
                       for n, f in (("ffmpeg", "a.tar.xz"), ("dav1d", "b.tar.xz"))]
            sources[1]["sha256"] = "0" * 64
            dest = Path(tmp) / "out"
            with patch.object(rs, "pinned_sources", return_value=sources):
                with self.assertRaisesRegex(ValueError, "SHA-256 mismatch"):
                    rs.prepare(dest, root=root, opener=lambda req, timeout: Response(payloads[req.full_url.rsplit("/", 1)[-1]]))
            self.assertFalse(dest.exists())
            self.assertEqual(list(Path(tmp).glob(".out.staging-*")), [])

    def test_copy_failure_leaves_no_destination(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "root"
            (root / "build").mkdir(parents=True)
            (root / "licenses").mkdir()
            (root / "build" / "ffmpeg_lgpl.py").write_text("recipe")
            for name in ("ffmpeg-COPYING.LGPLv2.1", "dav1d-COPYING"):
                (root / "licenses" / name).write_text(name)
            payload = b"archive"
            sources = [{"name": "ffmpeg", "version": "1", "url": "https://example/a.tar.xz",
                        "sha256": hashlib.sha256(payload).hexdigest()}]
            dest = Path(tmp) / "out"
            with patch.object(rs, "pinned_sources", return_value=sources), \
                 patch.object(rs.shutil, "copy2", side_effect=OSError("copy failed")):
                with self.assertRaisesRegex(OSError, "copy failed"):
                    rs.prepare(dest, root=root, opener=lambda *a, **k: Response(payload))
            self.assertFalse(dest.exists())
            self.assertEqual(list(Path(tmp).glob(".out.staging-*")), [])

    def test_existing_destination_is_unchanged(self):
        with tempfile.TemporaryDirectory() as tmp:
            dest = Path(tmp) / "out"
            dest.mkdir()
            (dest / "user.txt").write_text("preserve")
            with self.assertRaises(FileExistsError):
                rs.prepare(dest)
            self.assertEqual((dest / "user.txt").read_text(), "preserve")

    def test_verified_assets_manifest_and_copied_notice_recipe(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "root"
            (root / "build").mkdir(parents=True)
            (root / "licenses").mkdir()
            (root / "build" / "ffmpeg_lgpl.py").write_text("recipe")
            for name in ("ffmpeg-COPYING.LGPLv2.1", "dav1d-COPYING"):
                (root / "licenses" / name).write_text(name)
            payloads = {"a.tar.xz": b"ffmpeg", "b.tar.xz": b"dav1d"}
            sources = [{"name": key, "version": "1", "url": "https://example/" + filename,
                        "sha256": hashlib.sha256(payloads[filename]).hexdigest()}
                       for key, filename in (("ffmpeg", "a.tar.xz"), ("dav1d", "b.tar.xz"))]
            with patch.object(rs, "pinned_sources", return_value=sources):
                manifest = rs.prepare(Path(tmp) / "out", root=root,
                                      opener=lambda req, timeout: Response(payloads[req.full_url.rsplit("/", 1)[-1]]))
            out = Path(tmp) / "out"
            loaded = json.loads((out / "manifest.json").read_text())
            self.assertEqual(loaded, manifest)
            self.assertEqual([x["asset"] for x in loaded["sources"]], ["a.tar.xz", "b.tar.xz"])
            self.assertEqual((out / "a.tar.xz").read_bytes(), b"ffmpeg")
            self.assertEqual((out / "ffmpeg_lgpl.py").read_text(), "recipe")
            self.assertTrue((out / "notices" / "dav1d-COPYING").is_file())
            self.assertIn("not a complete", loaded["scope"])

    def test_no_command_is_refused_without_network(self):
        with patch.object(rs.urllib.request, "urlopen", side_effect=AssertionError("network")):
            with self.assertRaises(SystemExit):
                rs.main([])


if __name__ == "__main__":
    unittest.main()
