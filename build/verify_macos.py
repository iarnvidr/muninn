#!/usr/bin/env python3
"""Check cross-built bundle metadata, architectures and ad-hoc page hashes.

rcodesign 0.29's `verify` rejects ad-hoc signatures for lacking a CMS certificate
and warns that its results are unreliable. Check the embedded CodeDirectory
hashes directly; this is build integrity validation, not a native launch test.
"""

import hashlib
from pathlib import Path
import plistlib
import struct
import subprocess
import sys
import tempfile


def check_helpers(bin_dir, expected):
    """The tools that make Muninn zero-setup must be inside the bundle, executable, for every arch."""
    for name in ("yt-dlp", "ffmpeg", "deno"):
        tool = bin_dir / name
        if not tool.is_file():
            raise ValueError(f"missing bundled tool: {name}")
        if not tool.stat().st_mode & 0o111:
            raise ValueError(f"bundled tool is not executable: {name}")
        archs = subprocess.check_output(["llvm-lipo-14", "-archs", str(tool)], text=True).split()
        # yt-dlp_macos is always a universal2 binary; ffmpeg and deno follow the app.
        want = ["arm64", "x86_64"] if name == "yt-dlp" else expected
        if sorted(archs) != sorted(want):
            raise ValueError(f"{name}: expected architectures {want}; got {archs}")
    print(f"PASS bundled tools: yt-dlp, ffmpeg, deno for {' + '.join(expected)}")


def verify(app, architectures):
    contents = app / "Contents"
    info_bytes = (contents / "Info.plist").read_bytes()
    info = plistlib.loads(info_bytes)
    if info["CFBundleIdentifier"] != "io.github.iarnvidr.muninn":
        raise ValueError("unexpected bundle identifier")
    if info.get("LSMinimumSystemVersion") != "14.0":
        raise ValueError("the app needs macOS 14 (per-web-view proxy); LSMinimumSystemVersion must be 14.0")
    if not info.get("NSAppTransportSecurity", {}).get("NSAllowsLocalNetworking"):
        raise ValueError("NSAllowsLocalNetworking is required for the local UI server")
    binary = contents / "MacOS" / info["CFBundleExecutable"]
    actual = subprocess.check_output(["llvm-lipo-14", "-archs", str(binary)], text=True).split()
    expected = [{"aarch64": "arm64", "x86_64": "x86_64"}[arch] for arch in architectures]
    if sorted(actual) != sorted(expected):
        raise ValueError(f"expected architectures {expected}; got {actual}")
    check_helpers(contents / "Resources" / "bin", expected)

    resources_bytes = (contents / "_CodeSignature/CodeResources").read_bytes()
    resources = plistlib.loads(resources_bytes)
    for name, entry in resources["files2"].items():
        if "hash2" not in entry:
            raise ValueError(f"resource has no SHA-256 seal: {name}")
        if hashlib.sha256((contents / name).read_bytes()).digest() != entry["hash2"]:
            raise ValueError(f"resource hash mismatch: {name}")

    for arch in actual:
        with tempfile.TemporaryDirectory() as directory:
            thin = binary
            if len(actual) > 1:
                thin = Path(directory) / arch
                subprocess.run(["llvm-lipo-14", str(binary), "-thin", arch, "-output", str(thin)], check=True)
            code = thin.read_bytes()
            cd = subprocess.check_output(["rcodesign", "extract", "code-directory-raw", str(thin)])
            # Layout and slot constants: Apple's xnu/osfmk/kern/cs_blobs.h.
            magic, length, version, flags, offset, identity, special, slots, limit, size, kind, platform, power = struct.unpack_from(
                ">9I4B", cd
            )
            if magic != 0xFADE0C02 or length != len(cd) or not flags & 2 or (size, kind) != (32, 2):
                raise ValueError(f"unexpected ad-hoc CodeDirectory: {arch}")
            if power > 20 or limit > len(code) or not limit or special < 3 or offset < special * size:
                raise ValueError(f"invalid CodeDirectory bounds: {arch}")
            page = 1 << power
            if slots != (limit + page - 1) // page or offset + slots * size > len(cd):
                raise ValueError(f"invalid CodeDirectory slots: {arch}")
            for index in range(slots):
                digest = hashlib.sha256(code[index * page:min((index + 1) * page, limit)]).digest()
                if digest != cd[offset + index * size:offset + (index + 1) * size]:
                    raise ValueError(f"code hash mismatch: {arch}, page {index}")
            for slot, data in ((1, info_bytes), (3, resources_bytes)):
                if hashlib.sha256(data).digest() != cd[offset - slot * size:offset - (slot - 1) * size]:
                    raise ValueError(f"bundle seal mismatch: {arch}, slot {slot}")
        print(f"PASS {arch}: ad-hoc signature page hashes and bundle seals")
    print(f"PASS {info['CFBundleName']} {info['CFBundleShortVersionString']}: metadata and resources")


if __name__ == "__main__":
    verify(Path(sys.argv[1]), sys.argv[2:])
