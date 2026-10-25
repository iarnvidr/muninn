#!/usr/bin/env python3
"""Prepare verified corresponding-source assets for FFmpeg and dav1d.

This is a narrowly scoped source-asset preparation helper, not a license audit
or a determination that a release satisfies all distribution obligations. It
performs no network activity unless explicitly invoked with ``prepare``.
"""
import argparse
import hashlib
import json
import shutil
import sys
import tempfile
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def pinned_sources(root=ROOT):
    """Read exact FFmpeg/dav1d pins already maintained by ffmpeg_lgpl.py."""
    sys.path.insert(0, str(Path(root) / "build"))
    import ffmpeg_lgpl

    tools = json.loads((Path(root) / "build" / "tools.lock.json").read_text())
    ffmpeg = tools["ffmpeg"]["arm64"]
    return [
        {"name": "ffmpeg", "version": tools["ffmpeg"]["version"], "url": ffmpeg["url"], "sha256": ffmpeg["sha256"]},
        {"name": "dav1d", "version": ffmpeg_lgpl.DAV1D_VERSION, "url": ffmpeg_lgpl.DAV1D_URL,
         "sha256": ffmpeg_lgpl.DAV1D_SHA256},
    ]


def _verified_download(source, opener=urllib.request.urlopen):
    request = urllib.request.Request(source["url"], headers={"User-Agent": "muninn-corresponding-source"})
    with opener(request, timeout=300) as response:
        data = response.read()
    digest = hashlib.sha256(data).hexdigest()
    if digest != source["sha256"]:
        raise ValueError(f"{source['name']} archive SHA-256 mismatch: expected {source['sha256']}, got {digest}")
    return data


def prepare(destination, root=ROOT, opener=urllib.request.urlopen):
    """Download explicitly requested pinned archives and write assets+manifest."""
    root, destination = Path(root), Path(destination)
    sources = pinned_sources(root)
    # Never write into a caller-owned directory. Build the complete release in a
    # private sibling, then publish it with one directory rename.
    destination.parent.mkdir(parents=True, exist_ok=True)
    if destination.exists() or destination.is_symlink():
        raise FileExistsError(f"destination already exists: {destination}")
    staging = Path(tempfile.mkdtemp(prefix=f".{destination.name}.staging-", dir=destination.parent))
    try:
        assets = []
        for source in sources:
            filename = source["url"].rsplit("/", 1)[-1]
            data = _verified_download(source, opener)
            (staging / filename).write_bytes(data)
            assets.append({**source, "asset": filename})

        shutil.copy2(root / "build" / "ffmpeg_lgpl.py", staging / "ffmpeg_lgpl.py")
        notices = staging / "notices"
        notices.mkdir()
        for name in ("ffmpeg-COPYING.LGPLv2.1", "dav1d-COPYING"):
            shutil.copy2(root / "licenses" / name, notices / name)
        manifest = {
            "format": "muninn-corresponding-source-v1",
            "scope": "Pinned FFmpeg and dav1d source archives and associated build recipe/notices only; not a complete license-compliance determination.",
            "sources": assets,
            "build_recipe": "ffmpeg_lgpl.py",
            "license_notices": ["notices/ffmpeg-COPYING.LGPLv2.1", "notices/dav1d-COPYING"],
        }
        (staging / "manifest.json").write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
        if destination.exists() or destination.is_symlink():
            raise FileExistsError(f"destination already exists: {destination}")
        staging.rename(destination)
        return manifest
    finally:
        if staging.exists():
            shutil.rmtree(staging)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    command = sub.add_parser("prepare", help="download and verify pinned source archives")
    command.add_argument("destination", type=Path)
    args = parser.parse_args(argv)
    try:
        manifest = prepare(args.destination)
    except (OSError, ValueError, KeyError) as exc:
        parser.exit(1, f"release source preparation failed: {exc}\n")
    print(json.dumps(manifest, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
