"""Execute the source-mirror/cache contract with real, offline Cargo builds.

The fixture has two independent libraries and one binary. Cargo's JSON `fresh`
flag identifies actual compilation, rather than inferring cache hits from time
or matching the pipeline's source text. No project files or shared target are
modified: every test uses its own disposable input, cache, and Cargo home.

These tests exercise run-rust.sh, not a running Dagger engine. Hosted CI must
still execute the pinned module and the native macOS acceptance checks.
"""

from __future__ import annotations

import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time
import unittest


HELPER = Path(__file__).with_name("run-rust.sh")
PACKAGES = {"cache_fixture_base", "cache_fixture_other", "cache_fixture_app"}


class CargoCacheContract(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        # These are deliberately required, not skip conditions: this is a CI
        # correctness gate and an absent compiler must not look like success.
        for executable in ("bash", "cargo", "rustc", "rsync"):
            if shutil.which(executable) is None:
                raise RuntimeError(f"cache-contract tests require {executable} on PATH")
        if not HELPER.is_file():
            raise RuntimeError(f"missing production cache helper: {HELPER}")

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="muninn-cargo-cache-test-")
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.source = self.root / "input"
        self.cache = self.root / "cache"
        self.target = self.cache / "target" / "test-abi"
        self.mirror = self.cache / "worktree"
        self.env = os.environ.copy()
        self.env.update({
            "CARGO_HOME": str(self.root / "cargo-home"),
            "CARGO_TARGET_DIR": str(self.target),
            "CARGO_INCREMENTAL": "0",
            "CARGO_BUILD_JOBS": "1",
            "CARGO_TERM_COLOR": "never",
        })
        # Do not let the developer's optional compiler wrapper alter which
        # compilation/freshness behavior this fixture is testing.
        self.env.pop("RUSTC_WRAPPER", None)
        self.env.pop("RUSTC_WORKSPACE_WRAPPER", None)
        self.env.pop("RUSTFLAGS", None)
        self.env.pop("CARGO_ENCODED_RUSTFLAGS", None)
        self.write("Cargo.toml", '[workspace]\nresolver = "3"\nmembers = ["base", "other", "app"]\n')
        for name in ("base", "other", "app"):
            manifest = (
                '[package]\n'
                f'name = "cache_fixture_{name}"\n'
                'version = "0.0.0"\n'
                'edition = "2021"\n'
            )
            if name == "app":
                manifest += (
                    '[dependencies]\n'
                    'cache_fixture_base = { path = "../base" }\n'
                    'cache_fixture_other = { path = "../other" }\n'
                )
            self.write(f"{name}/Cargo.toml", manifest)
        self.write("base/src/lib.rs", "pub fn value() -> u32 { 1 }\n")
        self.write("other/src/lib.rs", "pub fn value() -> u32 { 7 }\n")
        self.write("app/src/main.rs", 'fn main() { println!("{}", cache_fixture_base::value() + cache_fixture_other::value()); }\n')
        self.write("Cargo.lock", '''version = 4

[[package]]
name = "cache_fixture_app"
version = "0.0.0"
dependencies = ["cache_fixture_base", "cache_fixture_other"]

[[package]]
name = "cache_fixture_base"
version = "0.0.0"

[[package]]
name = "cache_fixture_other"
version = "0.0.0"
''')

    def write(self, relative: str, contents: str):
        destination = self.source / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_text(contents)
        return destination

    def build(self, *, succeeds: bool = True, env: dict | None = None):
        result = subprocess.run(
            ["bash", str(HELPER), str(self.source), str(self.cache),
             "cargo", "build", "--offline", "--locked", "--workspace", "--message-format=json"],
            cwd=self.root, env=env or self.env, text=True, capture_output=True,
            timeout=90,
        )
        output = result.stdout + result.stderr
        if succeeds:
            self.assertEqual(result.returncode, 0, output)
        else:
            self.assertNotEqual(result.returncode, 0, output)
        fresh = {}
        for line in result.stdout.splitlines():
            try:
                message = json.loads(line)
            except json.JSONDecodeError:
                continue
            if message.get("reason") == "compiler-artifact" and message["target"]["name"] in PACKAGES:
                fresh[message["target"]["name"]] = message["fresh"]
        if succeeds:
            self.assertEqual(set(fresh), PACKAGES, output)
        return fresh, output

    def assert_program_value(self, expected: int):
        executable = self.target / "debug" / "cache_fixture_app"
        result = subprocess.run([str(executable)], text=True, capture_output=True, timeout=10)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), str(expected))

    def test_cold_then_identical_and_timestamp_only_inputs_are_fresh(self):
        fresh, _ = self.build()
        self.assertEqual(fresh, dict.fromkeys(PACKAGES, False))
        self.assert_program_value(8)
        original_times = {p.relative_to(self.mirror): p.stat().st_mtime_ns
                          for p in self.mirror.rglob("*") if p.is_file()}

        fresh, _ = self.build()
        self.assertEqual(fresh, dict.fromkeys(PACKAGES, True))

        # A new checkout can give identical files new (even future) mtimes. The
        # checksum mirror must neither propagate them nor rewrite the files.
        later = time.time_ns() + 86_400_000_000_000
        for path in self.source.rglob("*"):
            if path.is_file():
                os.utime(path, ns=(later, later))
        fresh, _ = self.build()
        self.assertEqual(fresh, dict.fromkeys(PACKAGES, True))
        self.assertEqual({p.relative_to(self.mirror): p.stat().st_mtime_ns
                          for p in self.mirror.rglob("*") if p.is_file()}, original_times)

    def test_equal_or_old_input_mtime_cannot_hide_same_length_content_changes(self):
        self.build()
        source = self.source / "base/src/lib.rs"
        previous_build_time = (self.mirror / "base/src/lib.rs").stat().st_mtime_ns
        for value, timestamp in ((2, previous_build_time), (3, 1_000_000_000)):
            with self.subTest(value=value, source_mtime=timestamp):
                source.write_text(f"pub fn value() -> u32 {{ {value} }}\n")
                os.utime(source, ns=(timestamp, timestamp))
                fresh, _ = self.build()
                self.assertEqual(fresh, {
                    "cache_fixture_base": False,
                    "cache_fixture_other": True,
                    "cache_fixture_app": False,
                })
                self.assert_program_value(value + 7)

    def test_paired_worktree_and_target_snapshot_restore_preserves_freshness(self):
        self.build()
        # Model the exported pair being restored at the same stable cache path.
        # copy2 preserves timestamps, as the pinned Dagger host importer does.
        # Merely restoring target without its source mirror is insufficient.
        snapshot = self.root / "exported-cache"
        shutil.copytree(self.cache, snapshot)
        self.cache.rename(self.root / "previous-cache")
        shutil.copytree(snapshot, self.cache)
        later = time.time_ns() + 86_400_000_000_000
        for path in self.source.rglob("*"):
            if path.is_file():
                os.utime(path, ns=(later, later))
        fresh, _ = self.build()
        self.assertEqual(fresh, dict.fromkeys(PACKAGES, True))
        self.assert_program_value(8)

    def test_deleted_input_is_not_resurrected_from_the_cached_worktree(self):
        self.build()
        (self.source / "base/src/lib.rs").unlink()
        fresh, _ = self.build(succeeds=False)
        self.assertNotIn("cache_fixture_app", fresh)
        self.assertFalse((self.mirror / "base/src/lib.rs").exists())

    def test_future_target_timestamp_fails_closed_instead_of_reusing_stale_code(self):
        self.build()
        future = time.time_ns() + 3_600_000_000_000
        # This includes Cargo's dep-info/build anchor; an older next runner must
        # not conclude that changed source is older than the last compilation.
        for path in self.target.rglob("*"):
            if path.is_file():
                os.utime(path, ns=(future, future))
        source = self.write("base/src/lib.rs", "pub fn value() -> u32 { 2 }\n")
        os.utime(source, ns=(1_000_000_000, 1_000_000_000))
        fresh, output = self.build(succeeds=False)
        self.assertFalse(fresh, output)
        self.assertRegex(output.lower(), r"future|clock|timestamp")

    def test_symlinked_input_is_rejected_instead_of_trusting_the_target_mtime(self):
        self.build()
        self.write("base/src/alternative.rs", "pub fn value() -> u32 { 2 }\n")
        source = self.source / "base/src/lib.rs"
        source.unlink()
        source.symlink_to("alternative.rs")
        fresh, output = self.build(succeeds=False)
        self.assertFalse(fresh, output)
        self.assertRegex(output.lower(), r"symlink|symbolic link")

    def test_target_outside_the_designated_cache_is_rejected_without_mutation(self):
        outside = self.root / "outside"
        outside.mkdir()
        sentinel = outside / "user-file"
        sentinel.write_text("keep me")
        env = {**self.env, "CARGO_TARGET_DIR": str(outside)}
        fresh, output = self.build(succeeds=False, env=env)
        self.assertFalse(fresh, output)
        self.assertEqual(sentinel.read_text(), "keep me")
        self.assertEqual(list(outside.iterdir()), [sentinel])

    def test_cache_nested_inside_input_is_rejected_before_self_mirroring(self):
        self.cache = self.source / "nested-cache"
        self.target = self.cache / "target" / "test-abi"
        env = {**self.env, "CARGO_TARGET_DIR": str(self.target)}
        fresh, output = self.build(succeeds=False, env=env)
        self.assertFalse(fresh, output)
        self.assertFalse(self.cache.exists(), "invalid layout must fail before creating the mirror")


if __name__ == "__main__":
    unittest.main()
