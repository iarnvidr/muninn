"""Static fail-closed checks for the protected production release workflow."""
import re
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


class ProductionReleaseContract(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.workflow = (ROOT / ".github/workflows/release.yml").read_text()
        cls.checks = (ROOT / ".github/workflows/check.yml").read_text()
        cls.signer = (ROOT / "build/sign-notarize-macos.sh").read_text()

    def test_protected_main_version_tag_and_publish_gates_remain(self):
        for snippet in ("if: github.ref == 'refs/heads/main'", "name: muninn-release",
                        "test \"$INPUT_TAG\" = \"v$INPUT_VERSION\"",
                        "test \"$tag_commit\" = \"$GITHUB_SHA\"",
                        "if: github.ref == 'refs/heads/main' && inputs.publish",
                        "needs: prepare", "gh release view \"$INPUT_TAG\""):
            self.assertIn(snippet, self.workflow)
        self.assertIn("workflow_dispatch:", self.workflow)
        self.assertNotRegex(self.workflow, r"(?m)^\s+release:\s*create")

    def test_credential_preflight_precedes_expensive_build(self):
        preflight = self.workflow.index("Fail-closed Apple signing credential preflight")
        build = self.workflow.index("Build, test and sign exact app bundle")
        self.assertLess(preflight, build)
        names = re.findall(r"APPLE_[A-Z0-9_]+", self.signer.split("for n in ", 1)[1].split("; do", 1)[0])
        self.assertEqual(len(names), 6)
        for name in names:
            self.assertIn(name, self.workflow[preflight:build])

    def test_notarization_precedes_archive_and_updater_signing(self):
        notarize = self.workflow.index("build/sign-notarize-macos.sh")
        archive = self.workflow.index("Create final Tauri-compatible archive, sign and verify")
        signer = self.workflow.index("signer sign Muninn.app.tar.gz")
        self.assertLess(notarize, archive)
        self.assertLess(archive, signer)
        self.assertIn("rm -f Muninn.app.tar.gz Muninn.app.tar.gz.sig", self.workflow)
        self.assertIn("stapler staple", self.signer)

    def test_review_and_publish_contain_corresponding_source_and_evidence(self):
        for asset in ("corresponding-source/**", "signing-notarization-evidence.txt"):
            self.assertIn(asset, self.workflow)
        self.assertIn("build/release_sources.py prepare", self.workflow)
        self.assertIn('source_root=release-assets/corresponding-source', self.workflow)
        self.assertIn('find "$source_root" -type f -print0', self.workflow)
        self.assertIn('mapfile -d \'\' -t source_files', self.workflow)
        self.assertIn('if ((${#source_files[@]} == 0))', self.workflow)
        self.assertNotIn('release-assets/corresponding-source/**', self.workflow)
        self.assertIn("release-assets/signing-notarization-evidence.txt", self.workflow)

    def test_release_source_enumeration_handles_nested_and_empty(self):
        block = self.workflow.split('source_root=release-assets/corresponding-source', 1)[1].split('gh release upload', 1)[0]
        script = 'set -euo pipefail\nsource_root=release-assets/corresponding-source' + block
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            source = root / 'release-assets/corresponding-source'
            (source / 'nested one').mkdir(parents=True)
            (source / 'nested two').mkdir()
            (source / 'nested one' / 'NOTICE').write_text('notice a')
            (source / 'nested two' / 'NOTICE').write_text('notice b')
            completed = subprocess.run(['bash', '-c', script], cwd=root, text=True, capture_output=True)
            self.assertEqual(completed.returncode, 0, completed.stderr)
            self.assertEqual(len(re.findall(r'^source_files\[', completed.stdout, re.M)), 0)
            # Verify enumeration and flattened asset names using the exact workflow logic.
            check = subprocess.run(['bash', '-c', script + '\nprintf \'%s\\n\' "${source_files[@]}"'], cwd=root, text=True, capture_output=True)
            self.assertEqual(check.returncode, 0, check.stderr)
            paths = check.stdout.splitlines()
            self.assertEqual(len(paths), 2)
            self.assertNotEqual(Path(paths[0]).parent, Path(paths[1]).parent)
            self.assertIn('source_root=', self.workflow)
            empty = root / 'empty'
            empty.mkdir()
            fail_script = script.replace('source_root=release-assets/corresponding-source', f'source_root={empty}')
            rejected = subprocess.run(['bash', '-c', fail_script], cwd=root, text=True, capture_output=True)
            self.assertNotEqual(rejected.returncode, 0)
            self.assertIn('No corresponding-source files found', rejected.stderr)

    def test_hosted_checks_run_contract_test(self):
        self.assertIn("python3 -m unittest -v test_release_production_contract", self.checks)


if __name__ == "__main__":
    unittest.main()
