#!/usr/bin/env python3
"""Fail-closed gates for automatic, signed, accepted-main preview releases.

The trusted workflow_run consumer is only a publication trigger. The actual
native app and Dagger check evidence are independently verified by
release_candidate.py before any package is signed or published.
"""

import argparse
import hashlib
import json
from pathlib import Path
import re

import release_candidate
import updater_release


REPOSITORY = release_candidate.REPOSITORY
MAPPING_WORKFLOW = release_candidate.MAPPING_WORKFLOW
SHA = re.compile(r"[0-9a-f]{40}\Z")
TAG = re.compile(r"v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)\Z")


def version_key(value):
    match = TAG.fullmatch(value)
    return tuple(map(int, match.groups())) if match else None


def release_versions(api_fn):
    """Enumerate every published, numeric release; drafts are not installable."""
    versions = []
    for page in range(1, 21):
        rows = api_fn(f"repos/{REPOSITORY}/releases?per_page=100&page={page}")
        if not isinstance(rows, list):
            raise ValueError("GitHub release listing is malformed")
        for release in rows:
            if not release.get("draft") and version_key(release.get("tag_name", "")) is not None:
                versions.append(release)
        if len(rows) < 100:
            return versions
    raise ValueError("GitHub release listing exceeded pagination limit")


def decision(event, checkout_sha, current_main_sha, source_version, published):
    run = event.get("workflow_run") or {}
    required = {
        "path": MAPPING_WORKFLOW, "event": "push", "head_branch": "main",
        "status": "completed", "conclusion": "success",
        "head_sha": checkout_sha,
    }
    if any(run.get(name) != value for name, value in required.items()):
        raise ValueError("trigger is not a successful main acceptance-mapping run for this checkout")
    if run.get("repository", {}).get("full_name") != REPOSITORY:
        raise ValueError("acceptance mapping came from a different repository")
    head_repository = run.get("head_repository")
    if head_repository is not None and head_repository.get("full_name") != REPOSITORY:
        raise ValueError("acceptance mapping used an external head repository")
    if not isinstance(run.get("id"), int) or run["id"] < 1:
        raise ValueError("acceptance mapping has no valid run id")
    if not SHA.fullmatch(checkout_sha) or current_main_sha != checkout_sha:
        raise ValueError("accepted merge is no longer the current main commit")
    if not updater_release.stable(source_version):
        raise ValueError("source version is not a numeric SemVer release")

    expected_tag = f"v{source_version}"
    current_key = version_key(expected_tag)
    previous = [(version_key(row["tag_name"]), row) for row in published]
    if any(key is None or row.get("draft") for key, row in previous):
        raise ValueError("release listing contained invalid entries")
    latest_key = max((key for key, _ in previous), default=None)
    if latest_key is not None and current_key < latest_key:
        raise ValueError("source version regressed behind an already published version")
    already_released = [row for key, row in previous if key == current_key]
    if len(already_released) > 1:
        raise ValueError("multiple published releases have the same numeric version")
    if already_released:
        published_assets = {asset.get("name") for asset in already_released[0].get("assets", [])}
        required_assets = {f"Muninn_{source_version}_macos-arm64.dmg", "Muninn.app.tar.gz", "latest.json"}
        if published_assets != required_assets:
            raise ValueError("existing published version is missing its exact updater assets")
    return {
        "release_needed": not bool(already_released),
        "version": source_version,
        "tag": expected_tag,
        "source_sha": checkout_sha,
        "mapping_run_id": run["id"],
    }


def plan(root, event, api_fn=release_candidate.api):
    checkout_sha = release_candidate.git(root, "rev-parse", "HEAD^{commit}")
    current_main_sha = api_fn(f"repos/{REPOSITORY}/git/ref/heads/main")["object"]["sha"]
    source_version = release_candidate.version(root)
    return decision(event, checkout_sha, current_main_sha, source_version, release_versions(api_fn))


def validate_assets(directory, version, tag, sha, mapping_run_id):
    """Check the published payload's provenance, manifest and review signatures."""
    if not updater_release.stable(version) or tag != f"v{version}" or not SHA.fullmatch(sha):
        raise ValueError("invalid release version/tag/commit")
    if not str(mapping_run_id).isdigit():
        raise ValueError("invalid acceptance mapping run id")
    directory = Path(directory)
    names = [f"Muninn_{version}_macos-arm64.dmg", "Muninn.app.tar.gz", "latest.json"]
    for name in names + ["Muninn.app.tar.gz.sig", "accepted-native.json", "release-review.txt"]:
        file = directory / name
        if not file.is_file() or not file.stat().st_size:
            raise ValueError(f"missing or empty release-review asset: {name}")
    accepted = json.loads((directory / "accepted-native.json").read_text())
    if (accepted.get("accepted_commit") != sha or accepted.get("version") != version
            or not isinstance(accepted.get("check_receipt"), dict)):
        raise ValueError("release-review accepted-native receipt has incorrect provenance")
    review = (directory / "release-review.txt").read_text().strip()
    if review != f"version={version} tag={tag} commit={sha} mapping_run={mapping_run_id}":
        raise ValueError("release-review metadata does not identify the accepted merge")
    feed = json.loads((directory / "latest.json").read_text())
    updater_release.validate_feed(feed)
    expected_url = f"https://github.com/{REPOSITORY}/releases/download/{tag}/Muninn.app.tar.gz"
    payload = feed["platforms"]["darwin-aarch64"]
    if feed["version"] != version or payload["url"] != expected_url:
        raise ValueError("updater manifest does not point to this immutable version")
    if payload["signature"] != (directory / "Muninn.app.tar.gz.sig").read_text().strip():
        raise ValueError("updater manifest is not bound to the verified detached signature")
    return {name: "sha256:" + hashlib.sha256((directory / name).read_bytes()).hexdigest() for name in names}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="cmd", required=True)
    prepare = commands.add_parser("plan")
    prepare.add_argument("--root", type=Path, required=True)
    prepare.add_argument("--event", type=Path, required=True)
    prepare.add_argument("--github-output", type=Path, required=True)
    verify = commands.add_parser("verify-assets")
    verify.add_argument("--directory", type=Path, required=True)
    verify.add_argument("--version", required=True)
    verify.add_argument("--tag", required=True)
    verify.add_argument("--sha", required=True)
    verify.add_argument("--mapping-run-id", required=True)
    args = parser.parse_args()
    if args.cmd == "plan":
        result = plan(args.root, json.loads(args.event.read_text()))
        with args.github_output.open("a") as outputs:
            for key, value in result.items():
                outputs.write(f"{key}={str(value).lower() if isinstance(value, bool) else value}\n")
    else:
        validate_assets(args.directory, args.version, args.tag, args.sha, args.mapping_run_id)


if __name__ == "__main__":
    try:
        main()
    except (ValueError, KeyError, OSError, json.JSONDecodeError) as exc:
        raise SystemExit(f"automatic preview release blocked: {exc}") from exc
