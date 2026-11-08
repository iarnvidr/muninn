"""Regression guards for opt-in PR discovery and physically separate packaging."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[1]


class PrPreviewContract(unittest.TestCase):
    def test_original_release_candidate_remains_production_identity(self):
        workflow = (ROOT / ".github/workflows/macos.yml").read_text()
        self.assertIn('bash build/build-native-macos.sh "$RUNNER_TEMP/muninn"', workflow)
        self.assertIn("Record exact native release candidate", workflow)
        self.assertIn("Build separately identified Muninn Preview app", workflow)
        self.assertLess(workflow.index("Record exact native release candidate"),
                        workflow.index("Build separately identified Muninn Preview app"))
        self.assertIn("name: muninn-pr-preview-arm64", workflow)
        self.assertIn("Smoke isolated PR Preview launch", workflow)

    def test_preview_config_isolated_and_explicitly_does_not_register_links(self):
        script = (ROOT / "build/build-preview-macos.sh").read_text()
        self.assertIn('io.github.iarnvidr.muninn.preview', script)
        self.assertIn('"productName":"Muninn Preview"', script)
        self.assertIn('"infoPlist":"Info.preview.plist"', script)
        self.assertIn('CFBundleURLTypes', script)
        self.assertIn('Muninn Preview.app', script)
        self.assertIn('MUNINN_BUILD_CHANNEL=pr-preview', script)
        self.assertIn('codesign --verify --deep --strict', script)
        self.assertIn('Muninn_PR${pr}_${head:0:12}_macos-arm64', script)

    def test_pr_channel_is_discovery_only_and_build_provenance_is_visible(self):
        updater = (ROOT / "src-tauri/src/updater.rs").read_text()
        ui = (ROOT / "ui/pages.js").read_text()
        self.assertIn('PrCandidate', updater)
        self.assertIn('include_pr_builds', updater)
        self.assertIn('Production updates are disabled inside Muninn Preview', updater)
        self.assertIn('PR builds must be downloaded as separate applications', updater)
        self.assertIn('PR builds (separate app)', ui)
        self.assertIn('Open PR build on GitHub', ui)
        self.assertIn('sourceSha.slice(0, 8)', ui)
        self.assertIn('M.boot?.build', ui)
        report = (ROOT / "core/src/diagnostics.rs").read_text()
        self.assertIn('export_to_with_build', report)
        self.assertIn('"build":build.map(', report)

    def test_package_retains_preview_separately_but_not_as_signed_updater_payload(self):
        wf = (ROOT / ".github/workflows/package-snapshots.yml").read_text()
        self.assertIn('Include isolated, separately identified PR build', wf)
        self.assertIn('--name muninn-pr-preview-arm64', wf)
        self.assertNotIn('TAURI_SIGNING_PRIVATE_KEY', wf)
        self.assertNotIn('github.event.workflow_run.head_repository.full_name !=', wf)


if __name__ == "__main__":
    unittest.main()
