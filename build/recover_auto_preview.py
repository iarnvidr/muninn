#!/usr/bin/env python3
"""Find a previously accepted, interrupted draft and its retained signed review.

Runs only on trusted pushes to main. A later CI-only fix can resume an older
draft without retagging it to the fix commit or rebuilding the native app.
"""

import argparse
import json
from pathlib import Path
import re
import subprocess

import auto_preview_release as preview
import release_candidate


MARKER = re.compile(r"<!-- muninn-auto-preview source=([0-9a-f]{40}) mapping=([1-9][0-9]*) -->")
WORKFLOW = ".github/workflows/auto-preview-release.yml"


def ancestor(root, source):
    return subprocess.run(
        ["git", "merge-base", "--is-ancestor", source, "HEAD"],
        cwd=root, check=False, capture_output=True,
    ).returncode == 0


def decision(root, api_fn=release_candidate.api, git_fn=release_candidate.git,
             ancestor_fn=ancestor):
    version = release_candidate.version(root)
    if not preview.updater_release.stable(version):
        raise ValueError("cannot recover a non-numeric preview version")
    tag = f"v{version}"
    rows = preview.all_releases(api_fn)
    same_tag = [row for row in rows if row.get("tag_name") == tag]
    if len(same_tag) > 1:
        raise ValueError("ambiguous releases for the current version")
    if not same_tag or not same_tag[0].get("draft"):
        return {"recover_needed": False}

    draft = same_tag[0]
    found = MARKER.findall(draft.get("body") or "")
    if len(found) != 1 or not draft.get("prerelease"):
        raise ValueError("interrupted draft lacks a unique trusted preview marker")
    source_sha, mapping_run_id = found[0]
    current = git_fn(root, "rev-parse", "HEAD^{commit}")
    main = api_fn(f"repos/{preview.REPOSITORY}/git/ref/heads/main")["object"]["sha"]
    if main != current or not ancestor_fn(root, source_sha):
        raise ValueError("draft source is not an ancestor of the current main head")
    tag_ref = api_fn(f"repos/{preview.REPOSITORY}/git/ref/tags/{tag}")["object"]
    if tag_ref.get("type") != "commit" or tag_ref.get("sha") != source_sha:
        raise ValueError("immutable draft tag does not match its accepted source")

    mapping = api_fn(f"repos/{preview.REPOSITORY}/actions/runs/{mapping_run_id}")
    if (mapping.get("repository", {}).get("full_name") != preview.REPOSITORY
            or mapping.get("path") != preview.MAPPING_WORKFLOW
            or mapping.get("event") != "push" or mapping.get("head_branch") != "main"
            or mapping.get("head_sha") != source_sha or mapping.get("conclusion") != "success"):
        raise ValueError("draft mapping is not a successful trusted main acceptance")

    artifact_name = f"muninn-auto-preview-{source_sha}"
    candidates = api_fn(
        f"repos/{preview.REPOSITORY}/actions/artifacts?name={artifact_name}&per_page=100"
    )["artifacts"]
    matching = [a for a in candidates if a.get("name") == artifact_name
                and not a.get("expired")
                and a.get("workflow_run", {}).get("head_sha") == source_sha]
    if len(matching) != 1:
        raise ValueError("interrupted draft has no unique retained review artifact")
    run_id = matching[0]["workflow_run"]["id"]
    run = api_fn(f"repos/{preview.REPOSITORY}/actions/runs/{run_id}")
    if (run.get("repository", {}).get("full_name") != preview.REPOSITORY
            or run.get("path") != WORKFLOW or run.get("event") != "workflow_run"
            or run.get("head_sha") != source_sha):
        raise ValueError("review artifact does not originate from the trusted preview workflow")
    jobs = api_fn(f"repos/{preview.REPOSITORY}/actions/runs/{run_id}/jobs?per_page=100")["jobs"]
    if not any(j.get("name") == "package" and j.get("conclusion") == "success" for j in jobs):
        raise ValueError("the signed package was not produced by a successful review job")
    return {
        "recover_needed": True,
        "version": version, "tag": tag,
        "source_sha": source_sha, "mapping_run_id": mapping_run_id,
        "review_run_id": run_id, "artifact_name": artifact_name,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--github-output", type=Path, required=True)
    args = parser.parse_args()
    result = decision(args.root)
    with args.github_output.open("a") as out:
        for key, value in result.items():
            out.write(f"{key}={str(value).lower() if isinstance(value, bool) else value}\n")
    print("No interrupted draft to recover." if not result["recover_needed"]
          else f"Recovering verified {result['tag']} from accepted {result['source_sha']}")


if __name__ == "__main__":
    try:
        main()
    except (ValueError, KeyError, OSError, json.JSONDecodeError) as exc:
        raise SystemExit(f"automatic draft recovery blocked: {exc}") from exc
