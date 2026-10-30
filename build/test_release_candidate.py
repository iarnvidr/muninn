import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest import mock
import zipfile

import release_candidate as candidate


class NativeCandidateTests(unittest.TestCase):
    def fixture(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        root = Path(temp.name)
        config = root / "src-tauri"
        config.mkdir()
        (config / "tauri.conf.json").write_text(json.dumps({"version": "0.2.14"}))
        app = root / "Muninn.app"
        binary = app / "Contents" / "MacOS" / "Muninn"
        binary.parent.mkdir(parents=True)
        binary.write_bytes(b"candidate app")
        binary.chmod(0o755)
        tar_stream = io.BytesIO()
        with tarfile.open(fileobj=tar_stream, mode="w:gz", format=tarfile.PAX_FORMAT) as archive:
            archive.add(app, arcname="Muninn.app")
        tar_bytes = tar_stream.getvalue()
        app_files = candidate.inventory(app)
        receipt = {
            "schema": 1,
            "repository": candidate.REPOSITORY,
            "workflow_path": candidate.NATIVE_WORKFLOW,
            "run_id": 123,
            "run_attempt": 1,
            "event": "pull_request",
            "workflow_sha": "a" * 40,
            "head_sha": "b" * 40,
            "base_sha": "f" * 40,
            "pr_number": 44,
            "tested_tree": "c" * 40,
            "version": "0.2.14",
            "platform": "darwin-arm64",
            "files": {"Muninn.app.tar.gz": candidate.sha(tar_bytes)},
            "app_files": app_files,
        }
        bundle = io.BytesIO()
        with zipfile.ZipFile(bundle, "w", zipfile.ZIP_DEFLATED) as archive:
            archive.writestr("Muninn.app.tar.gz", tar_bytes)
            archive.writestr("native-receipt.json", json.dumps(receipt))
        run = {
            "id": 123,
            "run_attempt": 1,
            "repository": {"full_name": candidate.REPOSITORY},
            "head_repository": {"full_name": candidate.REPOSITORY},
            "path": candidate.NATIVE_WORKFLOW,
            "conclusion": "success",
            "event": "pull_request",
            "head_sha": "b" * 40,
            "pull_requests": [{"number": 44, "base": {"sha": "f" * 40}}],
        }
        pr = {
            "number": 44,
            "merged": True,
            "merge_commit_sha": "c" * 40,
            "head": {"sha": "b" * 40, "repo": {"full_name": candidate.REPOSITORY}},
            "base": {"ref": "main", "repo": {"full_name": candidate.REPOSITORY}},
        }
        artifact = {
            "id": 555,
            "name": "muninn-native-candidate-123-1",
            "digest": "sha256:" + "d" * 64,
            "expired": False,
        }
        api = lambda _path: {"sha": "e" * 40}
        return root, bundle.getvalue(), artifact, run, pr, api

    def test_candidate_binds_exact_run_merge_tree_and_app_inventory(self):
        root, data, artifact, run, pr, api = self.fixture()
        record = candidate.verify_candidate(data, artifact, run, pr, "c" * 40, "c" * 40, root, api)
        self.assertEqual(record["accepted_commit"], "c" * 40)
        self.assertEqual(record["source_artifact_id"], 555)
        self.assertIn("Muninn.app/Contents/MacOS/Muninn", record["app_files"])

    def test_candidate_rejects_tree_mismatch_and_fork_producer(self):
        root, data, artifact, run, pr, api = self.fixture()
        with self.assertRaisesRegex(ValueError, "accepted merge tree"):
            candidate.verify_candidate(data, artifact, run, pr, "c" * 40, "f" * 40, root, api)
        pr["head"]["repo"]["full_name"] = "attacker/muninn"
        with self.assertRaisesRegex(ValueError, "same-repository main merge"):
            candidate.verify_candidate(data, artifact, run, pr, "c" * 40, "c" * 40, root, api)

    def test_candidate_rejects_modified_producer_helper(self):
        root, data, artifact, run, pr, _ = self.fixture()
        def changed_helper(path):
            if "build/release_candidate.py" in path and "ref=" + "a" * 40 in path:
                return {"sha": "f" * 40}
            return {"sha": "e" * 40}
        with self.assertRaisesRegex(ValueError, "build/release_candidate.py"):
            candidate.verify_candidate(data, artifact, run, pr, "c" * 40, "c" * 40,
                                       root, changed_helper)

    def test_candidate_rejects_extra_files_and_bad_digest_receipt(self):
        root, data, artifact, run, pr, api = self.fixture()
        with zipfile.ZipFile(io.BytesIO(data)) as source:
            members = {name: source.read(name) for name in source.namelist()}
        members["unexpected.bin"] = b"extra"
        extra = io.BytesIO()
        with zipfile.ZipFile(extra, "w") as archive:
            for name, body in members.items():
                archive.writestr(name, body)
        with self.assertRaisesRegex(ValueError, "unexpected file"):
            candidate.verify_candidate(extra.getvalue(), artifact, run, pr, "c" * 40, "c" * 40, root, api)

    def test_artifact_zip_rejects_traversal_and_duplicate_names(self):
        traversal = io.BytesIO()
        with zipfile.ZipFile(traversal, "w") as archive:
            archive.writestr("../escape", b"bad")
        with self.assertRaisesRegex(ValueError, "unexpected file"):
            candidate.zip_members(traversal.getvalue(), {"safe"})
        duplicate = io.BytesIO()
        with zipfile.ZipFile(duplicate, "w") as archive:
            archive.writestr("safe", b"one")
            archive.writestr("safe", b"two")
        with self.assertRaisesRegex(ValueError, "duplicate"):
            candidate.zip_members(duplicate.getvalue(), {"safe"})

    def test_main_mapping_requires_latest_dagger_and_native_checks(self):
        good = {"check_runs": [
            {"name": "test", "status": "completed", "conclusion": "failure",
             "started_at": "2026-01-01T00:00:00Z", "app": {"slug": "github-actions"}},
            {"name": "test", "status": "completed", "conclusion": "success",
             "started_at": "2026-01-02T00:00:00Z", "app": {"slug": "github-actions"}},
            {"name": "package", "status": "completed", "conclusion": "success",
             "started_at": "2026-01-02T00:00:00Z", "app": {"slug": "github-actions"}},
        ]}
        candidate.require_checks("f" * 40, lambda _path: good)
        with self.assertRaisesRegex(ValueError, "required accepted PR check is missing"):
            candidate.require_checks("f" * 40, lambda _path: {"check_runs": []})
        failed = {"check_runs": [dict(good["check_runs"][1], conclusion="failure"), good["check_runs"][2]]}
        with self.assertRaisesRegex(ValueError, "not successful"):
            candidate.require_checks("f" * 40, lambda _path: failed)

    def check_receipt_fixture(self):
        receipt = {
            "schema": 1,
            "repository": candidate.REPOSITORY,
            "workflow_path": candidate.CHECK_WORKFLOW,
            "run_id": 124,
            "run_attempt": 2,
            "event": "pull_request",
            "workflow_sha": "a" * 40,
            "head_sha": "b" * 40,
            "base_sha": "f" * 40,
            "pr_number": 44,
            "tested_tree": "c" * 40,
            "check": "test",
        }
        bundle = io.BytesIO()
        with zipfile.ZipFile(bundle, "w", zipfile.ZIP_DEFLATED) as archive:
            archive.writestr("check-candidate.json", json.dumps(receipt))
        data = bundle.getvalue()
        artifact = {
            "id": 777,
            "name": "muninn-check-candidate-124-2",
            "digest": "sha256:" + candidate.sha(data),
            "expired": False,
        }
        run = {
            "id": 124,
            "run_attempt": 2,
            "path": candidate.CHECK_WORKFLOW,
            "event": "pull_request",
            "conclusion": "success",
            "head_sha": "b" * 40,
            "repository": {"full_name": candidate.REPOSITORY},
            "head_repository": {"full_name": candidate.REPOSITORY},
            "pull_requests": [{"number": 44, "base": {"sha": "f" * 40}}],
        }
        accepted = {
            "pr_number": 44,
            "pr_head_sha": "b" * 40,
            "source_base_sha": "f" * 40,
            "accepted_tree": "c" * 40,
            "check_receipt": {
                "run_id": 124,
                "run_attempt": 2,
                "artifact_id": 777,
                "artifact_digest": artifact["digest"],
                "workflow_sha": "a" * 40,
                "workflow_path": candidate.CHECK_WORKFLOW,
                "tested_tree": "c" * 40,
                "base_sha": "f" * 40,
            },
        }

        def api_fn(path, binary=False):
            if path.endswith("/actions/runs/124"):
                return run
            if path.endswith("/actions/runs/124/artifacts?per_page=100"):
                return {"artifacts": [artifact]}
            if path.endswith("/actions/artifacts/777/zip"):
                return data
            if "/contents/" in path:
                return {"sha": "e" * 40}
            raise AssertionError(path)

        return accepted, api_fn

    def test_release_revalidates_exact_unexpired_dagger_receipt_and_policy(self):
        accepted, api_fn = self.check_receipt_fixture()
        with mock.patch.object(candidate, "api", side_effect=api_fn):
            result = candidate.verify_check_receipt(accepted, "d" * 40, api_fn=api_fn)
        self.assertEqual(result["tested_tree"], accepted["accepted_tree"])

    def test_release_rejects_expired_or_changed_check_artifact(self):
        accepted, api_fn = self.check_receipt_fixture()
        def expired(path, binary=False):
            value = api_fn(path, binary)
            if path.endswith("artifacts?per_page=100"):
                value["artifacts"][0]["expired"] = True
            return value
        with mock.patch.object(candidate, "api", side_effect=expired):
            with self.assertRaisesRegex(ValueError, "missing, expired, or has changed identity"):
                candidate.verify_check_receipt(accepted, "d" * 40, api_fn=expired)

    def test_release_rejects_modified_check_helper_policy(self):
        accepted, api_fn = self.check_receipt_fixture()
        def changed_helper(path, binary=False):
            if "/contents/build/release_candidate.py?ref=" + "a" * 40 in path:
                return {"sha": "f" * 40}
            return api_fn(path, binary)
        with mock.patch.object(candidate, "api", side_effect=changed_helper):
            with self.assertRaisesRegex(ValueError, "build/release_candidate.py"):
                candidate.verify_check_receipt(accepted, "d" * 40, api_fn=changed_helper)


if __name__ == "__main__":
    unittest.main()
