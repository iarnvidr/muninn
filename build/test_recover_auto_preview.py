"""Regression contracts for resuming an accepted, interrupted preview draft."""
from pathlib import Path
import unittest
from unittest.mock import patch

import recover_auto_preview as recovery


SOURCE = "c" * 40
MAIN = "d" * 40
MAPPING = 42465050220
REVIEW = 42465077729
NAME = f"muninn-auto-preview-{SOURCE}"


def draft(body=None):
    return {
        "tag_name": "v0.2.23", "draft": True, "prerelease": True,
        "body": body or f"Preview\n<!-- muninn-auto-preview source={SOURCE} mapping={MAPPING} -->",
        "assets": [],
    }


def fake_api(path, release=None, tag_sha=SOURCE, mapping_ok=True, artifact_ok=True):
    release = release if release is not None else draft()
    if "/releases?" in path:
        return [release]
    if path.endswith("/git/ref/heads/main"):
        return {"object": {"sha": MAIN}}
    if path.endswith("/git/ref/tags/v0.2.23"):
        return {"object": {"type": "commit", "sha": tag_sha}}
    if path.endswith(f"/actions/runs/{MAPPING}"):
        return {"repository": {"full_name": "iarnvidr/muninn"},
                "path": ".github/workflows/accept-native.yml",
                "event": "push", "head_branch": "main", "head_sha": SOURCE,
                "conclusion": "success" if mapping_ok else "failure"}
    if "/actions/artifacts?" in path:
        return {"artifacts": [{"id": 123, "name": NAME, "expired": False,
                               "workflow_run": {"id": REVIEW, "head_sha": SOURCE if artifact_ok else MAIN}}]}
    if path.endswith(f"/actions/runs/{REVIEW}"):
        return {"repository": {"full_name": "iarnvidr/muninn"},
                "path": ".github/workflows/auto-preview-release.yml",
                "event": "workflow_run", "head_sha": SOURCE}
    if path.endswith(f"/actions/runs/{REVIEW}/jobs?per_page=100"):
        return {"jobs": [{"name": "package", "conclusion": "success"},
                         {"name": "publish", "conclusion": "failure"}]}
    raise AssertionError(f"unexpected GitHub lookup: {path}")


class RecoveryTests(unittest.TestCase):
    def decide(self, **api_overrides):
        with patch.object(recovery.release_candidate, "version", return_value="0.2.23"):
            return recovery.decision(
                Path("/unused"),
                api_fn=lambda path: fake_api(path, **api_overrides),
                git_fn=lambda _root, *_cmd: MAIN,
                ancestor_fn=lambda _root, _source: True,
            )

    def test_recovers_previous_accepted_tree_without_retagging(self):
        result = self.decide()
        self.assertTrue(result["recover_needed"])
        self.assertEqual(result["source_sha"], SOURCE)
        self.assertEqual(result["mapping_run_id"], str(MAPPING))
        self.assertEqual(result["review_run_id"], REVIEW)
        self.assertEqual(result["artifact_name"], NAME)

    def test_no_draft_is_a_safe_noop(self):
        published = {**draft(), "draft": False}
        self.assertEqual(self.decide(release=published), {"recover_needed": False})

    def test_refuses_mismatched_tag_mapping_and_review_artifact(self):
        for overrides, reason in (
            ({"tag_sha": MAIN}, "immutable draft tag"),
            ({"mapping_ok": False}, "successful trusted main acceptance"),
            ({"artifact_ok": False}, "unique retained review artifact"),
            ({"release": draft(body="No provenance")}, "unique trusted preview marker"),
        ):
            with self.subTest(overrides=overrides), self.assertRaisesRegex(ValueError, reason):
                self.decide(**overrides)

    def test_refuses_source_not_ancestor_of_main(self):
        with patch.object(recovery.release_candidate, "version", return_value="0.2.23"):
            with self.assertRaisesRegex(ValueError, "not an ancestor"):
                recovery.decision(
                    Path("/unused"), api_fn=fake_api,
                    git_fn=lambda _root, *_cmd: MAIN,
                    ancestor_fn=lambda _root, _source: False,
                )


if __name__ == "__main__":
    unittest.main()
