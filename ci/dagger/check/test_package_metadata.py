"""GHCR publisher must not confuse a cache package with CI evidence."""

import unittest

from package_metadata import package_metadata


def workflow_run(name="Checks", event="pull_request", conclusion="failure"):
    return {
        "repository": {"full_name": "iarnvidr/muninn"},
        "workflow_run": {
            "name": name,
            "event": event,
            "status": "completed",
            "conclusion": conclusion,
            "head_repository": {"full_name": "iarnvidr/muninn"},
            "head_sha": "a" * 40,
            "head_branch": "codex/muninn-modular-cache-runtime",
            "id": 42417777767,
            "run_attempt": 2,
            "pull_requests": [{"number": 43}],
        },
    }


class PackagePublisherContract(unittest.TestCase):
    def test_failed_crate_check_still_publishes_cache_not_test_receipt(self):
        data = package_metadata(workflow_run())
        self.assertEqual(data["kind"], "cargo-cache-v1")
        self.assertEqual(data["artifact"], "muninn-linux-cargo-snapshot-42417777767-2")
        self.assertEqual(data["image"], "ghcr.io/iarnvidr/muninn-ci-cache")
        self.assertEqual(data["tags"], f"run-42417777767-2 sha-{'a' * 40} pr-43")

    def test_only_green_default_branch_updates_main_cache(self):
        event = workflow_run(event="push", conclusion="success")
        event["workflow_run"]["head_branch"] = "main"
        self.assertTrue(package_metadata(event)["tags"].endswith(" main"))
        event["workflow_run"]["conclusion"] = "failure"
        self.assertNotIn(" main", package_metadata(event)["tags"])

    def test_mac_package_is_not_a_release_and_needs_a_successful_build(self):
        event = workflow_run(name="Apple Silicon build", conclusion="success")
        data = package_metadata(event)
        self.assertEqual(data["kind"], "macos-candidate-v1")
        self.assertEqual(data["artifact"], "muninn-macos-arm64")
        event["workflow_run"]["conclusion"] = "failure"
        with self.assertRaisesRegex(ValueError, "no publishable artifact"):
            package_metadata(event)

    def test_fork_is_rejected_even_if_it_copies_the_workflow_name(self):
        event = workflow_run()
        event["workflow_run"]["head_repository"]["full_name"] = "attacker/fork"
        with self.assertRaisesRegex(ValueError, "untrusted fork"):
            package_metadata(event)

    def test_bad_sha_or_unexpected_event_is_rejected(self):
        event = workflow_run()
        event["workflow_run"]["head_sha"] = "$(whoami)"
        with self.assertRaisesRegex(ValueError, "invalid source commit"):
            package_metadata(event)
        event = workflow_run(event="pull_request_target")
        with self.assertRaisesRegex(ValueError, "unexpected workflow trigger"):
            package_metadata(event)


if __name__ == "__main__":
    unittest.main()
