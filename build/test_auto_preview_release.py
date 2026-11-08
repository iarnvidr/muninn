"""Regression tests for trusted post-merge preview publication."""

import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

import auto_preview_release as preview
import updater_release


SHA = "a" * 40
OTHER = "b" * 40


def event(**overrides):
    run = {
        "id": 42430719763,
        "path": ".github/workflows/accept-native.yml",
        "event": "push",
        "head_branch": "main",
        "status": "completed",
        "conclusion": "success",
        "head_sha": SHA,
        "repository": {"full_name": "iarnvidr/muninn"},
        "head_repository": {"full_name": "iarnvidr/muninn"},
    }
    run.update(overrides)
    return {"workflow_run": run}


def released(version):
    return {
        "tag_name": "v" + version, "draft": False, "prerelease": True,
        "assets": [{"name": n} for n in
                   (f"Muninn_{version}_macos-arm64.dmg", "Muninn.app.tar.gz", "latest.json")],
    }


class PreviewReleasePlanTests(unittest.TestCase):
    def test_newer_accepted_merge_creates_versioned_preview(self):
        result = preview.decision(event(), SHA, SHA, "0.2.23", [released("0.2.22")])
        self.assertTrue(result["release_needed"])
        self.assertEqual(result["tag"], "v0.2.23")
        self.assertEqual(result["source_sha"], SHA)
        self.assertEqual(result["mapping_run_id"], 42430719763)

    def test_unchanged_published_version_is_noop(self):
        result = preview.decision(event(), SHA, SHA, "0.2.23", [released("0.2.23")])
        self.assertFalse(result["release_needed"])

    def test_outdated_version_cannot_replace_a_newer_published_one(self):
        with self.assertRaisesRegex(ValueError, "regressed"):
            preview.decision(event(), SHA, SHA, "0.2.23", [released("0.3.0")])

    def test_preexisting_incomplete_published_release_blocks(self):
        bad = released("0.2.23")
        bad["assets"].pop()
        with self.assertRaisesRegex(ValueError, "missing its exact updater assets"):
            preview.decision(event(), SHA, SHA, "0.2.23", [bad])

    def test_numeric_semver_not_lexicographical(self):
        result = preview.decision(event(), SHA, SHA, "0.2.100", [released("0.2.99")])
        self.assertTrue(result["release_needed"])

    def test_event_must_be_successful_trusted_main_mapping(self):
        invalid = [
            {"conclusion": "failure"}, {"status": "in_progress"},
            {"head_branch": "feature"}, {"event": "pull_request"},
            {"path": ".github/workflows/macos.yml"},
            {"repository": {"full_name": "outsider/muninn"}},
            {"head_repository": {"full_name": "outsider/muninn"}},
            {"head_sha": OTHER}, {"id": "42430719763"},
        ]
        for change in invalid:
            with self.subTest(change=change), self.assertRaises(ValueError):
                preview.decision(event(**change), SHA, SHA, "0.2.23", [released("0.2.22")])

    def test_branch_movement_or_unexpected_version_blocks(self):
        for current in (OTHER, "", "not-a-sha"):
            with self.assertRaisesRegex(ValueError, "no longer the current main"):
                preview.decision(event(), SHA, current, "0.2.23", [])
        for version in ("0.2.23-dev", "0.2", "v0.2.23", "0.2.023"):
            with self.assertRaisesRegex(ValueError, "not a numeric SemVer"):
                preview.decision(event(), SHA, SHA, version, [])

    def test_drafts_and_nonsemver_tags_do_not_block(self):
        drafts = [dict(released("0.99.0"), draft=True), {"tag_name": "v0.2.23-rc.2", "draft": False}]
        def list_fn(path):
            self.assertEqual(path, "repos/iarnvidr/muninn/releases?per_page=100&page=1")
            return drafts
        self.assertEqual(preview.release_versions(list_fn), [])
        self.assertTrue(preview.decision(event(), SHA, SHA, "0.2.23", [])["release_needed"])


class PreviewReleaseArtifactTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.base = Path(self.temp.name)
        self.version = "0.2.23"
        self.tag = "v0.2.23"
        self.mapping_run_id = "42430719763"
        self.files = {
            "Muninn_0.2.23_macos-arm64.dmg": b"DMG",
            "Muninn.app.tar.gz": b"GZIP",
            "Muninn.app.tar.gz.sig": b"signed\n",
            "accepted-native.json": json.dumps({"accepted_commit": SHA, "version": self.version,
                                                 "check_receipt": {"pipeline": "dagger"}}).encode(),
            "release-review.txt": f"version={self.version} tag={self.tag} commit={SHA} mapping_run={self.mapping_run_id}\n".encode(),
            "latest.json": json.dumps(updater_release.make_feed(self.version,
                                                                  f"https://github.com/iarnvidr/muninn/releases/download/{self.tag}/Muninn.app.tar.gz",
                                                                  "signed")).encode(),
        }
        for name, content in self.files.items():
            (self.base / name).write_bytes(content)

    def verify(self):
        return preview.validate_assets(self.base, self.version, self.tag, SHA, self.mapping_run_id)

    def test_valid_artifacts_have_three_verified_upload_digests(self):
        result = self.verify()
        self.assertEqual(set(result), {"Muninn_0.2.23_macos-arm64.dmg", "latest.json", "Muninn.app.tar.gz"})
        self.assertTrue(all(digest.startswith("sha256:") for digest in result.values()))

    def test_tampered_manifest_signature_is_rejected(self):
        feed = json.loads((self.base / "latest.json").read_text())
        feed["platforms"]["darwin-aarch64"]["signature"] = "different"
        (self.base / "latest.json").write_text(json.dumps(feed))
        with self.assertRaisesRegex(ValueError, "verified detached signature"):
            self.verify()

    def test_tampered_release_version_or_url_is_rejected(self):
        feed = json.loads((self.base / "latest.json").read_text())
        feed["platforms"]["darwin-aarch64"]["url"] = "https://example.com/Muninn.app.tar.gz"
        (self.base / "latest.json").write_text(json.dumps(feed))
        with self.assertRaisesRegex(ValueError, "immutable version"):
            self.verify()

    def test_invalid_mapping_and_missing_assets_are_rejected(self):
        (self.base / "accepted-native.json").write_text('{"accepted_commit":"other"}')
        with self.assertRaisesRegex(ValueError, "incorrect provenance"):
            self.verify()
        (self.base / "accepted-native.json").write_bytes(self.files["accepted-native.json"])
        for name in ("Muninn.app.tar.gz", "latest.json", "Muninn_0.2.23_macos-arm64.dmg"):
            with self.subTest(name=name):
                path = self.base / name
                path.unlink()
                with self.assertRaisesRegex(ValueError, "missing or empty"):
                    self.verify()
                path.write_bytes(self.files[name])


class PreviewWorkflowContractTests(unittest.TestCase):
    def test_automatic_preview_remains_separate_from_protected_stable_release(self):
        root = Path(__file__).resolve().parents[1]
        auto = (root / ".github/workflows/auto-preview-release.yml").read_text()
        protected = (root / ".github/workflows/release.yml").read_text()
        publisher = (root / "build/publish_auto_preview.sh").read_text()
        for string in ("workflow_run:", "Map accepted Muninn native artifact", "types: [completed]",
                       "release_candidate.py retrieve", "--expected-commit \"$SOURCE_SHA\"",
                       "auto_preview_release.py verify-assets", "contents: write", "contents: read",
                       "TAURI_SIGNING_PRIVATE_KEY", "minisign -Vm", "package-dmg.sh",
                       "release_needed == 'true'"):
            self.assertIn(string, auto)
        self.assertNotIn("pull_request_target:", auto)
        self.assertNotIn("cargo build", auto)
        self.assertNotIn("--clobber", publisher)
        self.assertIn("--draft --prerelease", publisher)
        self.assertIn('gh release edit "$TAG" --draft=false', publisher)
        self.assertIn("name: muninn-release", protected)
        self.assertIn("workflow_dispatch:", protected)
        self.assertNotIn("workflow_run:", protected)


@unittest.skipUnless(shutil.which("jq"), "publisher requires jq on Ubuntu runners")
class PreviewPublisherTests(PreviewReleaseArtifactTests):
    """Exercise real publisher control flow using a deterministic fake gh CLI."""

    def setUp(self):
        super().setUp()
        root = Path(__file__).resolve().parents[1]
        (self.base / "build").symlink_to(root / "build", target_is_directory=True)
        assets = self.base / "release-assets"
        assets.mkdir()
        for name, content in self.files.items():
            (assets / name).write_bytes(content)
        self.state_file = self.base / "gh-state.json"
        self.state_file.write_text(json.dumps({"main": SHA, "tag": None, "release": None, "calls": []}))
        binary = self.base / "bin"
        binary.mkdir()
        cli = binary / "gh"
        cli.write_text('''#!/usr/bin/env python3
import hashlib, json, os, pathlib, sys
state_file=pathlib.Path(os.environ["GH_STATE"])
s=json.loads(state_file.read_text())
a=sys.argv[1:]
s["calls"].append(a)
def missing():
    print("GraphQL: Not Found (HTTP 404)",file=sys.stderr)
    state_file.write_text(json.dumps(s))
    sys.exit(1)
if a[:2] == ["api", "repos/iarnvidr/muninn/git/ref/heads/main"]:
    print(s["main"])
elif a[:2] == ["api", "repos/iarnvidr/muninn/git/ref/tags/v0.2.23"]:
    if s["tag"] is None: missing()
    print(json.dumps({"object":{"type":"commit","sha":s["tag"]}}))
elif a[:3] == ["api", "--method", "POST"]:
    s["tag"] = next(x.removeprefix("sha=") for x in a if x.startswith("sha="))
    print("{}")
elif a[:2] == ["api", "repos/iarnvidr/muninn/releases/tags/v0.2.23"]:
    if s["release"] is None: missing()
    print(json.dumps(s["release"]))
elif a[:2] == ["release", "create"]:
    s["release"]={"draft":True,"prerelease":True,"body":a[a.index("--notes")+1],"assets":[]}
elif a[:2] == ["release", "upload"]:
    file=pathlib.Path(a[3]); data=file.read_bytes()
    s["release"]["assets"].append({"name":file.name,"digest":"sha256:"+hashlib.sha256(data).hexdigest()})
elif a[:2] == ["release", "edit"]:
    s["release"]["draft"]=False
else:
    raise RuntimeError("unexpected GitHub CLI arguments: "+repr(a))
state_file.write_text(json.dumps(s))
''')
        cli.chmod(0o755)
        self.publisher = root / "build/publish_auto_preview.sh"
        self.env = dict(os.environ, PATH=str(binary) + os.pathsep + os.environ["PATH"],
                        GH_TOKEN="mock", GH_REPO="iarnvidr/muninn", GH_STATE=str(self.state_file),
                        VERSION=self.version, TAG=self.tag, SOURCE_SHA=SHA,
                        MAPPING_RUN_ID=self.mapping_run_id, RUNNER_TEMP=str(self.base))

    def run_publisher(self):
        return subprocess.run(["bash", str(self.publisher)], cwd=self.base, env=self.env,
                              text=True, capture_output=True)

    def state(self):
        return json.loads(self.state_file.read_text())

    def test_publishes_draft_then_three_verified_assets_and_is_idempotent(self):
        result = self.run_publisher()
        self.assertEqual(result.returncode, 0, result.stderr)
        state = self.state()
        self.assertEqual(state["tag"], SHA)
        self.assertFalse(state["release"]["draft"])
        self.assertEqual(len(state["release"]["assets"]), 3)
        upload_calls = [a for a in state["calls"] if a[:2] == ["release", "upload"]]
        self.assertEqual(len(upload_calls), 3)
        result = self.run_publisher()
        self.assertEqual(result.returncode, 0, result.stderr)
        again = self.state()
        self.assertEqual(len([a for a in again["calls"] if a[:2] == ["release", "upload"]]), 3)

    def test_refuses_to_modify_existing_published_digest_mismatch(self):
        self.assertEqual(self.run_publisher().returncode, 0)
        state = self.state()
        state["release"]["assets"][0]["digest"] = "sha256:" + "f" * 64
        self.state_file.write_text(json.dumps(state))
        rejected = self.run_publisher()
        self.assertNotEqual(rejected.returncode, 0)
        self.assertIn("digest mismatch", rejected.stderr)

    def test_refuses_stale_branch_and_tag_conflict(self):
        state = self.state()
        state["main"] = OTHER
        self.state_file.write_text(json.dumps(state))
        rejected = self.run_publisher()
        self.assertNotEqual(rejected.returncode, 0)
        self.assertIn("no longer main HEAD", rejected.stderr)
        state["main"] = SHA
        state["tag"] = OTHER
        self.state_file.write_text(json.dumps(state))
        rejected = self.run_publisher()
        self.assertNotEqual(rejected.returncode, 0)
        self.assertIn("different or non-commit", rejected.stderr)


if __name__ == "__main__":
    unittest.main()
