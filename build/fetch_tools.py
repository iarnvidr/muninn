#!/usr/bin/env python3
"""Download, verify and lay out the helper binaries bundled into Muninn.app.

  fetch_tools.py OUT [--cache DIR] [--lock tools.lock.json]

Writes OUT/arm64/{yt-dlp,ffmpeg,deno}, OUT/x86_64/{...} and OUT/universal/{...}
(yt-dlp_macos is already universal2; ffmpeg and deno are joined with lipo), plus
OUT/NOTICE.txt with versions, licenses and where the sources are. Every download
is checked against the SHA-256 pinned in tools.lock.json before anything is
unpacked; a mismatch aborts the build. ffmpeg is compiled from its pinned source
tarball with an LGPL-only configuration (ffmpeg_lgpl.py; Apple Silicon Mac only,
or set FFMPEG_LGPL_BINARY to a binary built that way), so only arm64 is available.
"""
import argparse
import hashlib
import io
import json
import os
import shutil
import subprocess
import sys
import urllib.request
import zipfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import ffmpeg_lgpl  # noqa: E402

ARCHS = ("arm64", "x86_64")
FFMPEG_SOURCE_KIND = "ffmpeg-lgpl-source"


class ChecksumError(Exception):
    pass


def sha256_of(data):
    return hashlib.sha256(data).hexdigest()


def download(url, expected, cache):
    """Bytes of `url`, verified against `expected` (hex sha256). Cached by digest."""
    cache.mkdir(parents=True, exist_ok=True)
    cached = cache / expected
    if cached.exists():
        data = cached.read_bytes()
        if sha256_of(data) == expected:
            return data
        cached.unlink()
    req = urllib.request.Request(url, headers={"User-Agent": "muninn-build"})
    with urllib.request.urlopen(req, timeout=300) as r:
        data = r.read()
    actual = sha256_of(data)
    if actual != expected:
        raise ChecksumError(f"{url}: expected sha256 {expected}, got {actual}")
    tmp = cache / (expected + ".part")
    tmp.write_bytes(data)
    tmp.rename(cached)
    return data


class ToolUnavailable(Exception):
    pass


def build_ffmpeg_lgpl(spec, cache):
    """An LGPL-only ffmpeg: a verified override, a cached earlier build, or a fresh build.

    FFMPEG_LGPL_BINARY names a binary built elsewhere with build/ffmpeg_lgpl.py (for
    example on a Mac, when this runs in a Linux container); it is re-verified.
    """
    override = os.environ.get("FFMPEG_LGPL_BINARY")
    if override:
        ffmpeg_lgpl.verify(Path(override))
        return Path(override).read_bytes()
    recipe = sha256_of((HERE / "ffmpeg_lgpl.py").read_bytes())[:16]
    built = cache / f"ffmpeg-lgpl-{spec['sha256']}-{recipe}"
    if built.is_file():
        try:
            ffmpeg_lgpl.verify(built)
            return built.read_bytes()
        except ffmpeg_lgpl.LicenseError:
            built.unlink()
    try:
        data = ffmpeg_lgpl.build(download(spec["url"], spec["sha256"], cache))
    except ffmpeg_lgpl.BuildUnavailable as e:
        raise ToolUnavailable(str(e))
    cache.mkdir(parents=True, exist_ok=True)
    partial = built.with_suffix(".part")
    partial.write_bytes(data)
    partial.rename(built)
    return data


def produce(tool, arch, spec, cache):
    if spec["kind"] == FFMPEG_SOURCE_KIND:
        return build_ffmpeg_lgpl(spec, cache)
    return extract(download(spec["url"], spec["sha256"], cache), spec)


def extract(data, spec):
    if spec["kind"] == "file":
        return data
    with zipfile.ZipFile(io.BytesIO(data)) as z:
        return z.read(spec["member"])


def lipo_command():
    return "lipo" if sys.platform == "darwin" else "llvm-lipo-14"


def macho_archs(path):
    out = subprocess.check_output([lipo_command(), "-archs", str(path)], text=True)
    return sorted(out.split())


def write_exec(path, data):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)
    path.chmod(0o755)


def build(out, lock, cache, lipo=True, archs=ARCHS):
    for tool, entry in lock.items():  # fail before downloading anything
        if not tool.startswith("_") and "universal" not in entry:
            for arch in archs:
                if arch not in entry:
                    raise ToolUnavailable(f"{tool} has no {arch} build; run with --arch arm64")
    out.mkdir(parents=True, exist_ok=True)
    notice = ["Muninn bundles these third-party tools in Contents/Resources/bin:\n"]
    for tool, entry in lock.items():
        if tool.startswith("_"):
            continue
        notice.append(f"- {tool} {entry['version']}: {entry['license']}\n  source: {entry['source']}\n")
        if any(isinstance(s, dict) and s.get("kind") == FFMPEG_SOURCE_KIND for s in entry.values()):
            notice.append("  configure options: " + " ".join(ffmpeg_lgpl.configure_args()) + "\n  recipe: build/ffmpeg_lgpl.py\n")
        if "universal" in entry:
            data = produce(tool, "universal", entry["universal"], cache)
            for arch in (*archs, *(("universal",) if len(archs) == 2 else ())):
                write_exec(out / arch / tool, data)
            continue
        per_arch = {}
        for arch in archs:
            spec = entry.get(arch)
            if spec is None:
                raise ToolUnavailable(f"{tool} has no {arch} build; run with --arch arm64")
            per_arch[arch] = produce(tool, arch, spec, cache)
            write_exec(out / arch / tool, per_arch[arch])
        if lipo:
            subprocess.check_call([lipo_command(), "-create", str(out / "arm64" / tool), str(out / "x86_64" / tool), "-output", str(out / "universal" / tool)])
            (out / "universal" / tool).chmod(0o755)
    (out / "NOTICE.txt").write_text("\n".join(notice))
    return out


def main(argv=None):
    p = argparse.ArgumentParser()
    p.add_argument("out")
    p.add_argument("--cache", default=os.path.expanduser("~/.cache/muninn-tools"))
    p.add_argument("--lock", default=str(HERE / "tools.lock.json"))
    p.add_argument("--arch", choices=ARCHS, help="Fetch only this architecture; do not create universal binaries")
    args = p.parse_args(argv)
    lock = json.loads(Path(args.lock).read_text())
    archs = (args.arch,) if args.arch else ARCHS
    build(Path(args.out), lock, Path(args.cache), lipo=not args.arch, archs=archs)
    for arch in (*archs, *(("universal",) if not args.arch else ())):
        for tool in ("yt-dlp", "ffmpeg", "deno"):
            f = Path(args.out) / arch / tool
            print(f"{arch}/{tool}: {f.stat().st_size} bytes, archs {macho_archs(f)}, sha256 {sha256_of(f.read_bytes())[:16]}")


if __name__ == "__main__":
    try:
        main()
    except ChecksumError as e:
        print(f"ERROR: {e}", file=sys.stderr)
        sys.exit(2)
    except (ToolUnavailable, ffmpeg_lgpl.LicenseError) as e:
        print(f"ERROR: {e}", file=sys.stderr)
        sys.exit(3)
