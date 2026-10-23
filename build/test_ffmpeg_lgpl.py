import sys
import tempfile
import unittest
from pathlib import Path

import ffmpeg_lgpl as recipe

LGPL_BINARY = b"\0" + " ".join(recipe.configure_args()).encode() + b"\0libavcodec license: LGPL version 2.1 or later\0"
GPL_BINARY = b"\0--enable-static --enable-gpl --enable-version3 --enable-libx264\0libavcodec license: GPL version 3 or later\0"


class RecipeTests(unittest.TestCase):
    def write(self, data):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        path = Path(directory.name) / "ffmpeg"
        path.write_bytes(data)
        return path

    def test_configuration_is_lgpl_only(self):
        args = recipe.configure_args()
        for flag in ("--disable-gpl", "--disable-nonfree", "--disable-autodetect",
                     "--enable-videotoolbox", "--enable-securetransport", "--enable-libdav1d", "--arch=arm64"):
            self.assertIn(flag, args)
        for flag in recipe.FORBIDDEN_IN_CONFIGURATION:
            self.assertNotIn(flag, args)

    @unittest.skipIf(sys.platform == "darwin", "verify() executes the binary on macOS")
    def test_verify_accepts_lgpl_and_rejects_gpl(self):
        self.assertEqual(recipe.verify(self.write(LGPL_BINARY)), " ".join(recipe.configure_args()))
        with self.assertRaises(recipe.LicenseError):
            recipe.verify(self.write(GPL_BINARY))
        with self.assertRaises(recipe.LicenseError):
            recipe.verify(self.write(b"not ffmpeg"))

    def test_static_flag_check(self):
        recipe._check_flags(recipe.static_configuration(LGPL_BINARY))
        with self.assertRaises(recipe.LicenseError):
            recipe._check_flags(recipe.static_configuration(GPL_BINARY))
        self.assertIsNone(recipe.static_configuration(b"nothing here"))

    def test_dav1d_is_pinned_to_a_release_tarball_with_a_sha256(self):
        self.assertRegex(recipe.DAV1D_SHA256, r"^[0-9a-f]{64}$")
        self.assertTrue(recipe.DAV1D_URL.startswith("https://downloads.videolan.org/"))
        self.assertIn(recipe.DAV1D_VERSION, recipe.DAV1D_URL)
        self.assertIn("--default-library=static", recipe.DAV1D_MESON_ARGS)

    def test_a_dav1d_download_that_does_not_match_the_pin_is_rejected(self):
        from unittest import mock

        response = mock.MagicMock()
        response.__enter__.return_value.read.return_value = b"tampered"
        with mock.patch("urllib.request.urlopen", return_value=response):
            with self.assertRaises(RuntimeError):
                recipe.fetch_dav1d()


if __name__ == "__main__":
    unittest.main()
