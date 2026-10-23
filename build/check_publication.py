#!/usr/bin/env python3
"""Enforce the standalone repository boundary; not a replacement for gitleaks."""
import json
import re
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[1]
# Absolute per-user paths under a home or Users directory; a temp-dir home is fine.
HOME_PATH = re.compile(r"(?<![A-Za-z0-9_.-])/(?:home|Users)/(?!runner/|user/|you/)[A-Za-z0-9._-]+/")


def main():
    files = subprocess.check_output(["git", "ls-files", "-z"], cwd=ROOT).decode().split("\0")
    problems = []
    for name in filter(None, files):
        path = ROOT / name
        if path.suffix in (".sqlite", ".sqlite3", ".db", ".jsonl", ".zip", ".pem", ".key"):
            problems.append(f"private/generated data must not be tracked: {name}")
        if path.suffix in (".rs", ".md", ".sh", ".py", ".json", ".yml", ".yaml", ".html", ".js", ".plist", ".toml", ".txt"):
            text = path.read_text(errors="replace")
            if any(value in text.lower() for value in (
                "ginnunga" + "gap", "rode" + "kors.dk", "efthomsen" + "/muninn",
            )):
                problems.append(f"private or obsolete publication reference: {name}")
            if HOME_PATH.search(text):
                problems.append(f"personal home-directory path: {name}")
            if name.startswith(".github/workflows/") and any(s in text for s in ("self" + "-hosted", "pull_request" + "_target")):
                problems.append(f"unsafe public CI boundary: {name}")
    cfg = json.loads((ROOT / "src-tauri/tauri.conf.json").read_text())
    if cfg["identifier"] != "io.github.iarnvidr.muninn":
        problems.append("application identifier changed: data directories follow it")
    if problems:
        raise SystemExit("\n".join(problems))
    print(f"Standalone boundary checks passed ({len(files) - 1} tracked files).")


if __name__ == "__main__":
    main()
