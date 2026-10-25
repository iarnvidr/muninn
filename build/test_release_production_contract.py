"""Static fail-closed checks for the protected production release workflow."""
import json
import os
import re
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

    def test_archive_version_check_resolves_helper_from_output_directory(self):
        archive = self.workflow.split("Create final Tauri-compatible archive, sign and verify", 1)[1]
        archive = archive.split("Prepare corresponding source archives", 1)[0]
        command = next(line.strip() for line in archive.splitlines() if " check-version " in line)
        env = dict(os.environ, GITHUB_WORKSPACE=str(ROOT), bundle_version="0.2.14", INPUT_VERSION="0.2.14")
        with tempfile.TemporaryDirectory() as output:
            completed = subprocess.run(["bash", "-c", command], cwd=output, env=env, text=True, capture_output=True)
            self.assertEqual(completed.returncode, 0, completed.stderr)
            env["bundle_version"] = "0.2.13"
            mismatch = subprocess.run(["bash", "-c", command], cwd=output, env=env, text=True, capture_output=True)
            self.assertNotEqual(mismatch.returncode, 0, "version mismatch must still block release")
            old_command = 'python3 build/updater_release.py check-version "$bundle_version" "$INPUT_VERSION"'
            old = subprocess.run(["bash", "-c", old_command], cwd=output, env=env, text=True, capture_output=True)
            self.assertNotEqual(old.returncode, 0, "the old relative path must reproduce the failure")

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

    def test_review_keeps_corresponding_source_evidence_and_updater_signature(self):
        for asset in ("corresponding-source/**", "signing-notarization-evidence.txt"):
            self.assertIn(asset, self.workflow)
        self.assertIn("build/release_sources.py prepare", self.workflow)
        self.assertIn("build/package-dmg.sh", self.workflow)
        self.assertIn("${{ runner.temp }}/muninn/Muninn.app.tar.gz.sig", self.workflow)

    def test_publish_keeps_only_the_dmg_and_required_updater_files(self):
        block = self.workflow.split('Upload the DMG and required updater files only', 1)[1]
        script = re.sub(r'(?m)^          ', '', block.split('run: |', 1)[1])
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            assets = root / 'release-assets'
            assets.mkdir()
            for name in ('Muninn_0.2.14_macos-arm64.dmg', 'unexpected.dmg', 'latest.json',
                         'Muninn.app.tar.gz', 'Muninn.app.tar.gz.sig', 'SHA256SUMS', 'source.tar.xz'):
                (assets / name).write_text('fixture')
            binary = root / 'bin'
            binary.mkdir()
            gh = binary / 'gh'
            gh.write_text('#!/usr/bin/env python3\nimport json,os,sys\n'
                          'with open(os.environ["GH_CALLS"],"a") as f: f.write(json.dumps(sys.argv[1:])+"\\n")\n')
            gh.chmod(0o755)
            calls = root / 'calls.jsonl'
            env = dict(os.environ, PATH=str(binary) + os.pathsep + os.environ['PATH'],
                       GH_CALLS=str(calls), INPUT_TAG='v0.2.14', INPUT_VERSION='0.2.14')
            completed = subprocess.run(['bash', '-c', script], cwd=root, env=env, text=True, capture_output=True)
            self.assertEqual(completed.returncode, 0, completed.stderr)
            uploads = [json.loads(line) for line in calls.read_text().splitlines()
                       if json.loads(line)[:2] == ['release', 'upload']]
            self.assertEqual(uploads, [['release', 'upload', 'v0.2.14',
                                       'release-assets/Muninn_0.2.14_macos-arm64.dmg',
                                       'release-assets/latest.json', 'release-assets/Muninn.app.tar.gz', '--clobber']])
            (assets / 'Muninn_0.2.14_macos-arm64.dmg').unlink()
            calls.write_text('')
            rejected = subprocess.run(['bash', '-c', script], cwd=root, env=env, text=True, capture_output=True)
            self.assertNotEqual(rejected.returncode, 0, 'missing DMG must block upload')
            self.assertFalse(any(json.loads(line)[:2] == ['release', 'upload']
                                 for line in calls.read_text().splitlines()))
            (assets / 'Muninn_0.2.14_macos-arm64.dmg').write_text('fixture')
            (assets / 'latest.json').unlink()
            calls.write_text('')
            rejected = subprocess.run(['bash', '-c', script], cwd=root, env=env, text=True, capture_output=True)
            self.assertNotEqual(rejected.returncode, 0, 'missing updater manifest must block upload')
            self.assertFalse(any(json.loads(line)[:2] == ['release', 'upload']
                                 for line in calls.read_text().splitlines()))

    def test_hosted_checks_run_contract_test(self):
        self.assertIn("python3 -m unittest discover -s build -p 'test_release_production_contract.py' -v", self.checks)


if __name__ == "__main__":
    unittest.main()
