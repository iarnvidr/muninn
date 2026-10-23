#!/usr/bin/env python3
"""Inventory the exact Cargo dependency graph without exposing registry paths.

Run after cargo fetch. Output is generated JSON, suitable for license review;
this inventory is not a substitute for collecting actual license notices.
"""
import json
import subprocess
import argparse
from pathlib import Path
import shutil


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--collect", type=Path, help="Collect available upstream license/notice files here")
    args = parser.parse_args()
    data = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--locked", "--format-version", "1"], text=True))
    packages = [{"name": p["name"], "version": p["version"],
                 "license": p["license"], "repository": p["repository"],
                 "license_file_declared": bool(p["license_file"])}
                for p in data["packages"] if p["source"]]
    if args.collect:
        for package, row in zip((p for p in data["packages"] if p["source"]), packages):
            base = Path(package["manifest_path"]).parent
            candidates = set()
            for pattern in ("LICENSE*", "LICENCE*", "COPYING*", "NOTICE*", "UNLICENSE*"):
                for path in base.glob(pattern):
                    candidates.update(path.rglob("*") if path.is_dir() else [path])
            if package["license_file"]:
                candidates.add(base / package["license_file"])
            destination = args.collect / (row["name"] + "-" + row["version"])
            copied = []
            for path in sorted(candidates):
                if not path.is_file() or not path.resolve().is_relative_to(base.resolve()):
                    continue
                relative = path.relative_to(base)
                target = destination / relative
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(path, target)
                copied.append(relative.as_posix())
            row["notice_files"] = copied
    print(json.dumps(sorted(packages, key=lambda p: (p["name"], p["version"])), indent=2))


if __name__ == "__main__":
    main()
