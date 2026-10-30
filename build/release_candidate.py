#!/usr/bin/env python3
"""Bind an accepted Muninn Apple build to its exact native run and merge tree."""
import argparse
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import re
import subprocess
import tarfile
import tempfile
import urllib.parse
import urllib.request
import zipfile

REPOSITORY = "iarnvidr/muninn"
NATIVE_WORKFLOW = ".github/workflows/macos.yml"
CHECK_WORKFLOW = ".github/workflows/check.yml"
MAPPING_WORKFLOW = ".github/workflows/accept-native.yml"
HELPER_PATH = "build/release_candidate.py"
NATIVE_ARTIFACT_PREFIX = "muninn-native-candidate"
CHECK_ARTIFACT_PREFIX = "muninn-check-candidate"
MAPPING_ARTIFACT_PREFIX = "muninn-accepted-native"
MAX_ARCHIVE_BYTES = 1024 * 1024 * 1024
REQUIRED_CHECKS = ("test", "package")


def sha(data):
    return hashlib.sha256(data).hexdigest()


def git(root, *args):
    return subprocess.run(["git", "-C", str(root), *args], check=True,
                          capture_output=True, text=True).stdout.strip()


def version(root):
    return json.loads((root / "src-tauri/tauri.conf.json").read_text())["version"]


def inventory(app):
    rows = {}
    for base, dirs, files in os.walk(app, followlinks=False):
        dirs.sort()
        files.sort()
        for name in dirs + files:
            path = Path(base) / name
            rel = path.relative_to(app.parent).as_posix()
            st = path.lstat()
            if path.is_symlink():
                target = os.readlink(path)
                resolved = (PurePosixPath(rel).parent / target)
                stack = []
                for part in resolved.parts:
                    if part in ("", "."):
                        continue
                    if part == "..":
                        if not stack:
                            raise ValueError("application symlink escapes bundle")
                        stack.pop()
                    else:
                        stack.append(part)
                if not stack or stack[0] != "Muninn.app":
                    raise ValueError("application symlink escapes bundle")
                rows[rel] = {"type": "symlink", "target": target}
            elif path.is_file():
                rows[rel] = {"type": "file", "mode": st.st_mode & 0o777,
                             "sha256": sha(path.read_bytes())}
    return rows


def create(root):
    app = root / "Muninn.app"
    if not app.is_dir():
        raise ValueError("native producer did not create Muninn.app")
    out = root / "native-candidate"
    out.mkdir(parents=True, exist_ok=True)
    archive = out / "Muninn.app.tar.gz"
    with tarfile.open(archive, "w:gz", format=tarfile.PAX_FORMAT) as bundle:
        bundle.add(app, arcname="Muninn.app", recursive=True)
    event = json.loads(Path(os.environ["GITHUB_EVENT_PATH"]).read_text())
    pr = event.get("pull_request") or {}
    record = {
        "schema": 1,
        "repository": os.environ["GITHUB_REPOSITORY"],
        "workflow_path": NATIVE_WORKFLOW,
        "run_id": int(os.environ["GITHUB_RUN_ID"]),
        "run_attempt": int(os.environ["GITHUB_RUN_ATTEMPT"]),
        "event": os.environ["GITHUB_EVENT_NAME"],
        "workflow_sha": os.environ["GITHUB_WORKFLOW_SHA"],
        "head_sha": pr.get("head", {}).get("sha", os.environ["GITHUB_SHA"]),
        "pr_number": pr.get("number"),
        "base_sha": pr.get("base", {}).get("sha"),
        "tested_tree": git(root, "rev-parse", "HEAD^{tree}"),
        "version": version(root),
        "platform": "darwin-arm64",
        "files": {archive.name: sha(archive.read_bytes())},
        "app_files": inventory(app),
    }
    (out / "native-receipt.json").write_text(json.dumps(record, sort_keys=True) + "\n")


def create_check(root):
    event = json.loads(Path(os.environ["GITHUB_EVENT_PATH"]).read_text())
    pr = event.get("pull_request") or {}
    if not pr:
        raise ValueError("Dagger check receipts are only created for pull requests")
    record = {
        "schema": 1,
        "repository": os.environ["GITHUB_REPOSITORY"],
        "workflow_path": CHECK_WORKFLOW,
        "run_id": int(os.environ["GITHUB_RUN_ID"]),
        "run_attempt": int(os.environ["GITHUB_RUN_ATTEMPT"]),
        "event": os.environ["GITHUB_EVENT_NAME"],
        "workflow_sha": os.environ["GITHUB_WORKFLOW_SHA"],
        "head_sha": pr.get("head", {}).get("sha"),
        "base_sha": pr.get("base", {}).get("sha"),
        "pr_number": pr.get("number"),
        "tested_tree": git(root, "rev-parse", "HEAD^{tree}"),
        "check": "test",
    }
    (root / "check-candidate.json").write_text(json.dumps(record, sort_keys=True) + "\n")


def api(path, binary=False):
    if not os.environ.get("GH_TOKEN"):
        raise ValueError("GH_TOKEN is required for trusted artifact lookup")
    url = path if path.startswith("https://") else "https://api.github.com/" + path
    request = urllib.request.Request(url, headers={
        "Accept": "application/vnd.github+json",
        "Authorization": "Bearer " + os.environ["GH_TOKEN"],
        "X-GitHub-Api-Version": "2022-11-28",
    })
    with urllib.request.urlopen(request, timeout=60) as response:
        data = response.read(MAX_ARCHIVE_BYTES + 1)
    if len(data) > MAX_ARCHIVE_BYTES:
        raise ValueError("GitHub response exceeded the bounded artifact size")
    return data if binary else json.loads(data)


def require_checks(commit, api_fn=api):
    result = api_fn(f"repos/{REPOSITORY}/commits/{commit}/check-runs?per_page=100")
    for name in REQUIRED_CHECKS:
        checks = [check for check in result.get("check_runs", [])
                  if check.get("name") == name and check.get("app", {}).get("slug") == "github-actions"]
        if not checks:
            raise ValueError(f"required accepted PR check is missing: {name}")
        latest = max(checks, key=lambda check: check.get("started_at", ""))
        if latest.get("status") != "completed" or latest.get("conclusion") != "success":
            raise ValueError(f"required accepted PR check is not successful: {name}")


def artifact_bytes(metadata):
    if metadata.get("expired") or not str(metadata.get("id", "")).isdigit():
        raise ValueError("accepted artifact is missing, expired, or has an invalid id")
    digest = metadata.get("digest", "")
    if not re.fullmatch(r"sha256:[0-9a-f]{64}", digest):
        raise ValueError("GitHub artifact has no immutable SHA-256 digest")
    data = api(f"repos/{REPOSITORY}/actions/artifacts/{metadata['id']}/zip", binary=True)
    if "sha256:" + sha(data) != digest:
        raise ValueError("GitHub artifact digest mismatch")
    return data


def zip_members(data, expected):
    with zipfile.ZipFile(io.BytesIO(data)) as bundle:
        infos = bundle.infolist()
        names = [item.filename for item in infos if not item.is_dir()]
        if len(names) != len(set(names)) or set(names) != set(expected):
            raise ValueError("native artifact has a missing, duplicate, or unexpected file")
        for item in infos:
            path = PurePosixPath(item.filename)
            if path.is_absolute() or ".." in path.parts or "\\" in item.filename:
                raise ValueError("unsafe native artifact member path")
            if item.file_size > MAX_ARCHIVE_BYTES:
                raise ValueError("native artifact member exceeds size limit")
        return {name: bundle.read(name) for name in names}


def verify_candidate(data, metadata, run, pr, merge_sha, merge_tree, root, api_fn=api):
    artifact_name = f"{NATIVE_ARTIFACT_PREFIX}-{run['id']}-{run['run_attempt']}"
    if (run.get("repository", {}).get("full_name") != REPOSITORY
            or run.get("head_repository", {}).get("full_name") != REPOSITORY
            or run.get("path") != NATIVE_WORKFLOW
            or run.get("conclusion") != "success"
            or run.get("event") != "pull_request"
            or len(run.get("pull_requests", [])) != 1
            or run["pull_requests"][0].get("number") != pr["number"]
            or run["head_sha"] != pr["head"]["sha"]):
        raise ValueError("native producer is not the successful same-repository PR run")
    if (not pr.get("merged") or pr.get("base", {}).get("ref") != "main"
            or pr.get("base", {}).get("repo", {}).get("full_name") != REPOSITORY
            or pr.get("head", {}).get("repo", {}).get("full_name") != REPOSITORY
            or pr.get("merge_commit_sha") != merge_sha):
        raise ValueError("candidate PR is not the exact same-repository main merge")
    if metadata.get("name") != artifact_name:
        raise ValueError("native artifact name does not identify its producing run attempt")
    members = zip_members(data, {"Muninn.app.tar.gz", "native-receipt.json"})
    receipt = json.loads(members["native-receipt.json"])
    expected = {
        "schema": 1, "repository": REPOSITORY, "workflow_path": NATIVE_WORKFLOW,
        "run_id": run["id"], "run_attempt": run["run_attempt"],
        "event": "pull_request", "head_sha": run["head_sha"],
        "base_sha": run["pull_requests"][0].get("base", {}).get("sha"),
        "pr_number": pr["number"], "tested_tree": merge_tree,
        "version": version(root), "platform": "darwin-arm64",
        "files": {"Muninn.app.tar.gz": sha(members["Muninn.app.tar.gz"])},
    }
    if any(receipt.get(key) != value for key, value in expected.items()):
        raise ValueError("native receipt does not match the accepted merge tree or run")
    # A candidate can change its workflow file. Accept only when the exact workflow
    # blob used for that run matches the trusted workflow at the merged tree.
    workflow_sha = receipt.get("workflow_sha", "")
    if not re.fullmatch(r"[0-9a-f]{40}", workflow_sha):
        raise ValueError("native receipt has no valid workflow revision")
    for policy_path in (NATIVE_WORKFLOW, HELPER_PATH):
        path = urllib.parse.quote(policy_path, safe="/")
        approved = api_fn(f"repos/{REPOSITORY}/contents/{path}?ref={merge_sha}")["sha"]
        producing = api_fn(f"repos/{REPOSITORY}/contents/{path}?ref={workflow_sha}")["sha"]
        if approved != producing:
            raise ValueError(f"native policy file differs from the accepted main policy: {policy_path}")
    with tempfile.TemporaryDirectory() as temp:
        archive_path = Path(temp) / "Muninn.app.tar.gz"
        archive_path.write_bytes(members["Muninn.app.tar.gz"])
        with tarfile.open(archive_path, "r:gz") as tar:
            entries = tar.getmembers()
            for entry in entries:
                path = PurePosixPath(entry.name)
                if path.is_absolute() or ".." in path.parts or not path.parts or path.parts[0] != "Muninn.app":
                    raise ValueError("unsafe application archive path")
                if entry.issym() or entry.islnk():
                    resolved = (path.parent / entry.linkname)
                    stack = []
                    for part in resolved.parts:
                        if part in ("", "."):
                            continue
                        if part == "..":
                            if not stack:
                                raise ValueError("application archive link escapes bundle")
                            stack.pop()
                        else:
                            stack.append(part)
                    if not stack or stack[0] != "Muninn.app":
                        raise ValueError("application archive link escapes bundle")
                elif not (entry.isfile() or entry.isdir()):
                    raise ValueError("unsupported application archive entry type")
            tar.extractall(temp, filter="data")
        app = Path(temp) / "Muninn.app"
        if inventory(app) != receipt.get("app_files"):
            raise ValueError("application file inventory does not match the native receipt")
    return {
        "schema": 1, "repository": REPOSITORY, "accepted_commit": merge_sha,
        "accepted_tree": merge_tree, "pr_number": pr["number"],
        "pr_head_sha": pr["head"]["sha"], "source_run_id": run["id"],
        "source_run_attempt": run["run_attempt"], "source_artifact_id": metadata["id"],
        "source_artifact_digest": metadata["digest"], "source_artifact_name": artifact_name,
        "workflow_path": NATIVE_WORKFLOW, "workflow_sha": workflow_sha,
        "source_base_sha": receipt["base_sha"],
        "version": version(root), "platform": "darwin-arm64",
        "files": expected["files"], "app_files": receipt["app_files"],
    }


def check_candidate(root, pr, accepted, merge_sha, merge_tree):
    runs = []
    for page in range(1, 11):
        query = urllib.parse.urlencode({"head_sha": pr["head"]["sha"], "event": "pull_request",
                                        "status": "success", "per_page": 100, "page": page})
        batch = api(f"repos/{REPOSITORY}/actions/workflows/check.yml/runs?{query}")["workflow_runs"]
        runs.extend(x for x in batch if x.get("path") == CHECK_WORKFLOW
                    and x.get("head_sha") == pr["head"]["sha"]
                    and x.get("conclusion") == "success"
                    and len(x.get("pull_requests", [])) == 1
                    and x["pull_requests"][0].get("number") == pr["number"]
                    and x["pull_requests"][0].get("base", {}).get("sha") == accepted["source_base_sha"])
        if len(batch) < 100:
            break
    if not runs:
        raise ValueError("no successful Dagger check run for the exact PR head and base")
    run = max(runs, key=lambda x: (x["id"], x["run_attempt"]))
    artifacts = api(f"repos/{REPOSITORY}/actions/runs/{run['id']}/artifacts?per_page=100")["artifacts"]
    name = f"{CHECK_ARTIFACT_PREFIX}-{run['id']}-{run['run_attempt']}"
    found = [a for a in artifacts if a.get("name") == name and not a.get("expired")]
    if len(found) != 1:
        raise ValueError("exact Dagger check run has no unique unexpired receipt artifact")
    artifact = found[0]
    members = zip_members(artifact_bytes(artifact), {"check-candidate.json"})
    record = json.loads(members["check-candidate.json"])
    expected = {
        "schema": 1, "repository": REPOSITORY, "workflow_path": CHECK_WORKFLOW,
        "run_id": run["id"], "run_attempt": run["run_attempt"], "event": "pull_request",
        "head_sha": pr["head"]["sha"], "base_sha": accepted["source_base_sha"],
        "pr_number": pr["number"], "tested_tree": merge_tree, "check": "test",
    }
    if any(record.get(key) != value for key, value in expected.items()):
        raise ValueError("Dagger receipt does not identify the accepted merge tree")
    workflow_sha = record.get("workflow_sha", "")
    if not re.fullmatch(r"[0-9a-f]{40}", workflow_sha):
        raise ValueError("Dagger receipt has no valid workflow revision")
    for policy_path in (CHECK_WORKFLOW, HELPER_PATH):
        path = urllib.parse.quote(policy_path, safe="/")
        approved = api(f"repos/{REPOSITORY}/contents/{path}?ref={merge_sha}")["sha"]
        producing = api(f"repos/{REPOSITORY}/contents/{path}?ref={workflow_sha}")["sha"]
        if approved != producing:
            raise ValueError(f"Dagger policy file differs from the accepted main policy: {policy_path}")
    return {"run_id": run["id"], "run_attempt": run["run_attempt"],
            "artifact_id": artifact["id"], "artifact_digest": artifact["digest"],
            "workflow_sha": workflow_sha, "workflow_path": CHECK_WORKFLOW,
            "tested_tree": merge_tree, "base_sha": accepted["source_base_sha"]}


def verify_check_receipt(accepted, merge_sha, api_fn=api, artifact_bytes_fn=artifact_bytes):
    reference = accepted.get("check_receipt")
    if not isinstance(reference, dict):
        raise ValueError("accepted mapping has no Dagger check receipt reference")
    run_id = reference.get("run_id")
    attempt = reference.get("run_attempt")
    if not isinstance(run_id, int) or not isinstance(attempt, int):
        raise ValueError("Dagger check receipt run identity is invalid")
    run = api_fn(f"repos/{REPOSITORY}/actions/runs/{run_id}")
    expected_run = {
        "id": run_id, "run_attempt": attempt, "path": CHECK_WORKFLOW,
        "event": "pull_request", "conclusion": "success",
        "head_sha": accepted["pr_head_sha"],
    }
    if any(run.get(key) != value for key, value in expected_run.items()):
        raise ValueError("Dagger receipt is not from the successful accepted PR check run")
    if (run.get("repository", {}).get("full_name") != REPOSITORY
            or run.get("head_repository", {}).get("full_name") != REPOSITORY
            or len(run.get("pull_requests", [])) != 1
            or run["pull_requests"][0].get("number") != accepted["pr_number"]
            or run["pull_requests"][0].get("base", {}).get("sha") != accepted["source_base_sha"]):
        raise ValueError("Dagger receipt run is not bound to the accepted same-repository PR and base")
    artifacts = api_fn(f"repos/{REPOSITORY}/actions/runs/{run_id}/artifacts?per_page=100")["artifacts"]
    name = f"{CHECK_ARTIFACT_PREFIX}-{run_id}-{attempt}"
    found = [a for a in artifacts if a.get("id") == reference.get("artifact_id")
             and a.get("name") == name and a.get("digest") == reference.get("artifact_digest")
             and not a.get("expired")]
    if len(found) != 1:
        raise ValueError("accepted Dagger check artifact is missing, expired, or has changed identity")
    members = zip_members(artifact_bytes_fn(found[0]), {"check-candidate.json"})
    record = json.loads(members["check-candidate.json"])
    expected_record = {
        "schema": 1, "repository": REPOSITORY, "workflow_path": CHECK_WORKFLOW,
        "run_id": run_id, "run_attempt": attempt, "event": "pull_request",
        "head_sha": accepted["pr_head_sha"], "base_sha": accepted["source_base_sha"],
        "pr_number": accepted["pr_number"], "tested_tree": accepted["accepted_tree"],
        "check": "test", "workflow_sha": reference.get("workflow_sha"),
    }
    if any(record.get(key) != value for key, value in expected_record.items()):
        raise ValueError("Dagger receipt contents do not match the accepted PR tree and run")
    workflow_sha = record.get("workflow_sha", "")
    if (reference.get("workflow_path") != CHECK_WORKFLOW
            or reference.get("tested_tree") != accepted["accepted_tree"]
            or reference.get("base_sha") != accepted["source_base_sha"]):
        raise ValueError("Dagger receipt reference does not match the accepted tree, base, or workflow")
    if not re.fullmatch(r"[0-9a-f]{40}", workflow_sha):
        raise ValueError("Dagger receipt has no valid workflow revision")
    for policy_path in (CHECK_WORKFLOW, HELPER_PATH):
        path = urllib.parse.quote(policy_path, safe="/")
        approved = api_fn(f"repos/{REPOSITORY}/contents/{path}?ref={merge_sha}")["sha"]
        producing = api_fn(f"repos/{REPOSITORY}/contents/{path}?ref={workflow_sha}")["sha"]
        if approved != producing:
            raise ValueError(f"Dagger policy file differs from the accepted main policy: {policy_path}")
    return record


def promote(root, merge_sha):
    if git(root, "rev-parse", "HEAD^{commit}") != merge_sha:
        raise ValueError("promotion checkout does not match the actual main merge commit")
    merge_tree = git(root, "rev-parse", "HEAD^{tree}")
    prs = api(f"repos/{REPOSITORY}/commits/{merge_sha}/pulls")
    full_prs = [api(f"repos/{REPOSITORY}/pulls/{pr['number']}") for pr in prs
                if pr.get("number") and pr.get("base", {}).get("ref") == "main"]
    matches = [pr for pr in full_prs if pr.get("merged") and pr.get("merge_commit_sha") == merge_sha
               and pr.get("base", {}).get("ref") == "main"]
    if len(matches) != 1:
        raise ValueError("main commit must correspond to exactly one merged PR")
    pr = matches[0]
    if (pr.get("head", {}).get("repo", {}).get("full_name") != REPOSITORY
            or pr.get("base", {}).get("repo", {}).get("full_name") != REPOSITORY):
        raise ValueError("fork-produced artifacts cannot be promoted to a release")
    require_checks(pr["head"]["sha"])
    runs = []
    for page in range(1, 11):
        query = urllib.parse.urlencode({"head_sha": pr["head"]["sha"], "event": "pull_request",
                                        "status": "success", "per_page": 100, "page": page})
        batch = api(f"repos/{REPOSITORY}/actions/workflows/macos.yml/runs?{query}")["workflow_runs"]
        runs.extend(x for x in batch if x.get("path") == NATIVE_WORKFLOW
                    and x.get("head_sha") == pr["head"]["sha"]
                    and x.get("conclusion") == "success"
                    and x.get("pull_requests", [{}])[0].get("number") == pr["number"])
        if len(batch) < 100:
            break
    if not runs:
        raise ValueError("no successful native PR run for the exact merged head")
    run = max(runs, key=lambda x: (x["id"], x["run_attempt"]))
    artifacts = api(f"repos/{REPOSITORY}/actions/runs/{run['id']}/artifacts?per_page=100")["artifacts"]
    name = f"{NATIVE_ARTIFACT_PREFIX}-{run['id']}-{run['run_attempt']}"
    found = [a for a in artifacts if a.get("name") == name and not a.get("expired")]
    if len(found) != 1:
        raise ValueError("exact native run has no unique unexpired release candidate artifact")
    artifact = found[0]
    data = artifact_bytes(artifact)
    record = verify_candidate(data, artifact, run, pr, merge_sha, merge_tree, root)
    record["check_receipt"] = check_candidate(root, pr, record, merge_sha, merge_tree)
    (root / "accepted-native.json").write_text(json.dumps(record, sort_keys=True) + "\n")
    return record


def mapping(root, mapping_run_id, expected_commit):
    if not str(mapping_run_id).isdigit():
        raise ValueError("mapping run id must be numeric")
    run = api(f"repos/{REPOSITORY}/actions/runs/{mapping_run_id}")
    if (run.get("repository", {}).get("full_name") != REPOSITORY
            or run.get("path") != MAPPING_WORKFLOW or run.get("event") != "push"
            or run.get("head_branch") != "main" or run.get("head_sha") != expected_commit
            or run.get("conclusion") != "success"):
        raise ValueError("mapping run is not the successful trusted main promotion for this commit")
    artifacts = api(f"repos/{REPOSITORY}/actions/runs/{mapping_run_id}/artifacts?per_page=100")["artifacts"]
    name = f"{MAPPING_ARTIFACT_PREFIX}-{expected_commit}"
    found = [a for a in artifacts if a.get("name") == name and not a.get("expired")]
    if len(found) != 1:
        raise ValueError("accepted mapping artifact is missing, expired, or ambiguous")
    bundle = zip_members(artifact_bytes(found[0]), {"accepted-native.json"})
    accepted = json.loads(bundle["accepted-native.json"])
    tree = git(root, "rev-parse", "HEAD^{tree}")
    if (accepted.get("repository") != REPOSITORY or accepted.get("accepted_commit") != expected_commit
            or accepted.get("accepted_tree") != tree or accepted.get("version") != version(root)):
        raise ValueError("accepted mapping does not match this release tree and version")
    return accepted


def retrieve(root, mapping_run_id, expected_commit, target):
    accepted = mapping(root, mapping_run_id, expected_commit)
    verify_check_receipt(accepted, expected_commit)
    run = api(f"repos/{REPOSITORY}/actions/runs/{accepted['source_run_id']}")
    artifacts = api(f"repos/{REPOSITORY}/actions/runs/{accepted['source_run_id']}/artifacts?per_page=100")["artifacts"]
    found = [a for a in artifacts if a.get("id") == accepted["source_artifact_id"]]
    if len(found) != 1 or found[0].get("digest") != accepted["source_artifact_digest"]:
        raise ValueError("source artifact identity changed after acceptance")
    pr = api(f"repos/{REPOSITORY}/pulls/{accepted['pr_number']}")
    data = artifact_bytes(found[0])
    verified = verify_candidate(data, found[0], run, pr, expected_commit,
                                accepted["accepted_tree"], root)
    native_mapping = {key: value for key, value in accepted.items() if key != "check_receipt"}
    if verified != native_mapping:
        raise ValueError("source artifact no longer matches its accepted mapping")
    members = zip_members(data, {"Muninn.app.tar.gz", "native-receipt.json"})
    target = Path(target)
    target.mkdir(parents=True, exist_ok=True)
    with tarfile.open(fileobj=io.BytesIO(members["Muninn.app.tar.gz"]), mode="r:gz") as tar:
        tar.extractall(target, filter="data")
    (target / "accepted-native.json").write_text(json.dumps(accepted, sort_keys=True) + "\n")
    return accepted


def main():
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="command", required=True)
    create_parser = sub.add_parser("create")
    create_parser.add_argument("--root", type=Path, default=Path.cwd())
    check_parser = sub.add_parser("create-check")
    check_parser.add_argument("--root", type=Path, default=Path.cwd())
    promote_parser = sub.add_parser("promote")
    promote_parser.add_argument("--root", type=Path, default=Path.cwd())
    promote_parser.add_argument("--merge-sha", required=True)
    retrieve_parser = sub.add_parser("retrieve")
    retrieve_parser.add_argument("--root", type=Path, default=Path.cwd())
    retrieve_parser.add_argument("--mapping-run-id", required=True)
    retrieve_parser.add_argument("--expected-commit", required=True)
    retrieve_parser.add_argument("--target", type=Path, required=True)
    args = parser.parse_args()
    if args.command == "create":
        create(args.root)
    elif args.command == "create-check":
        create_check(args.root)
    elif args.command == "promote":
        promote(args.root, args.merge_sha)
    else:
        retrieve(args.root, args.mapping_run_id, args.expected_commit, args.target)


if __name__ == "__main__":
    try:
        main()
    except Exception as exc:
        raise SystemExit(f"release candidate blocked: {exc}") from exc
