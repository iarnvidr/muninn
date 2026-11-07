"""Manifest/source ownership guards for the crate split.

These checks protect compilation boundaries; they are not runtime UI parity,
macOS acceptance, or measurements of Cargo/Dagger cache reuse.
"""
from pathlib import Path
import json
import re
import tomllib
import unittest


ROOT = Path(__file__).resolve().parents[1]
LEAVES = {
    "muninn-domain", "muninn-policy", "muninn-capture", "muninn-storage",
    "muninn-media", "muninn-protocol", "muninn-navigation",
}
REQUIRED = LEAVES | {
    "muninn-core", "muninn-client", "muninn-server", "muninn-ui-assets", "muninn",
}


def read_toml(path):
    return tomllib.loads(path.read_text())


def production_dependencies(manifest):
    """Include target/build edges, but not integration-test-only dependencies."""
    for table in (manifest, *manifest.get("target", {}).values()):
        for key in ("dependencies", "build-dependencies"):
            yield from table.get(key, {}).items()


def rust_block_after(source, marker):
    """Extract one wiring block, ignoring braces inside strings and comments."""
    masked = re.sub(r'"(?:\\.|[^"\\])*"|//[^\n]*|/\*.*?\*/',
                    lambda match: " " * len(match.group()), source, flags=re.DOTALL)
    start = masked.index("{", source.index(marker))
    depth = 1
    for end in range(start + 1, len(masked)):
        depth += (masked[end] == "{") - (masked[end] == "}")
        if depth == 0:
            return source[start + 1:end]
    raise AssertionError(f"unclosed Rust block after {marker}")


class ArchitectureContract(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.workspace = read_toml(ROOT / "Cargo.toml")
        cls.packages = {}
        cls.paths = {}
        for pattern in cls.workspace["workspace"]["members"]:
            for directory in ROOT.glob(pattern):
                manifest = read_toml(directory / "Cargo.toml")
                name = manifest["package"]["name"]
                cls.packages[name] = manifest
                cls.paths[name] = directory
        shared = cls.workspace["workspace"].get("dependencies", {})
        cls.graph = {}
        cls.all_dependencies = {}
        for name, manifest in cls.packages.items():
            dependencies = set()
            for alias, spec in production_dependencies(manifest):
                if isinstance(spec, dict) and spec.get("workspace"):
                    spec = shared[alias]
                package = spec.get("package", alias) if isinstance(spec, dict) else alias
                dependencies.add(package)
            cls.all_dependencies[name] = dependencies
            cls.graph[name] = dependencies & cls.packages.keys()

    def closure(self, name):
        self.assertIn(name, self.graph, f"missing compilation boundary: {name}")
        seen = set()
        todo = [name]
        while todo:
            current = todo.pop()
            if current not in seen:
                seen.add(current)
                todo.extend(self.graph[current] - seen)
        return seen

    def test_responsibilities_are_real_workspace_packages(self):
        self.assertFalse(REQUIRED - self.packages.keys(),
                         f"missing crates: {sorted(REQUIRED - self.packages.keys())}")
        self.assertEqual(self.workspace["workspace"].get("resolver"), "3")

    def test_backend_is_independent_of_client_http_assets_and_desktop(self):
        forbidden = {"muninn-client", "muninn-server", "muninn-ui-assets", "muninn"}
        self.assertFalse(self.closure("muninn-core") & forbidden,
                         "changing a transport/UI must not invalidate backend libraries")

    def test_leaf_libraries_never_depend_back_on_application_adapters(self):
        forbidden = {"muninn-core", "muninn-client", "muninn-server", "muninn-ui-assets", "muninn"}
        for package in LEAVES:
            with self.subTest(package=package):
                self.assertFalse(self.closure(package) & forbidden)
        self.assertEqual(self.closure("muninn-domain"), {"muninn-domain"})

    def test_client_uses_contracts_without_linking_backend_implementation(self):
        self.assertLessEqual(self.closure("muninn-client"),
                             {"muninn-client", "muninn-protocol", "muninn-domain", "muninn-navigation", "muninn-capture"})
        self.assertIn("muninn-protocol", self.graph["muninn-client"])
        self.assertLessEqual(self.closure("muninn-protocol"),
                             {"muninn-protocol", "muninn-domain"})

    def test_http_server_is_an_adapter_not_a_dependency_of_client(self):
        closure = self.closure("muninn-server")
        self.assertTrue({"muninn-core", "muninn-protocol", "muninn-ui-assets"} <= closure)
        self.assertFalse({"muninn-client", "muninn"} & closure)

    def test_desktop_is_the_embedded_composition_root(self):
        self.assertTrue({"muninn-core", "muninn-client", "muninn-server"} <= self.graph["muninn"])
        # Native bindings belong only to the desktop package, never a reusable
        # domain, client, service, or UI-resource library.
        for package, dependencies in self.all_dependencies.items():
            if package == "muninn":
                continue
            with self.subTest(package=package):
                native = {dep for dep in dependencies
                          if dep in {"tauri", "tauri-build", "wry", "webkit2gtk", "cocoa", "objc2-app-kit"}
                          or dep.startswith("tauri-plugin-")}
                self.assertFalse(native, f"native bindings escaped desktop: {native}")

    def test_ui_asset_compilation_is_not_hidden_in_backend_includes(self):
        self.assertEqual(self.closure("muninn-ui-assets"), {"muninn-ui-assets"})
        includes = re.compile(r'include_(?:str|bytes)!\s*\(\s*"([^"]+)"')
        ui = (ROOT / "ui").resolve()
        desktop = (ROOT / "src-tauri").resolve()
        for package in LEAVES | {"muninn-core", "muninn-client", "muninn-protocol"}:
            self.assertIn(package, self.paths)
            for source in self.paths[package].rglob("*.rs"):
                for relative in includes.findall(source.read_text()):
                    target = (source.parent / relative).resolve()
                    with self.subTest(source=source.relative_to(ROOT), target=relative):
                        self.assertFalse(target.is_relative_to(ui) or target.is_relative_to(desktop),
                                         "GUI assets/configuration invalidate this library's compilation")

    def test_native_background_bridge_is_only_for_new_tabs_without_focus_or_playback(self):
        # This is intentionally a source wiring assertion. Client's Rust tests
        # prove intent semantics; actual AppKit focus/cookie behavior still needs
        # the native smoke test before this PR is ready for a release/cutover.
        shell = (ROOT / "src-tauri/src/shell.rs").read_text()
        opening = rust_block_after(shell, "fn open_dest(")
        background = rust_block_after(opening, "if background")
        created = rust_block_after(background, "if created")
        blocking = rust_block_after(created, "spawn_blocking(")
        self.assertIn("client.prepare_background(&loc, created)", blocking)
        self.assertEqual(shell.count(".prepare_background("), 1,
                         "restoration/persistence must not implicitly acquire media")
        self.assertIn('self.persist(&t, "hidden")', created)
        for unwanted in ("self.activate(", "self.create_view(", "ingest_playback("):
            self.assertNotIn(unwanted, background)

    def test_workspace_versions_and_existing_app_identity_are_preserved(self):
        version = self.workspace["workspace"]["package"]["version"]
        config = json.loads((ROOT / "src-tauri/tauri.conf.json").read_text())
        self.assertEqual(config["version"], version)
        self.assertEqual(config["identifier"], "io.github.iarnvidr.muninn")
        self.assertEqual(config["bundle"]["macOS"]["minimumSystemVersion"], "14.0")
        lock = read_toml(ROOT / "Cargo.lock")
        locked = {p["name"]: p for p in lock["package"] if "source" not in p}
        for name, manifest in self.packages.items():
            with self.subTest(package=name):
                self.assertEqual(manifest["package"]["version"], {"workspace": True})
                self.assertTrue(name in locked, f"--locked builds must include workspace crate {name}")
                self.assertEqual(locked[name]["version"], version)


if __name__ == "__main__":
    unittest.main()
