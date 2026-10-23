import sys
import tempfile
import unittest
from pathlib import Path

import ffmpeg_lgpl as recipe

LGPL_BINARY = b"\0" + " ".join(recipe.configure_args()).encode() + b"\0libavcodec license: LGPL version 2.1 or later\0"
GPL_BINARY = b"\0--enable-static --enable-gpl --enable-version3 --enable-libx264\0libavcodec license: GPL version 3 or later\0"

# `ffmpeg -L` output for each license branch of show_license() in the pinned
# ffmpeg-9.0.2.tar.xz (fftools/opt_common.c), with the program name filled in.
UPSTREAM_LICENSE_TEXT = {
    'LGPLV2_1': 'ffmpeg is free software; you can redistribute it and/or\nmodify it under the terms of the GNU Lesser General Public\nLicense as published by the Free Software Foundation; either\nversion 2.1 of the License, or (at your option) any later version.\n\nffmpeg is distributed in the hope that it will be useful,\nbut WITHOUT ANY WARRANTY; without even the implied warranty of\nMERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the GNU\nLesser General Public License for more details.\n\nYou should have received a copy of the GNU Lesser General Public\nLicense along with ffmpeg; if not, write to the Free Software\nFoundation, Inc., 51 Franklin Street, Fifth Floor, Boston, MA 02110-1301 USA\n',
    'LGPLV3': 'ffmpeg is free software; you can redistribute it and/or modify\nit under the terms of the GNU Lesser General Public License as published by\nthe Free Software Foundation; either version 3 of the License, or\n(at your option) any later version.\n\nffmpeg is distributed in the hope that it will be useful,\nbut WITHOUT ANY WARRANTY; without even the implied warranty of\nMERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the\nGNU Lesser General Public License for more details.\n\nYou should have received a copy of the GNU Lesser General Public License\nalong with ffmpeg.  If not, see <http://www.gnu.org/licenses/>.\n',
    'GPL': 'ffmpeg is free software; you can redistribute it and/or modify\nit under the terms of the GNU General Public License as published by\nthe Free Software Foundation; either version 2 of the License, or\n(at your option) any later version.\n\nffmpeg is distributed in the hope that it will be useful,\nbut WITHOUT ANY WARRANTY; without even the implied warranty of\nMERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the\nGNU General Public License for more details.\n\nYou should have received a copy of the GNU General Public License\nalong with ffmpeg; if not, write to the Free Software\nFoundation, Inc., 51 Franklin Street, Fifth Floor, Boston, MA 02110-1301 USA\n',
    'GPLV3': 'ffmpeg is free software; you can redistribute it and/or modify\nit under the terms of the GNU General Public License as published by\nthe Free Software Foundation; either version 3 of the License, or\n(at your option) any later version.\n\nffmpeg is distributed in the hope that it will be useful,\nbut WITHOUT ANY WARRANTY; without even the implied warranty of\nMERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the\nGNU General Public License for more details.\n\nYou should have received a copy of the GNU General Public License\nalong with ffmpeg.  If not, see <http://www.gnu.org/licenses/>.\n',
}


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

    def test_license_text_accepts_wrapped_upstream_lgpl_2_1_notice(self):
        text = UPSTREAM_LICENSE_TEXT["LGPLV2_1"]
        # Why matching is whitespace-insensitive: FFmpeg wraps this phrase, so a
        # contiguous-substring check rejected every valid LGPL 2.1 build.
        self.assertNotIn("GNU Lesser General Public License", text)
        recipe.check_license_text(text)

    def test_license_text_rejects_gpl_and_version_3_notices(self):
        for branch in ("GPL", "GPLV3", "LGPLV3"):
            with self.subTest(branch=branch), self.assertRaises(recipe.LicenseError):
                recipe.check_license_text(UPSTREAM_LICENSE_TEXT[branch])
        for text in ("", "ffmpeg version 9.0.2", UPSTREAM_LICENSE_TEXT["LGPLV2_1"].replace("2.1", "2")):
            with self.subTest(text=text[:20]), self.assertRaises(recipe.LicenseError):
                recipe.check_license_text(text)
        mixed = UPSTREAM_LICENSE_TEXT["LGPLV2_1"] + "\nThis build is licensed under the GNU\nGeneral Public License."
        with self.assertRaises(recipe.LicenseError):
            recipe.check_license_text(mixed)

    def test_capture_fails_when_the_binary_fails(self):
        script = self.write(b"#!/bin/sh\necho partial output\nexit 7\n")
        script.chmod(0o755)
        with self.assertRaisesRegex(recipe.LicenseError, "exited with status 7"):
            recipe._capture(script, "-L")
        ok = self.write(b"#!/bin/sh\necho fine\n")
        ok.chmod(0o755)
        self.assertEqual(recipe._capture(ok, "-L").strip(), "fine")

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
