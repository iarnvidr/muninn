"""Static guardrails, not a substitute for executing the pinned Dagger engine."""

import json
import re
import shlex
import tomllib
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[3]
MODULE = ROOT / "ci/dagger/check"


class DaggerContract(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.source = (MODULE / "main.dang").read_text()
        cls.config = tomllib.loads((MODULE / "dagger-module.toml").read_text())
        cls.workspace = tomllib.loads((ROOT / "dagger.toml").read_text())
        cls.linux = (ROOT / ".github/workflows/check.yml").read_text()
        cls.macos = (ROOT / ".github/workflows/macos.yml").read_text()

    def test_native_module_and_hosted_cli_use_same_pinned_beta(self):
        self.assertEqual(self.config["engineVersion"], "v1.0.0-beta.25")
        self.assertEqual(self.config["runtime"]["source"], "dang")
        self.assertEqual(self.workspace["modules"]["muninn-ci"]["source"], "./ci/dagger/check")
        self.assertTrue(self.workspace["modules"]["muninn-ci"]["main"])
        self.assertIn("version: " + self.config["engineVersion"].removeprefix("v"), self.linux)
        self.assertIn("verb: check", self.linux)
        self.assertNotIn("args: checks --source .", self.linux)
        self.assertIn("dagger/dagger-for-github@496f1b3d8b0d823834c13e67cf8a8e08ca3b9602", self.linux)

    def test_exactly_four_first_class_checks(self):
        names = re.findall(r"pub (\w+): Void @check", self.source)
        self.assertEqual(names, ["coreTests", "desktopTests", "javascript", "packaging"])
        self.assertNotIn("@check", self.source.split("pub exportCargoCache", 1)[1])

    def test_cargo_check_selection_covers_every_workspace_package(self):
        workspace = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]
        packages = set()
        for member in workspace["members"]:
            for directory in ROOT.glob(member):
                manifest = tomllib.loads((directory / "Cargo.toml").read_text())
                packages.add(manifest["package"]["name"])
        rust_execs = [json.loads(value) for value in re.findall(
            r'\.withExec\((\["bash", "/run-rust.sh"[^\n]+\])\)', self.source)]
        self.assertEqual(len(rust_execs), 2)
        commands = []
        for command in rust_execs:
            self.assertEqual(command[:4], ["bash", "/run-rust.sh", "/input", "/cache"])
            if command[4:6] == ["bash", "-euc"]:
                commands.extend(shlex.split(part) for part in command[6].split(" && "))
            else:
                commands.append(command[4:])
        self.assertEqual(len(commands), 3)
        selected = {command[index + 1] for command in commands
                    for index, part in enumerate(command[:-1]) if part == "-p"}
        self.assertEqual(selected, packages)
        for command in commands:
            self.assertEqual(command[:2], ["cargo", "test"])
            self.assertIn("--locked", command)

    def test_core_input_scope_excludes_ui_but_keeps_workspace_discovery(self):
        scope = self.source.split("let coreSource:", 1)[1].split("let desktopSource:", 1)[0]
        self.assertNotIn('"ui/**"', scope)
        self.assertIn('"crates/*/Cargo.toml"', scope)
        self.assertIn('"crates/*/src/lib.rs"', scope)
        self.assertIn('"src-tauri/Cargo.toml"', scope)
        self.assertIn('"crates/navigation/**"', scope)
        self.assertIn('"crates/protocol/**"', scope)

    def test_cache_transport_is_explicit_and_not_a_result_receipt(self):
        self.assertIn("path: ci/cache/linux-cargo", self.linux)
        self.assertIn("restore-keys:", self.linux)
        self.assertIn('source: cargoSeed', self.source)
        self.assertEqual(self.source.count('.withMountedCache("/cache", cargoCache)'), 2)
        self.assertIn('ws.directory("/ci/cache/linux-cargo"', self.source)
        self.assertIn("export-cargo-cache --nonce", self.linux)
        self.assertIn("export --path ci/cache/linux-cargo", self.linux)
        self.assertNotIn("--wipe", self.linux)
        self.assertIn('withEnvVariable("MUNINN_CACHE_EXPORT_NONCE", nonce)', self.source)
        self.assertNotIn("MUNINN_CACHE_EXPORT_NONCE", self.source.split("pub exportCargoCache", 1)[0])
        self.assertIn("create-check --root", self.linux)
        self.assertIn("path: check-candidate.json", self.linux)
        self.assertIn("muninn-check-candidate-${{ github.run_id }}-${{ github.run_attempt }}", self.linux)

    def test_live_system_package_changes_partition_compiled_output(self):
        self.assertIn("dpkg-query -W | LC_ALL=C sort | sha256sum", self.source)
        self.assertIn('withEnvVariable("CARGO_TARGET_DIR", `/cache/target/${rustSystemABI}`)', self.source)
        self.assertIn('withEnvVariable("CARGO_HOME", "/cache/cargo")', self.source)

    def test_checksum_source_mirror_and_output_are_transported_together(self):
        helper = (MODULE / "run-rust.sh").read_text()
        self.assertIn('withFile("/run-rust.sh", ws.file("/ci/dagger/check/run-rust.sh"))', self.source)
        self.assertIn("rsync --recursive --checksum --links --perms --delete", helper)
        self.assertNotRegex(helper, r"^rsync .*--times", msg="snapshot mtimes must not replace content freshness")
        self.assertIn('cp -a /cache/worktree /snapshot/', self.source)
        self.assertIn("'ci/dagger/check/run-rust.sh'", self.linux)
        self.assertIn("bash build-essential git rsync", self.source)

    def test_mac_cache_is_native_and_preserves_smoke_workflow(self):
        self.assertIn("runs-on: macos-15", self.macos)
        self.assertIn("MACOSX_DEPLOYMENT_TARGET: '14.0'", self.macos)
        for required in ("rustc -vV", "xcodebuild -version", "--show-sdk-version",
                         "~/.cargo/registry", "~/.cargo/git", "muninn-native-cargo-v1-",
                         "build/native-ui-smoke.sh", "build/updater-smoke/run-macos.sh",
                         "muninn-native-candidate-${{ github.run_id }}-${{ github.run_attempt }}"):
            self.assertIn(required, self.macos)
        self.assertNotIn("muninn-dagger-cargo-v1-", self.macos)

    def test_images_pinned_and_public_jobs_have_no_private_engine(self):
        images = re.findall(r'\.from\("([^"\n]+)"\)', self.source)
        self.assertEqual(len(images), 3)
        for image in images:
            self.assertRegex(image, r"@sha256:[0-9a-f]{64}$")
        for workflow in (self.linux, self.macos):
            self.assertNotIn("_EXPERIMENTAL_DAGGER_RUNNER_HOST", workflow)
            self.assertNotIn("DAGGER_CLOUD_TOKEN", workflow)
            self.assertNotIn("cloud-token:", workflow)
            self.assertNotIn("pull_request" + "_target", workflow)
            self.assertNotIn("self" + "-hosted", workflow)


if __name__ == "__main__":
    unittest.main()
