"""Static guardrails, not a substitute for executing the pinned Dagger engine."""

import json
import re
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
        cls.publisher = (ROOT / ".github/workflows/package-snapshots.yml").read_text()

    def test_native_module_and_hosted_cli_use_same_pinned_beta(self):
        self.assertEqual(self.config["engineVersion"], "v1.0.0-beta.25")
        self.assertEqual(self.config["runtime"]["source"], "dang")
        self.assertEqual(self.workspace["modules"]["muninn-ci"]["source"], "./ci/dagger/check")
        self.assertTrue(self.workspace["modules"]["muninn-ci"]["main"])
        self.assertIn("version: " + self.config["engineVersion"].removeprefix("v"), self.linux)
        # The pinned action must only install the CLI once. Its legacy
        # `dagger --silent version` check fails on a second Dagger v1 call.
        self.assertIn("run: dagger --progress logs check", self.linux)
        self.assertNotIn("args: checks --source .", self.linux)
        self.assertIn("dagger/dagger-for-github@496f1b3d8b0d823834c13e67cf8a8e08ca3b9602", self.linux)

    def test_first_class_check_for_every_crate(self):
        names = re.findall(r"pub (\w+): Void @check", self.source)
        self.assertEqual(names, [
            "domainTests", "policyTests", "captureTests", "storageTests",
            "mediaTests", "navigationTests", "protocolTests", "clientTests",
            "coreTests", "uiAssetsTests", "serverTests", "cliTests",
            "desktopTests", "javascript", "packaging",
        ])
        self.assertNotIn("@check", self.source.split("pub exportCargoCache", 1)[1])

    def test_cargo_check_selection_covers_every_workspace_package(self):
        workspace = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]
        packages = set()
        for member in workspace["members"]:
            for directory in ROOT.glob(member):
                manifest = tomllib.loads((directory / "Cargo.toml").read_text())
                packages.add(manifest["package"]["name"])
        rust_checks = re.findall(
            r'pub (\w+Tests): Void @check \{\s*rustBase\.withMountedDirectory\("/input", (\w+Source)\)\s*(?:\.withEnvVariable\("MUNINN_CARGO_LOG", "[^"]+"\)\s*)?\.withExec\((\[[^\n]+\])\)',
            self.source)
        self.assertEqual(len(rust_checks), len(packages))
        chosen = {}
        for check, scope, command_json in rust_checks:
            command = json.loads(command_json)
            self.assertEqual(command[:4], ["bash", "/run-rust.sh", "/input", "/cache"])
            self.assertEqual(scope, check.removesuffix("Tests") + "Source")
            if check == "domainTests":
                self.assertIn("--no-default-features", command[-1])
                self.assertEqual(command[-1].count("-p muninn-domain"), 2)
                chosen[check] = "muninn-domain"
            else:
                self.assertEqual(command[4:8], ["cargo", "test", "--locked", "-p"])
                chosen[check] = command[8]
        self.assertEqual(set(chosen.values()), packages)
        self.assertEqual(chosen["desktopTests"], "muninn")

    def test_crate_sources_have_scoped_dependency_closures(self):
        skeleton = self.source.split("let workspaceSkeleton:", 1)[1].split("let domainSource:", 1)[0]
        self.assertIn('"crates/*/Cargo.toml"', skeleton)
        self.assertIn('"crates/*/src/lib.rs"', skeleton)
        self.assertIn('"src-tauri/Cargo.toml"', skeleton)
        scopes = dict(re.findall(r'let (\w+Source): Directory! \{(.*?)\n  \}', self.source, re.S))
        self.assertEqual(len(scopes), 14)

        def paths(name):
            source = scopes[name + "Source"]
            base = re.search(r'\b(\w+Source)\b', source)
            roots = paths(base.group(1).removesuffix("Source")) if base else set()
            return roots | set(re.findall(r'\.withDirectory\("/([^"/]+(?:/[^"/]+)?)"', source))

        def root(crate):
            return "core" if crate == "muninn-core" else ("src-tauri" if crate == "muninn" else "crates/" + crate.removeprefix("muninn-"))

        expected = {
            "domain": {"domain"},
            "policy": {"policy", "domain"},
            "capture": {"capture", "domain"},
            "storage": {"storage", "policy", "domain"},
            "media": {"media", "domain"},
            "navigation": {"navigation", "storage", "policy", "capture", "domain"},
            "protocol": {"protocol", "domain"},
            "client": {"client", "protocol", "navigation", "capture", "domain"},
            "core": {"core", "navigation", "storage", "policy", "capture", "domain", "media"},
            "uiAssets": {"ui-assets"},
            "server": {"server", "core", "navigation", "storage", "policy", "capture", "domain", "media", "protocol", "ui-assets"},
            "cli": {"cli", "server", "core", "navigation", "storage", "policy", "capture", "domain", "media", "protocol", "ui-assets"},
            "desktop": {"cli", "server", "core", "navigation", "storage", "policy", "capture", "domain", "media", "protocol", "ui-assets", "client"},
        }
        for name, dirs in expected.items():
            with self.subTest(name=name):
                paths_found = paths(name)
                expected_paths = {root("muninn-" + dependency) for dependency in dirs}
                if "ui-assets" in dirs:
                    expected_paths.add("ui")
                if name == "desktop":
                    expected_paths.add("src-tauri")
                    expected_paths.add("shell")
                self.assertEqual(paths_found, expected_paths)

    def test_desktop_tauri_build_time_references_are_in_source_scope(self):
        config = json.loads((ROOT / "src-tauri/tauri.conf.json").read_text())
        self.assertEqual(config["build"]["frontendDist"], "../shell")
        desktop = self.source.split("let desktopSource:", 1)[1].split("pub domainTests:", 1)[0]
        self.assertIn('withDirectory("/shell", ws.directory("/shell"))', desktop)
        self.assertIn('withFile("/LICENSE", ws.file("/LICENSE"))', desktop)

    def test_cache_transport_is_explicit_and_not_a_result_receipt(self):
        self.assertIn("path: ci/cache/linux-cargo", self.linux)
        self.assertIn("restore-keys:", self.linux)
        self.assertIn("            muninn-dagger-cargo-v1-${{ runner.os }}-${{ runner.arch }}-\n", self.linux)
        self.assertIn("actions/cache/restore@55cc8345863c7cc4c66a329aec7e433d2d1c52a9", self.linux)
        self.assertIn("restore-ghcr-cache.sh", self.linux)
        self.assertIn("muninn-linux-cargo-snapshot-${{ github.run_id }}-${{ github.run_attempt }}", self.linux)
        self.assertIn("muninn-linux-cargo-snapshot.tar.zst", self.linux)
        self.assertIn("tar --format=posix --use-compress-program='zstd -1 -T2'", self.linux)
        self.assertIn("compression-level: 0", self.linux)
        self.assertIn("always() && steps.dagger-checks.outcome != 'skipped'", self.linux)
        self.assertNotIn("continue-on-error: true", self.linux)
        self.assertIn("steps.dagger-checks.outcome == 'success'", self.linux)
        self.assertIn('source: cargoSeed', self.source)
        self.assertEqual(self.source.count('.withMountedCache("/cache", cargoCache)'), 2)
        self.assertIn('ws.directory("/ci/cache/linux-cargo"', self.source)
        self.assertIn("export-cargo-cache --nonce", self.linux)
        self.assertIn("-m ci/dagger/check api call export-cargo-cache", self.linux)
        self.assertIn("--output ci/cache/linux-cargo", self.linux)
        self.assertIn("dagger-flags: --progress logs", self.linux)
        self.assertNotIn("--wipe", self.linux)
        self.assertIn('withEnvVariable("MUNINN_CACHE_EXPORT_NONCE", nonce)', self.source)
        self.assertIn('withEnvVariable("MUNINN_CARGO_LOG", "/cache/check-logs/desktop-tests.log")', self.source)
        self.assertIn('cp -a /cache/check-logs /snapshot/', self.source)
        self.assertIn("Report failing desktop crate diagnostics", self.linux)
        self.assertIn("Separate diagnostic logs from reusable compilation cache", self.linux)
        self.assertIn("muninn-desktop-diagnostics-${{ github.run_id }}-${{ github.run_attempt }}", self.linux)
        self.assertIn("steps.cargo-export.outcome == 'success'", self.linux)
        self.assertIn("actions/cache/save@55cc8345863c7cc4c66a329aec7e433d2d1c52a9", self.linux)
        self.assertIn("steps.restore-cargo.outputs.cache-primary-key", self.linux)
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
        self.assertIn("Record complete checkout file list for source-scoped Cargo checks", self.linux)
        self.assertIn("current-source-files.txt", self.linux)
        self.assertIn('source_manifest="$cache_root/current-source-files.txt"', helper)
        self.assertIn('comm -23', helper)
        self.assertIn('rsync --recursive --checksum --links --perms "$input_dir/"', helper)
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
        self.assertNotIn("packages: write", self.linux)
        self.assertNotIn("packages: write", self.macos)

    def test_packages_are_data_only_and_published_from_a_trusted_workflow(self):
        self.assertIn("workflow_run:", self.publisher)
        self.assertIn("packages: write", self.publisher)
        self.assertIn("head_repository.full_name == 'iarnvidr/muninn'", self.publisher)
        self.assertIn("package_metadata.py", self.publisher)
        self.assertIn("group: ghcr-muninn-publisher", self.publisher)
        self.assertIn("cancel-in-progress: false", self.publisher)
        self.assertNotIn("pull_request" + "_target", self.publisher)
        self.assertIn("gh run download", self.publisher)
        for file in ("Dockerfile.cargo-cache", "Dockerfile.macos-candidate", "Dockerfile.check-evidence"):
            dockerfile = (MODULE / file).read_text()
            self.assertRegex(dockerfile, r"(?m)^FROM scratch$")
            self.assertNotRegex(dockerfile, r"(?m)^RUN ")
            self.assertRegex(dockerfile, r"(?m)^COPY ")


if __name__ == "__main__":
    unittest.main()
