import hashlib
import io
import json
import tempfile
import unittest
import zipfile
from pathlib import Path
from unittest.mock import patch

import fetch_tools


class FakeResponse(io.BytesIO):
    def __enter__(self):
        return self

    def __exit__(self, *a):
        return False


class FetchToolsTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.cache = Path(self.tmp.name) / "cache"

    def test_lock_file_pins_every_download_with_a_sha256(self):
        lock = json.loads((Path(fetch_tools.__file__).parent / "tools.lock.json").read_text())
        specs = []
        for tool, entry in lock.items():
            if tool.startswith("_"):
                continue
            specs += [entry[k] for k in ("universal", "arm64", "x86_64") if k in entry]
        self.assertGreaterEqual(len(specs), 4)
        for spec in specs:
            self.assertRegex(spec["sha256"], r"^[0-9a-f]{64}$")
            self.assertTrue(spec["url"].startswith("https://"))
        self.assertIn("arm64", lock["deno"])
        self.assertIn("x86_64", lock["deno"])
        # ffmpeg is compiled from a pinned source tarball with an LGPL-only configuration.
        self.assertEqual(lock["ffmpeg"]["arm64"]["kind"], fetch_tools.FFMPEG_SOURCE_KIND)
        self.assertNotIn("x86_64", lock["ffmpeg"])
        self.assertIn("LGPL", lock["ffmpeg"]["license"])
        self.assertNotIn("GPL-3", lock["ffmpeg"]["license"])

    def test_a_checksum_mismatch_installs_nothing(self):
        with patch("fetch_tools.urllib.request.urlopen", return_value=FakeResponse(b"tampered")):
            with self.assertRaises(fetch_tools.ChecksumError):
                fetch_tools.download("https://example.test/x", "0" * 64, self.cache)
        self.assertEqual(list(self.cache.glob("*")), [])

    def test_a_verified_download_is_cached_and_reused(self):
        data = b"binary"
        digest = hashlib.sha256(data).hexdigest()
        with patch("fetch_tools.urllib.request.urlopen", return_value=FakeResponse(data)) as m:
            self.assertEqual(fetch_tools.download("https://example.test/x", digest, self.cache), data)
            self.assertEqual(fetch_tools.download("https://example.test/x", digest, self.cache), data)
            self.assertEqual(m.call_count, 1)

    def test_a_corrupted_cache_entry_is_not_trusted(self):
        data = b"good"
        digest = hashlib.sha256(data).hexdigest()
        self.cache.mkdir()
        (self.cache / digest).write_bytes(b"bad")
        with patch("fetch_tools.urllib.request.urlopen", return_value=FakeResponse(data)) as m:
            self.assertEqual(fetch_tools.download("https://example.test/x", digest, self.cache), data)
            self.assertEqual(m.call_count, 1)

    def test_zip_members_are_extracted_and_laid_out_per_arch(self):
        def zipped(content):
            buf = io.BytesIO()
            with zipfile.ZipFile(buf, "w") as z:
                z.writestr("tool", content)
            return buf.getvalue()

        blobs = {"https://x/arm": zipped(b"ARM"), "https://x/x64": zipped(b"X64"), "https://x/ytdlp": b"YTDLP"}
        lock = {
            "tool": {"version": "1", "license": "MIT", "source": "s", "arm64": {"url": "https://x/arm", "sha256": hashlib.sha256(blobs["https://x/arm"]).hexdigest(), "kind": "zip", "member": "tool"},
                     "x86_64": {"url": "https://x/x64", "sha256": hashlib.sha256(blobs["https://x/x64"]).hexdigest(), "kind": "zip", "member": "tool"}},
            "yt-dlp": {"version": "2", "license": "Unlicense", "source": "s", "universal": {"url": "https://x/ytdlp", "sha256": hashlib.sha256(b"YTDLP").hexdigest(), "kind": "file"}},
        }

        def fake_open(req, timeout=0):
            return FakeResponse(blobs[req.full_url])

        out = Path(self.tmp.name) / "out"
        with patch("fetch_tools.urllib.request.urlopen", side_effect=fake_open):
            fetch_tools.build(out, lock, self.cache, lipo=False)
        self.assertEqual((out / "arm64/tool").read_bytes(), b"ARM")
        self.assertEqual((out / "x86_64/tool").read_bytes(), b"X64")
        self.assertEqual((out / "universal/yt-dlp").read_bytes(), b"YTDLP")
        self.assertTrue((out / "arm64/tool").stat().st_mode & 0o111)
        self.assertIn("tool 1", (out / "NOTICE.txt").read_text())

    def test_arm64_only_does_not_download_intel_or_invoke_lipo(self):
        blob = b"arm64 helper"
        lock = {"tool": {"version": "1", "license": "MIT", "source": "s",
                "arm64": {"url": "https://x/arm", "sha256": hashlib.sha256(blob).hexdigest(), "kind": "file"},
                "x86_64": {"url": "https://x/intel", "sha256": "0" * 64, "kind": "file"}}}
        with patch("fetch_tools.urllib.request.urlopen", return_value=FakeResponse(blob)) as get, \
             patch("fetch_tools.subprocess.check_call") as lipo:
            out = Path(self.tmp.name) / "arm-only"
            fetch_tools.build(out, lock, self.cache, lipo=False, archs=("arm64",))
        self.assertEqual(get.call_args.args[0].full_url, "https://x/arm")
        self.assertEqual(get.call_count, 1)
        lipo.assert_not_called()
        self.assertEqual((out / "arm64/tool").read_bytes(), blob)
        self.assertFalse((out / "x86_64").exists())

    def ffmpeg_lock(self, blob):
        return {"ffmpeg": {"version": "9", "license": "LGPL-2.1-or-later", "source": "s",
                "arm64": {"url": "https://x/ffmpeg.tar.xz", "sha256": hashlib.sha256(blob).hexdigest(),
                          "kind": fetch_tools.FFMPEG_SOURCE_KIND}}}

    def test_ffmpeg_is_built_from_the_pinned_tarball_and_the_configure_line_is_noticed(self):
        tarball = b"source tarball"
        out = Path(self.tmp.name) / "ffmpeg-out"
        with patch("fetch_tools.urllib.request.urlopen", return_value=FakeResponse(tarball)) as get, \
             patch("fetch_tools.ffmpeg_lgpl.build", return_value=b"LGPL-FFMPEG") as build:
            fetch_tools.build(out, self.ffmpeg_lock(tarball), self.cache, lipo=False, archs=("arm64",))
            build.assert_called_once_with(tarball)
            self.assertEqual(get.call_count, 1)
            # A second run reuses the cached, re-verified build and does not rebuild.
            with patch("fetch_tools.ffmpeg_lgpl.verify") as verify:
                fetch_tools.build(out, self.ffmpeg_lock(tarball), self.cache, lipo=False, archs=("arm64",))
            verify.assert_called_once()
            build.assert_called_once()
        self.assertEqual((out / "arm64/ffmpeg").read_bytes(), b"LGPL-FFMPEG")
        notice = (out / "NOTICE.txt").read_text()
        self.assertIn("--disable-gpl", notice)
        self.assertIn("--disable-nonfree", notice)
        self.assertNotIn("--enable-gpl", notice)

    def test_ffmpeg_stale_cache_directory_is_removed_and_rebuilt_from_pinned_lgpl_source(self):
        tarball = b"pinned source"
        lock = self.ffmpeg_lock(tarball)
        recipe = fetch_tools.sha256_of((Path(fetch_tools.__file__).parent / "ffmpeg_lgpl.py").read_bytes())[:16]
        cached = self.cache / f"ffmpeg-lgpl-{lock['ffmpeg']['arm64']['sha256']}-{recipe}"
        cached.mkdir(parents=True)
        (cached / "old-extraction").write_text("stale")
        good = b"\0--disable-gpl --disable-nonfree\0libavcodec license: LGPL version 2.1 or later\0"
        with patch("fetch_tools.urllib.request.urlopen", return_value=FakeResponse(tarball)) as get, \
             patch("fetch_tools.ffmpeg_lgpl.build", return_value=good) as build, \
             patch("fetch_tools.ffmpeg_lgpl.verify") as verify:
            result = fetch_tools.build_ffmpeg_lgpl(lock["ffmpeg"]["arm64"], self.cache)
        self.assertEqual(result, good)
        build.assert_called_once_with(tarball)
        verify.assert_not_called()  # the rebuilt binary was already verified by the builder
        self.assertEqual(get.call_count, 1)
        self.assertTrue(cached.is_file())
        self.assertEqual(cached.read_bytes(), good)
        self.assertIn("--disable-gpl", good.decode())
        self.assertNotIn("--enable-gpl", good.decode())

    def test_ffmpeg_non_executable_cache_is_rebuilt_not_executed(self):
        tarball = b"pinned source"
        lock = self.ffmpeg_lock(tarball)
        recipe = fetch_tools.sha256_of((Path(fetch_tools.__file__).parent / "ffmpeg_lgpl.py").read_bytes())[:16]
        cached = self.cache / f"ffmpeg-lgpl-{lock['ffmpeg']['arm64']['sha256']}-{recipe}"
        cached.parent.mkdir(parents=True)
        cached.write_bytes(b"stale")
        cached.chmod(0o644)
        good = b"\0--disable-gpl\0libavcodec license: LGPL version 2.1 or later\0"
        with patch("fetch_tools.urllib.request.urlopen", return_value=FakeResponse(tarball)), \
             patch("fetch_tools.ffmpeg_lgpl.build", return_value=good) as build, \
             patch("fetch_tools.ffmpeg_lgpl.verify") as verify:
            self.assertEqual(fetch_tools.build_ffmpeg_lgpl(lock["ffmpeg"]["arm64"], self.cache), good)
        build.assert_called_once_with(tarball)
        verify.assert_not_called()  # the rebuilt binary was already verified by the builder

    def test_ffmpeg_tarball_checksum_mismatch_builds_nothing(self):
        with patch("fetch_tools.urllib.request.urlopen", return_value=FakeResponse(b"tampered")), \
             patch("fetch_tools.ffmpeg_lgpl.build") as build:
            with self.assertRaises(fetch_tools.ChecksumError):
                fetch_tools.build(Path(self.tmp.name) / "o", self.ffmpeg_lock(b"expected"), self.cache,
                                  lipo=False, archs=("arm64",))
        build.assert_not_called()

    def test_ffmpeg_cannot_be_built_off_apple_silicon_without_an_override(self):
        tarball = b"t"
        with patch("fetch_tools.urllib.request.urlopen", return_value=FakeResponse(tarball)), \
             patch("fetch_tools.ffmpeg_lgpl.build", side_effect=fetch_tools.ffmpeg_lgpl.BuildUnavailable("needs a Mac")), \
             patch.dict("os.environ", {}, clear=False):
            with self.assertRaises(fetch_tools.ToolUnavailable):
                fetch_tools.build(Path(self.tmp.name) / "o", self.ffmpeg_lock(tarball), self.cache,
                                  lipo=False, archs=("arm64",))

    def test_ffmpeg_override_is_verified_and_a_gpl_binary_is_rejected(self):
        binary = Path(self.tmp.name) / "prebuilt-ffmpeg"
        binary.write_bytes(b"\0--enable-static --enable-gpl --enable-libx264\0libavcodec license: GPL version 2 or later\0")
        with patch.dict("os.environ", {"FFMPEG_LGPL_BINARY": str(binary)}), \
             patch("fetch_tools.urllib.request.urlopen") as get:
            with self.assertRaises(fetch_tools.ffmpeg_lgpl.LicenseError):
                fetch_tools.build(Path(self.tmp.name) / "o", self.ffmpeg_lock(b"t"), self.cache,
                                  lipo=False, archs=("arm64",))
        get.assert_not_called()

    def test_ffmpeg_has_no_intel_build(self):
        with self.assertRaises(fetch_tools.ToolUnavailable):
            fetch_tools.build(Path(self.tmp.name) / "o", self.ffmpeg_lock(b"t"), self.cache,
                              lipo=False, archs=("arm64", "x86_64"))


if __name__ == "__main__":
    unittest.main()
