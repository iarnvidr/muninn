#!/usr/bin/env python3
"""Restore the exact published 0.2.14 app and source files for DMG packaging.

This never rebuilds, re-signs or changes the published updater archive/manifest.
The download directory is populated by gh release download on a hosted Mac.
"""
import base64
import hashlib
import json
from pathlib import Path, PurePosixPath
import shutil
import sys
import tarfile

ARCHIVE_SHA256 = "bee02a41b60bdaa1a4dc3df3d1498a3df82dff838f2fa373ee97a7180a239a5d"


def restore_sources(root):
    root = Path(root)
    target = root / "corresponding-source"
    target.mkdir()
    entries = json.loads((root / "source-assets-manifest.json").read_text())
    seen = set()
    for entry in entries:
        relative = PurePosixPath(entry["original_path"])
        name = entry["asset_name"]
        if relative.is_absolute() or ".." in relative.parts or not relative.parts or str(relative) in seen:
            raise ValueError("unsafe or duplicate corresponding-source path")
        if Path(name).name != name or not name.startswith("source-"):
            raise ValueError("unsafe source asset name")
        seen.add(str(relative))
        source = root / name
        if hashlib.sha256(source.read_bytes()).hexdigest() != entry["sha256"]:
            raise ValueError("published source digest mismatch")
        destination = target / str(relative)
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source, destination)
    if "manifest.json" not in seen:
        raise ValueError("missing original corresponding-source manifest")
    return target


def main():
    root, config = map(Path, sys.argv[1:])
    archive = root / "Muninn.app.tar.gz"
    if hashlib.sha256(archive.read_bytes()).hexdigest() != ARCHIVE_SHA256:
        raise ValueError("archive differs from the published 0.2.14 payload")
    manifest = json.loads((root / "latest.json").read_text())
    assert manifest["version"] == "0.2.14"
    platform = manifest["platforms"]["darwin-aarch64"]
    assert platform["url"] == "https://github.com/iarnvidr/muninn/releases/download/v0.2.14/Muninn.app.tar.gz"
    (root / "updater.minisig").write_bytes(base64.b64decode(platform["signature"], validate=True))
    (root / "updater.pub").write_bytes(base64.b64decode(json.loads(config.read_text())["plugins"]["updater"]["pubkey"], validate=True))
    # Signature verification must happen before extracting or packaging the app.
    import subprocess
    subprocess.run(["minisign", "-Vm", str(archive), "-p", str(root / "updater.pub"), "-x", str(root / "updater.minisig")], check=True)
    with tarfile.open(archive) as bundle:
        bundle.extractall(root, filter="data")
    restore_sources(root)
    print("Restored exact published app, verified updater signature and original corresponding sources")


if __name__ == "__main__":
    main()
