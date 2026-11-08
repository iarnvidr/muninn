"""Contract tests for published per-crate compiled OCI artifacts."""

import argparse
import contextlib
import gzip
import importlib.util
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import time
import unittest


SPEC = importlib.util.spec_from_file_location("crate_packages", Path(__file__).with_name("crate_packages.py"))
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)
ABI = "a" * 64


class PublishedCrateTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.base = Path(self.tmp.name)
        self.root = self.base / "repo"
        self.target = self.base / "target"
        self.out = self.base / "output"
        (self.root / "crates").mkdir(parents=True)
        self.write("Cargo.toml", '[workspace]\nmembers = ["crates/*"]\n')
        self.write("Cargo.lock", '# lockfile\n')
        for name in ("alpha", "beta", "gamma"):
            self.write(f"crates/{name}/src/lib.rs", f"pub fn {name}() {{}}\n")
            deps = '[dependencies]\nalpha = { path = "../alpha" }\n' if name == "beta" else ""
            self.write(f"crates/{name}/Cargo.toml", f'[package]\nname = "{name}"\nversion = "0.1.0"\n{deps}')
            self.compiled_unit(name)

    def write(self, path: str, contents: str):
        file = self.root / path
        file.parent.mkdir(parents=True, exist_ok=True)
        file.write_text(contents)

    def compiled_unit(self, name: str):
        h = {"alpha": "1", "beta": "2", "gamma": "3"}[name] * 16
        fp = self.target / "debug/.fingerprint" / f"{name}-{h}"
        fp.mkdir(parents=True)
        (fp / f"lib-{name}.json").write_text('{"features":"[]"}')
        (fp / f"lib-{name}").write_text("fingerprint")
        lib = self.target / "debug/deps" / f"lib{name}-{h}.rlib"
        lib.parent.mkdir(parents=True, exist_ok=True)
        lib.write_bytes(("compiled-" + name).encode())
        (self.target / "debug/deps" / f"{name}-{h}.d").write_text("metadata")
        # A freshly compiled fingerprint must be newer than its sources.
        future = time.time() + 30
        for file in fp.iterdir():
            os.utime(file, (future, future))

    def plan(self):
        return MODULE.plans(self.root, "linux-amd64", ABI)

    def export(self):
        MODULE.export_packages(argparse.Namespace(root=self.root, target=self.target,
            source_root=self.root, output=self.out, platform="linux-amd64", abi=ABI,
            require_all=True))

    def test_dependency_change_invalidates_only_reverse_dependency_closure(self):
        original = self.plan()
        self.write("crates/alpha/src/lib.rs", "pub fn alpha() { panic!() }\n")
        changed = self.plan()
        self.assertNotEqual(original["alpha"]["identity"], changed["alpha"]["identity"])
        self.assertNotEqual(original["beta"]["identity"], changed["beta"]["identity"])
        self.assertEqual(original["gamma"]["identity"], changed["gamma"]["identity"])

    def test_unrelated_lockfile_entry_does_not_rebuild_other_crates(self):
        before = self.plan()
        self.write("Cargo.lock", '# lockfile\nversion = 4\n[[package]]\nname = "unused"\nversion = "1.0.0"\n')
        after = self.plan()
        self.assertEqual({k: v["identity"] for k,v in before.items()},
                         {k: v["identity"] for k,v in after.items()})

    def test_per_crate_payload_contains_actual_compiled_outputs_and_fingerprints(self):
        self.export()
        index, expected = MODULE.validate_index(self.out, self.root, "linux-amd64")
        self.assertEqual([p["crate"] for p in index["packages"]], ["alpha", "beta", "gamma"])
        self.assertEqual(len({p["image"] for p in index["packages"]}), 3)
        self.assertEqual(len({p["tag"] for p in index["packages"]}), 3)
        fresh = self.base / "empty-target"
        mirror = self.base / "source-mirror"
        count = MODULE.read_package(self.out / "alpha.tar.gz", expected["alpha"], fresh, self.root, mirror)
        self.assertEqual(count, 4)
        self.assertEqual((fresh / "debug/deps/libalpha-1111111111111111.rlib").read_bytes(), b"compiled-alpha")
        self.assertTrue((fresh / "debug/.fingerprint/alpha-1111111111111111/lib-alpha.json").exists())
        self.assertLess((mirror / "crates/alpha/src/lib.rs").stat().st_mtime,
                        (fresh / "debug/deps/libalpha-1111111111111111.rlib").stat().st_mtime)

    def test_wrong_compiler_abi_or_source_is_rejected(self):
        self.export()
        plan = self.plan()["alpha"]
        self.write("crates/alpha/src/lib.rs", "pub fn modified() {}")
        with self.assertRaisesRegex(ValueError, "identity mismatch"):
            MODULE.read_package(self.out / "alpha.tar.gz", self.plan()["alpha"])
        with self.assertRaisesRegex(ValueError, "identity mismatch"):
            MODULE.read_package(self.out / "alpha.tar.gz",
                MODULE.plans(self.root, "linux-amd64", "b" * 64)["alpha"])
        self.assertNotEqual(plan["identity"], self.plan()["alpha"]["identity"])

    def test_reject_archive_path_escape_before_any_target_write(self):
        self.export()
        path = self.out / "alpha.tar.gz"
        with tarfile.open(path, "r:gz") as original:
            metadata = original.extractfile("package.json").read()
        with path.open("wb") as raw:
            with gzip.GzipFile(fileobj=raw, mode="wb", mtime=0) as z:
                with tarfile.open(fileobj=z, mode="w") as tar:
                    MODULE.tar_add_bytes(tar, "package.json", metadata)
                    MODULE.tar_add_bytes(tar, "target/../../unsafe", b"bad")
        target = self.base / "restored"
        with self.assertRaisesRegex(ValueError, "Unsafe|Unexpected"):
            MODULE.read_package(path, self.plan()["alpha"], target, self.root)
        self.assertFalse(target.exists())

    def test_untrusted_run_provenance_rejected(self):
        self.export()
        with self.assertRaisesRegex(ValueError, "originating CI build context|source commit provenance"):
            MODULE.validate_index(self.out, self.root, "linux-amd64", "999", "a" * 40, "pull_request")

    def test_stale_target_must_not_be_published_for_new_source(self):
        future = time.time() + 3600
        self.write("crates/alpha/src/lib.rs", "pub fn alpha() { panic!() }\n")
        os.utime(self.root / "crates/alpha/src/lib.rs", (future, future))
        self.assertEqual(MODULE.crate_outputs(self.target, "alpha", self.root,
                                             self.plan()["alpha"]["source"]), [])
        self.assertTrue(MODULE.crate_outputs(self.target, "gamma", self.root,
                                            self.plan()["gamma"]["source"]))

    @unittest.skipUnless(shutil.which("cargo"), "Cargo toolchain unavailable")
    def test_real_cargo_reuses_published_rlibs_from_empty_target(self):
        """Acceptance proof: a second Cargo invocation must report all Fresh."""
        environment = {**os.environ, "CARGO_TARGET_DIR": str(self.target), "CARGO_INCREMENTAL": "0"}
        first = subprocess.run(["cargo", "build", "--offline", "--workspace", "-v"],
                               cwd=self.root, env=environment, capture_output=True, text=True)
        self.assertEqual(first.returncode, 0, first.stderr[-3000:])
        self.export()
        index, expected = MODULE.validate_index(self.out, self.root, "linux-amd64")
        self.assertEqual(len(index["packages"]), 3)
        # This is a disposable test target. Remove *all* compiled files, then
        # restore only the OCI crate payloads (no cache or registry snapshot).
        shutil.rmtree(self.target)
        for crate, plan in expected.items():
            MODULE.read_package(self.out / f"{crate}.tar.gz", plan, self.target, self.root)
        second = subprocess.run(["cargo", "build", "--offline", "--workspace", "-v"],
                                cwd=self.root, env=environment, capture_output=True, text=True)
        self.assertEqual(second.returncode, 0, second.stderr[-3000:])
        for crate in ("alpha", "beta", "gamma"):
            self.assertIn(f"Fresh {crate}", second.stderr, second.stderr[-2000:])
            self.assertNotIn(f"Compiling {crate}", second.stderr, second.stderr[-2000:])


if __name__ == "__main__":
    unittest.main()
