"""Static fail-closed checks for the protected production release workflow."""
import hashlib
import json
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
                        "needs: prepare", "gh release view \"$INPUT_TAG\"",
                        "GH_REPO: ${{ github.repository }}"):
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

    def test_ad_hoc_mode_is_explicit_and_keeps_updater_verification(self):
        self.assertIn("default: developer-id", self.workflow)
        preflight = self.workflow.split("Fail-closed Apple signing credential preflight", 1)[1]
        self.assertIn("if: inputs.apple_signing == 'developer-id'", preflight.split("env:", 1)[0])
        notarize = self.workflow.split("Sign, notarize, staple and assess production app", 1)[1]
        self.assertIn("if: inputs.apple_signing == 'developer-id'", notarize.split("env:", 1)[0])
        ad_hoc = self.workflow.split("Record explicitly selected ad-hoc signing", 1)[1].split("Create final Tauri-compatible", 1)[0]
        self.assertIn("if: inputs.apple_signing == 'ad-hoc'", ad_hoc)
        self.assertIn("codesign --verify --deep --strict", ad_hoc)
        self.assertIn("No Developer ID identity, notarization or Gatekeeper acceptance is claimed", ad_hoc)
        archive = self.workflow.split("Create final Tauri-compatible archive, sign and verify", 1)[1]
        self.assertIn("signer sign Muninn.app.tar.gz", archive)
        self.assertIn("minisign -Vm Muninn.app.tar.gz", archive)

    def test_review_and_publish_contain_corresponding_source_and_evidence(self):
        for asset in ("corresponding-source/**", "signing-notarization-evidence.txt"):
            self.assertIn(asset, self.workflow)
        self.assertIn("build/release_sources.py prepare", self.workflow)
        self.assertIn('source_root=release-assets/corresponding-source', self.workflow)
        self.assertIn('find "$source_root" -type f -print0', self.workflow)
        self.assertIn("while IFS= read -r -d '' file; do", self.workflow)
        self.assertIn('if ((${#source_files[@]} == 0))', self.workflow)
        self.assertNotIn('release-assets/corresponding-source/**', self.workflow)
        self.assertIn("release-assets/signing-notarization-evidence.txt", self.workflow)

    def test_release_source_upload_uses_real_unique_names_and_manifest(self):
        block = self.workflow.split('source_root=release-assets/corresponding-source', 1)[1]
        script = 'set -euo pipefail\nsource_root=release-assets/corresponding-source' + block
        script = script.replace('gh release view "$INPUT_TAG" >/dev/null', ':')
        script = script.replace('gh release upload "$INPUT_TAG" "${assets[@]}" --clobber', 'printf \'%s\\n\' "${assets[@]}"')
        script = script.replace("trap 'rm -rf \"$upload_dir\"' EXIT", ':')
        script = re.sub(r'(?m)^          ', '', script)
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            source = root / 'release-assets/corresponding-source'
            (source / 'nested one').mkdir(parents=True)
            (source / 'nested two').mkdir()
            (source / 'nested one' / 'NOTICE').write_text('notice a')
            (source / 'nested two' / 'NOTICE').write_text('notice b')
            completed = subprocess.run(['bash', '-c', script], cwd=root, text=True, capture_output=True)
            self.assertEqual(completed.returncode, 0, completed.stderr)
            (source / 'nested one' / 'NOTICE').write_text('notice a')
            completed = subprocess.run(['bash', '-c', script], cwd=root, text=True, capture_output=True)
            self.assertEqual(completed.returncode, 0, completed.stderr)
            uploaded = [Path(line) for line in completed.stdout.splitlines() if Path(line).name.startswith('source-') and Path(line).exists() and Path(line).name != 'source-assets-manifest.json']
            self.assertEqual(len(uploaded), 2)
            manifest_path = next(Path(line) for line in completed.stdout.splitlines() if Path(line).name == 'source-assets-manifest.json')
            manifest_records = json.loads(manifest_path.read_text())
            expected = {
                'nested one/NOTICE': ('notice a',),
                'nested two/NOTICE': ('notice b',),
            }
            self.assertEqual({record['original_path'] for record in manifest_records}, set(expected))
            self.assertEqual(len({record['asset_name'] for record in manifest_records}), 2)
            for record in manifest_records:
                content = expected[record['original_path']][0].encode()
                self.assertEqual(record['asset_name'], f"source-{hashlib.sha256(record['original_path'].encode()).hexdigest()[:12]}-NOTICE")
                self.assertEqual(record['sha256'], hashlib.sha256(content).hexdigest())
            self.assertEqual(len({p.name for p in uploaded}), 2)
            self.assertTrue(all('#' not in p.name and p.is_file() for p in uploaded))
            # The workflow removes its temporary staging directory on exit; verify manifest construction contract too.
            self.assertIn('"original_path": relative', self.workflow)
            self.assertIn('"asset_name": asset_name', self.workflow)
            self.assertIn('"sha256": hashlib.sha256(source.read_bytes()).hexdigest()', self.workflow)
            self.assertIn('source_root=', self.workflow)
            empty = root / 'empty'
            empty.mkdir()
            fail_script = script.replace('source_root=release-assets/corresponding-source', f'source_root={empty}')
            rejected = subprocess.run(['bash', '-c', fail_script], cwd=root, text=True, capture_output=True)
            self.assertNotEqual(rejected.returncode, 0)
            self.assertIn('No corresponding-source files found', rejected.stderr)

    def test_hosted_checks_run_contract_test(self):
        self.assertIn("python3 -m unittest discover -s build -p 'test_release_production_contract.py' -v", self.checks)


if __name__ == "__main__":
    unittest.main()
