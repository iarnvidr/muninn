#!/usr/bin/env python3
"""Reproducible LGPL-only FFmpeg for macOS arm64, built from a pinned source tarball.

No prebuilt macOS arm64 FFmpeg with published LGPL-only configure flags exists, and
the common static builds are GPL (--enable-gpl, libx264, libx265). This module builds
FFmpeg from the official source release, checks that the result really is LGPL-only,
and exposes the configure string for the third-party notices.

  ffmpeg_lgpl.py verify BINARY     check an existing binary and print its configure line

Library use: build(tarball_bytes) -> binary bytes (macOS arm64 only), verify(path).
The tarball itself is downloaded and SHA-256-checked by the caller's tool pins.

Notes on the configuration:
* --disable-gpl --disable-nonfree and no --enable-version3: LGPL 2.1 or later.
* --disable-autodetect plus an explicit list: Apple system frameworks, zlib and
  libdav1d (BSD-2-Clause, built here from a pinned source release and linked
  statically) are the only libraries. No x264, x265, fdk-aac, OpenSSL or other
  third-party library. dav1d provides software AV1 decoding.
* h264_videotoolbox / aac_at are Apple's hardware and system encoders; Secure
  Transport provides https. VideoToolbox availability depends on the Mac.
"""
import hashlib
import io
import multiprocessing
import os
import platform
import shutil
import subprocess
import sys
import tarfile
import tempfile
import urllib.request
from pathlib import Path

CONFIGURE_FLAGS = [
    "--enable-static",
    "--disable-shared",
    "--disable-gpl",
    "--disable-nonfree",
    "--disable-autodetect",
    "--disable-doc",
    "--disable-debug",
    "--disable-ffplay",
    "--disable-ffprobe",
    "--enable-zlib",
    "--enable-securetransport",
    "--enable-videotoolbox",
    "--enable-audiotoolbox",
    "--enable-libdav1d",
    "--pkg-config-flags=--static",
]
# dav1d (BSD-2-Clause) official source release; the digest is the publisher's own
# dav1d-1.5.4.tar.xz.sha256 from downloads.videolan.org.
DAV1D_VERSION = "1.5.4"
DAV1D_URL = "https://downloads.videolan.org/pub/videolan/dav1d/1.5.4/dav1d-1.5.4.tar.xz"
DAV1D_SHA256 = "686616b7c69eb88d44459391ab25cac13b6647a3b288835c5784e71c1514a5c5"
DAV1D_MESON_ARGS = [
    "--buildtype=release",
    "--default-library=static",
    "--libdir=lib",
    "-Denable_tools=false",
    "-Denable_tests=false",
    "-Denable_examples=false",
    "-Denable_docs=false",
]
BUILD_TOOLS = ("meson", "ninja", "pkg-config")
TARGET_FLAGS = [
    "--arch=arm64",
    "--extra-cflags=-mmacosx-version-min=14.0",
    "--extra-ldflags=-mmacosx-version-min=14.0",
]
FORBIDDEN_IN_CONFIGURATION = (
    "--enable-gpl",
    "--enable-nonfree",
    "--enable-version3",
    "--enable-libx264",
    "--enable-libx265",
    "--enable-libxvid",
    "--enable-libfdk-aac",
    "--enable-openssl",
)


class LicenseError(Exception):
    pass


class BuildUnavailable(Exception):
    pass


def configure_args():
    return [*CONFIGURE_FLAGS, *TARGET_FLAGS]


def fetch_dav1d():
    """The pinned dav1d source tarball, verified against its SHA-256."""
    request = urllib.request.Request(DAV1D_URL, headers={"User-Agent": "ffmpeg-lgpl-build"})
    with urllib.request.urlopen(request, timeout=300) as response:
        data = response.read()
    if hashlib.sha256(data).hexdigest() != DAV1D_SHA256:
        raise RuntimeError("dav1d source tarball does not match the pinned SHA-256")
    return data


def _extract(tarball, destination):
    with tarfile.open(fileobj=io.BytesIO(tarball), mode="r:xz") as archive:
        archive.extractall(destination, filter="data")
    roots = [p for p in destination.iterdir() if p.is_dir()]
    if len(roots) != 1:
        raise RuntimeError("FFmpeg source archive has an unexpected layout")
    return roots[0]


def _run(args, cwd, output, env=None):
    result = subprocess.run(args, cwd=cwd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, env=env)
    if result.returncode != 0:
        print(result.stdout[-4000:], file=output or sys.stderr)
        raise RuntimeError(f"{' '.join(args[:2])} failed with exit status {result.returncode}")
    return result.stdout


def build_dav1d(destination, prefix, output=None, extra_meson_args=()):
    """Build dav1d statically from the pinned source into `prefix`; returns the env to use."""
    source = _extract(fetch_dav1d(), destination)
    env = dict(os.environ, MACOSX_DEPLOYMENT_TARGET="14.0")
    _run(["meson", "setup", "build", f"--prefix={prefix}", *DAV1D_MESON_ARGS, *extra_meson_args], source, output, env)
    _run(["ninja", "-C", "build", "install"], source, output, env)
    return dict(env, PKG_CONFIG_PATH=str(Path(prefix) / "lib" / "pkgconfig"))


def build(tarball, jobs=None, output=None):
    """Compile `ffmpeg` from the source tarball bytes and return the verified binary bytes."""
    if sys.platform != "darwin" or platform.machine() != "arm64":
        raise BuildUnavailable(
            "The LGPL FFmpeg is built on an Apple Silicon Mac (Xcode Command Line Tools). "
            "On another host, build it on a Mac with this script and pass the verified binary "
            "through the FFMPEG_LGPL_BINARY environment variable."
        )
    missing_tools = [tool for tool in BUILD_TOOLS if shutil.which(tool) is None]
    if missing_tools:
        raise BuildUnavailable(
            "Missing build tools: " + ", ".join(missing_tools) + ". Install them with: brew install meson ninja pkgconf"
        )
    with tempfile.TemporaryDirectory(prefix="ffmpeg-lgpl-") as work:
        work = Path(work)
        (work / "dav1d-src").mkdir()
        (work / "ffmpeg-src").mkdir()
        env = build_dav1d(work / "dav1d-src", work / "dav1d-prefix", output)
        source = _extract(tarball, work / "ffmpeg-src")
        _run(["./configure", *configure_args()], source, output, env)
        _run(["make", f"-j{jobs or multiprocessing.cpu_count()}", "ffmpeg"], source, output, env)
        binary = source / "ffmpeg"
        verify(binary)
        missing = verify_capabilities(binary)
        if missing:
            raise RuntimeError("built FFmpeg lacks: " + ", ".join(missing))
        return binary.read_bytes()


def _capture(binary, *args):
    result = subprocess.run([str(binary), "-hide_banner", *args], stdin=subprocess.DEVNULL,
                            stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, timeout=60)
    return result.stdout


def static_configuration(data):
    """The configure string embedded in an FFmpeg binary, read without running it."""
    for chunk in data.split(b"\0"):
        if b"--disable-gpl" in chunk or b"--enable-gpl" in chunk:
            text = chunk.decode("ascii", "replace")
            return text[text.index("--"):].strip()
    return None


def _check_flags(configuration):
    flags = configuration.split()
    for forbidden in FORBIDDEN_IN_CONFIGURATION:
        if forbidden in flags:
            raise LicenseError(f"configure string contains {forbidden}")
    if "--disable-gpl" not in flags:
        raise LicenseError("configure string does not contain --disable-gpl")


def verify(binary):
    """Raise LicenseError unless `binary` is an LGPL-only build. Returns its configure string.

    A binary that can run here (macOS) is asked for `-version` and `-L`. Otherwise,
    such as in a Linux cross-build container, the embedded strings are checked.
    """
    binary = Path(binary)
    data = binary.read_bytes()
    embedded = static_configuration(data)
    if embedded is None:
        raise LicenseError("no FFmpeg configure string found in the binary")
    _check_flags(embedded)
    if b"libavcodec license: LGPL version 2.1 or later" not in data:
        raise LicenseError("the binary does not declare LGPL 2.1 or later")
    if sys.platform != "darwin":
        return embedded
    version = _capture(binary, "-version")
    configuration = next((line[len("configuration:"):].strip() for line in version.splitlines()
                          if line.startswith("configuration:")), None)
    if configuration is None:
        raise LicenseError("could not read the configure string from `ffmpeg -version`")
    _check_flags(configuration)
    license_text = _capture(binary, "-L")
    if "GNU Lesser General Public License" not in license_text:
        raise LicenseError("`ffmpeg -L` does not report the LGPL")
    if "GNU General Public License" in license_text.replace("GNU Lesser General Public License", ""):
        raise LicenseError("`ffmpeg -L` mentions the GPL")
    return configuration


def verify_capabilities(binary):
    """Features the apps rely on: VideoToolbox H.264 encoding, AV1 via dav1d, https. Returns missing names."""
    missing = []
    if "libdav1d" not in _capture(binary, "-decoders"):
        missing.append("libdav1d decoder")
    if "h264_videotoolbox" not in _capture(binary, "-encoders"):
        missing.append("h264_videotoolbox encoder")
    if not any(line.split()[:1] == ["https"] for line in _capture(binary, "-protocols").splitlines()):
        missing.append("https protocol")
    return missing


if __name__ == "__main__":
    if len(sys.argv) == 3 and sys.argv[1] == "verify":
        try:
            print(verify(Path(sys.argv[2])))
        except LicenseError as error:
            sys.exit(f"NOT LGPL-only: {error}")
        missing = verify_capabilities(Path(sys.argv[2]))
        if missing:
            sys.exit("missing required capabilities: " + ", ".join(missing))
    else:
        sys.exit(__doc__)
