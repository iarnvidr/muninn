#!/usr/bin/env python3
"""Validated workflow_run metadata for the privileged, data-only GHCR copier.

The publisher checks out DEFAULT-BRANCH code, never a PR's scripts. It treats
downloaded workflow artifacts exclusively as bytes copied into an OCI image.
"""

import json
import re
import sys
from pathlib import Path

REPOSITORY = "iarnvidr/muninn"
SHA = re.compile(r"[0-9a-f]{40}\Z")


def package_metadata(event):
    run = event["workflow_run"]
    if event["repository"]["full_name"] != REPOSITORY:
        raise ValueError("wrong workflow repository")
    if run["head_repository"]["full_name"] != REPOSITORY:
        raise ValueError("untrusted fork: no privileged publication")
    if run.get("status") != "completed":
        raise ValueError("workflow has not completed")

    sha = run["head_sha"].lower()
    if not SHA.fullmatch(sha):
        raise ValueError("invalid source commit SHA")
    run_id, attempt = int(run["id"]), int(run["run_attempt"])
    if not 0 < run_id < 2**63 or not 0 < attempt < 100_000:
        raise ValueError("invalid run identifier")

    workflow, trigger = run["name"], run["event"]
    if trigger not in {"pull_request", "push", "workflow_dispatch"}:
        raise ValueError("unexpected workflow trigger")
    if workflow == "Checks":
        image = "ghcr.io/iarnvidr/muninn-ci-cache"
        artifact = f"muninn-linux-cargo-snapshot-{run_id}-{attempt}"
        kind = "cargo-cache-v1"
    elif workflow == "Apple Silicon build" and run.get("conclusion") == "success":
        image = "ghcr.io/iarnvidr/muninn-macos-candidate"
        artifact = "muninn-macos-arm64"
        kind = "macos-candidate-v1"
    else:
        raise ValueError("workflow has no publishable artifact")

    # The workflow run/attempt is the precise immutable-by-convention locator.
    # A PR's head SHA may be tested against a moving base as a merge ref, so a
    # sha-* label alone must not be treated as release/check provenance.
    tags = [f"run-{run_id}-{attempt}", f"sha-{sha}"]
    if trigger == "pull_request":
        pull_requests = run.get("pull_requests") or []
        if len(pull_requests) == 1:
            pr = pull_requests[0].get("number")
            if type(pr) is int and pr > 0:
                tags.append(f"pr-{pr}")
    elif trigger == "push" and run.get("head_branch") == "main" and run.get("conclusion") == "success":
        tags.append("main")

    return {
        "artifact": artifact,
        "image": image,
        "kind": kind,
        "sha": sha,
        "tags": " ".join(tags),
    }


def main():
    if len(sys.argv) != 2:
        raise SystemExit("usage: package_metadata.py EVENT_JSON")
    event = json.loads(Path(sys.argv[1]).read_text())
    try:
        outputs = package_metadata(event)
    except (KeyError, ValueError, TypeError) as exc:
        raise SystemExit(f"GHCR publication blocked: {exc}") from exc
    for key, value in outputs.items():
        print(f"{key}={value}")


if __name__ == "__main__":
    main()
