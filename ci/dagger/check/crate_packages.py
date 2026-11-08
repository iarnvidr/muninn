#!/usr/bin/env python3
"""Publish and restore *individual compiled workspace crates* as OCI artifacts.

This is deliberately separate from Cargo's registry/download and aggregate target
caches. Each OCI reference identifies one Rust crate, its source/dependency
closure, compiler/SDK ABI and platform. The payload contains that crate's actual
Cargo outputs (.rlib/.rmeta, unit fingerprints and dep-info), not its source
tarball. Cargo remains the final authority on whether a restored unit is fresh.

The trusted workflow_run publisher executes this script from main against a
data-only artifact from a successful CI run. It never executes PR-supplied code.
"""

from __future__ import annotations

import argparse
import gzip
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import tomllib


SCHEMA = 1
REPOSITORY = "ghcr.io/iarnvidr"
ARTIFACT_TYPE = "application/vnd.muninn.compiled-crate.v1"
LAYER_TYPE = "application/vnd.muninn.compiled-crate.tar+gzip"
FINGERPRINT = re.compile(r"^[0-9a-f]{16}$")
PACKAGE_BINARIES = {"muninn": ("muninn",), "muninn-cli": ("muninn-cli",),
                    "muninn-server": ("muninn-service",)}
IGNORED_DIRS = {".git", "target", "gen", "node_modules", "__pycache__", ".venv"}
GLOBAL_FILES = [
    "Cargo.toml", "rust-toolchain.toml", ".cargo",
]
BUILD_CONTEXT_KEYS = ("MUNINN_BUILD_CHANNEL", "MUNINN_BUILD_SHA", "MUNINN_BUILD_PR",
                      "MUNINN_BUILD_PR_HEAD", "MUNINN_BUILD_RUN_ID")


def canonical(obj: object) -> bytes:
    return json.dumps(obj, sort_keys=True, separators=(",", ":")).encode("utf-8")


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def hash_file(path: Path) -> str:
    hasher = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            hasher.update(chunk)
    return hasher.hexdigest()


def relative_files(root: Path, path: Path) -> list[str]:
    if not path.exists():
        return []
    if path.is_symlink():
        raise ValueError(f"Source path is a symlink: {path}")
    if path.is_file():
        return [path.relative_to(root).as_posix()]
    items = []
    for base, dirs, files in os.walk(path):
        dirs[:] = sorted(name for name in dirs if name not in IGNORED_DIRS)
        for name in sorted(files):
            file = Path(base) / name
            if file.is_symlink():
                raise ValueError(f"Source file is a symlink: {file}")
            if file.is_file():
                items.append(file.relative_to(root).as_posix())
    return items


def direct_local_deps(manifest: dict, path: str, names: dict[str, str]) -> set[str]:
    sections = [manifest]
    sections.extend(manifest.get("target", {}).values())
    deps: set[str] = set()
    for section in sections:
        for key in ("dependencies", "dev-dependencies", "build-dependencies"):
            for alias, spec in section.get(key, {}).items():
                if not isinstance(spec, dict) or "path" not in spec:
                    continue
                dep_path = (Path(path) / spec["path"]).as_posix()
                # Normalize '/..' without resolving out of the source tree.
                dep_path = os.path.normpath(dep_path)
                if dep_path not in names:
                    raise ValueError(f"Unknown local dependency {alias}: {dep_path}")
                deps.add(names[dep_path])
    return deps


def workspace(root: Path) -> dict[str, dict]:
    root_manifest = tomllib.loads((root / "Cargo.toml").read_text())
    members = root_manifest["workspace"]["members"]
    paths: set[str] = set()
    for member in members:
        if "*" in member:
            paths.update(p.parent.relative_to(root).as_posix() for p in root.glob(member + "/Cargo.toml"))
        else:
            paths.add(member)
    manifests = {path: tomllib.loads((root / path / "Cargo.toml").read_text()) for path in sorted(paths)}
    names = {path: manifest["package"]["name"] for path, manifest in manifests.items()}
    if len(set(names.values())) != len(names):
        raise ValueError("Duplicate workspace crate names")
    return {
        name: {"path": path, "deps": sorted(direct_local_deps(manifests[path], path, names))}
        for path, name in names.items()
    }


def relevant_lock_packages(root: Path, crate: str) -> list[dict]:
    """Bind only the resolved third-party subtree, not unrelated lock edits.

    Cargo.lock holds the entire workspace, so hashing it wholesale would break
    per-crate reuse whenever another feature adds an unrelated dependency.
    Ambiguous names conservatively include *every* matching lock version.
    """
    lock = tomllib.loads((root / "Cargo.lock").read_text())
    by_name: dict[str, list[dict]] = {}
    for package in lock.get("package", []):
        by_name.setdefault(package["name"], []).append(package)
    pending = [crate]
    seen: set[tuple[str, str, str]] = set()
    selected: list[dict] = []
    while pending:
        spec = pending.pop()
        fields = spec.split()
        options = [p for p in by_name.get(fields[0], []) if len(fields) < 2 or p["version"] == fields[1]]
        for package in options:
            identity = (package["name"], package["version"], package.get("source", ""))
            if identity in seen:
                continue
            seen.add(identity)
            selected.append(package)
            pending.extend(package.get("dependencies", []))
    return sorted(selected, key=lambda p: (p["name"], p["version"], p.get("source", "")))


def plans(root: Path, platform: str, abi: str, context: dict[str, str] | None = None) -> dict[str, dict]:
    if not re.fullmatch(r"[a-z0-9-]+", platform) or not re.fullmatch(r"[a-f0-9]{64}", abi):
        raise ValueError("Platform and compiler/SDK ABI must be normalized, nonempty values")
    crates = workspace(root)
    result: dict[str, dict] = {}
    if context is None:
        context = {key: os.environ.get(key, "") for key in BUILD_CONTEXT_KEYS}
    if set(context) != set(BUILD_CONTEXT_KEYS) or not all(isinstance(v, str) for v in context.values()):
        raise ValueError("Malformed app build context")

    def resolve(crate: str, visiting: frozenset[str] = frozenset()) -> dict:
        if crate in result:
            return result[crate]
        if crate in visiting:
            raise ValueError(f"Cyclic crate dependency: {crate}")
        info = crates[crate]
        dependencies = {dep: resolve(dep, visiting | {crate})["identity"] for dep in info["deps"]}
        # CI orchestration and this publisher do not change Rust crate
        # machine code. Their edits must NOT invalidate every published crate.
        # Compiler/SDK/flags are already bound to the ABI and Cargo validates
        # each concrete mode/feature set inside the imported fingerprint.
        paths = [info["path"], *GLOBAL_FILES]
        if crate in {"muninn-ui-assets", "muninn-server", "muninn"}:
            paths.append("ui")
        if crate == "muninn":
            paths.extend(["shell", "LICENSE", "THIRD_PARTY_NOTICES.md"])
        files = sorted(set(f for path in paths for f in relative_files(root, root / path)))
        source = {path: hash_file(root / path) for path in files}
        identity = digest(canonical({
            "schema": SCHEMA, "crate": crate, "platform": platform, "abi": abi,
            "source": source, "dependencies": dependencies,
            "resolved_packages": relevant_lock_packages(root, crate),
            # The desktop embeds CI provenance into its binary. It must never
            # reuse an app binary from a different PR/run with identical code.
            "build_context": context if crate == "muninn" else {},
        }))
        result[crate] = {
            "crate": crate, "identity": identity, "platform": platform, "abi": abi,
            "source": source, "dependencies": dependencies,
            "image": f"{REPOSITORY}/muninn-crate-{crate.removeprefix('muninn-')}",
            "tag": f"{platform}-{identity}",
        }
        return result[crate]

    for name in sorted(crates):
        resolve(name)
    return result


def crate_outputs(target: Path, crate: str, source_root: Path, source: dict[str, str]) -> list[str]:
    """Select that crate's real Cargo units, including test/feature variants."""
    outputs: set[str] = set()
    for fp_root in target.glob("**/.fingerprint"):
        profile = fp_root.parent
        deps_dir = profile / "deps"
        if not deps_dir.is_dir():
            continue
        for unit in fp_root.iterdir():
            if not unit.is_dir() or not unit.name.startswith(crate + "-"):
                continue
            hash_suffix = unit.name[len(crate) + 1:]
            if not FINGERPRINT.fullmatch(hash_suffix):
                continue
            if not any(f.suffix == ".json" for f in unit.iterdir()):
                continue
            normalized = crate.replace("-", "_")
            unit_names = {normalized, *(name.replace("-", "_") for name in PACKAGE_BINARIES.get(crate, ()))}
            unit_outputs = [
                f for f in deps_dir.glob(f"*-{hash_suffix}*")
                if f.is_file() and any(f.name.startswith(name + "-") or f.name.startswith("lib" + name + "-")
                                       for name in unit_names)
            ]
            if not unit_outputs:
                continue
            # Never publish a stale unit under a *new* source-identity tag.
            # Freshly compiled output is at least as new as every input copied
            # to the Cargo worktree; an old unit in a restored cache isn't.
            compiled = max((p.stat().st_mtime_ns for p in unit.iterdir() if p.is_file()), default=0)
            if any(
                (source_root / path).exists() and (source_root / path).stat().st_mtime_ns > compiled + 2_000_000_000
                for path in source
            ):
                continue
            outputs.update(f.relative_to(target).as_posix() for f in unit_outputs)
            outputs.update(f.relative_to(target).as_posix() for f in unit.rglob("*") if f.is_file())
            for binary in PACKAGE_BINARIES.get(crate, ()):
                executable = profile / binary
                if executable.is_file():
                    outputs.add(executable.relative_to(target).as_posix())
            # Build-script outputs must travel with the fingerprint and .rlib.
            for build in (profile / "build").glob(f"{crate}-{hash_suffix}") if (profile / "build").is_dir() else ():
                outputs.update(f.relative_to(target).as_posix() for f in build.rglob("*") if f.is_file())
    return sorted(outputs)


def tar_add_bytes(tar: tarfile.TarFile, name: str, data: bytes, mtime: float = 0) -> None:
    info = tarfile.TarInfo(name)
    info.size = len(data)
    info.mode = 0o644
    info.mtime = int(mtime)
    if mtime != int(mtime):
        info.pax_headers = {"mtime": f"{mtime:.9f}"}
    tar.addfile(info, io.BytesIO(data))


def save_package(path: Path, target: Path, plan: dict, outputs: list[str]) -> dict:
    # Cargo can place a symlink at target/debug/<binary> pointing to its
    # fingerprinted executable in deps/. Store the actual compiled bytes as a
    # regular archive entry. Never follow a link outside the trusted target.
    resolved = {}
    for rel in outputs:
        file = (target / rel).resolve(strict=True)
        if not file.is_relative_to(target) or not file.is_file():
            raise ValueError(f"Cargo artifact escapes target directory: {rel}")
        resolved[rel] = file
    record = {**plan, "schema": SCHEMA, "outputs": {p: hash_file(src) for p, src in resolved.items()}}
    with path.open("wb") as raw:
        with gzip.GzipFile(fileobj=raw, filename="", mode="wb", mtime=0, compresslevel=1) as compressed:
            with tarfile.open(fileobj=compressed, mode="w", format=tarfile.PAX_FORMAT) as archive:
                tar_add_bytes(archive, "package.json", canonical(record))
                for rel in outputs:
                    src = resolved[rel]
                    # gettarinfo() may encode a repeated inode as a hardlink,
                    # even when `src.resolve()` is a regular file. The package
                    # format intentionally contains only complete regular
                    # files, including hardlinked/symlinked Cargo executables.
                    stat = src.stat()
                    info = tarfile.TarInfo("target/" + rel)
                    info.size = stat.st_size
                    info.mode = stat.st_mode & 0o777
                    info.mtime = stat.st_mtime
                    with src.open("rb") as stream:
                        archive.addfile(info, stream)
    return {"crate": plan["crate"], "archive": path.name, "sha256": hash_file(path),
            "tag": plan["tag"], "image": plan["image"], "outputs": len(outputs)}


def export_packages(args: argparse.Namespace) -> None:
    root, target, source_root, output = (Path(p).resolve() for p in
                                          (args.root, args.target, args.source_root, args.output))
    output.mkdir(parents=True, exist_ok=True)
    index = {"schema": SCHEMA, "platform": args.platform, "abi": args.abi,
             "source_sha": os.environ.get("GITHUB_SHA", ""), "packages": []}
    index["build_context"] = {key: os.environ.get(key, "") for key in BUILD_CONTEXT_KEYS}
    for crate, plan in sorted(plans(root, args.platform, args.abi).items()):
        outputs = crate_outputs(target, crate, source_root, plan["source"])
        if not outputs:
            if args.require_all:
                raise ValueError(f"No fresh compiled Cargo outputs for {crate}")
            continue
        archive = output / (crate + ".tar.gz")
        index["packages"].append(save_package(archive, target, plan, outputs))
    if not index["packages"]:
        raise ValueError("No compiled crates found; refusing empty OCI publication")
    (output / "index.json").write_bytes(canonical(index) + b"\n")
    print(f"Exported {len(index['packages'])} individually compiled crate packages to {output}")


def read_package(archive: Path, plan: dict, target: Path | None = None,
                 source_root: Path | None = None, mirror: Path | None = None) -> int:
    """Validate the whole archive before extracting any bytes from it."""
    with tarfile.open(archive, "r:gz") as tar:
        members = tar.getmembers()
        if not members or members[0].name != "package.json":
            raise ValueError("Missing package metadata")
        metadata = json.load(tar.extractfile(members[0]))  # type: ignore[arg-type]
        if {k: metadata.get(k) for k in plan} != plan or metadata.get("schema") != SCHEMA:
            raise ValueError(f"Source/dependency/compiler identity mismatch in {archive}")
        entries = metadata.get("outputs")
        if not isinstance(entries, dict) or not entries:
            raise ValueError("Compiled crate package has no build outputs")
        if len(entries) > 25000 or len(members) != len(entries) + 1:
            raise ValueError("Unexpected archive entry count")
        seen = set()
        for member in members[1:]:
            if not member.isfile() or member.issym() or member.islnk():
                raise ValueError("Only regular build output files are allowed")
            name = PurePosixPath(member.name)
            # `target/debug/muninn` is a valid top-level Cargo executable;
            # fingerprints and deps have one or more additional path parts.
            if name.is_absolute() or ".." in name.parts or len(name.parts) < 3 or name.parts[0] != "target":
                raise ValueError("Unsafe or non-target archive entry")
            rel = name.relative_to("target").as_posix()
            if rel in seen or rel not in entries or member.size > (1 << 31):
                raise ValueError("Unexpected, repeated or oversized build output")
            seen.add(rel)
            if not re.fullmatch(r"[a-f0-9]{64}", entries[rel]):
                raise ValueError("Malformed digest")
            with tar.extractfile(member) as stream:  # type: ignore[arg-type]
                h = hashlib.sha256()
                for chunk in iter(lambda: stream.read(1024 * 1024), b""):
                    h.update(chunk)
                if h.hexdigest() != entries[rel]:
                    raise ValueError(f"Tampered compiled output {rel}")
        if seen != set(entries):
            raise ValueError("Missing compiled output")
        if target is not None:
            target.mkdir(parents=True, exist_ok=True)
            # The entire archive and its source identity are verified first.
            for member in members[1:]:
                rel = PurePosixPath(member.name).relative_to("target")
                dst = target.joinpath(*rel.parts)
                if any(p.is_symlink() for p in (dst, *dst.parents)):
                    raise ValueError("Refusing to write through a symlink")
                dst.parent.mkdir(parents=True, exist_ok=True)
                with tar.extractfile(member) as stream:  # type: ignore[arg-type]
                    with tempfile.NamedTemporaryFile(dir=dst.parent, delete=False) as temp:
                        tmp_name = temp.name
                        shutil.copyfileobj(stream, temp)
                os.replace(tmp_name, dst)
                os.chmod(dst, member.mode & 0o777)
                os.utime(dst, ns=(int(member.mtime * 1e9), int(member.mtime * 1e9)))
            # Cargo uses source mtimes, not content digests, to check freshness.
            # Prime ONLY source files whose hash matches this package identity.
            # No timestamps are changed for modified crates or missing packages.
            if source_root is not None:
                baseline = min(m.mtime for m in members[1:]) - 120
                for rel, expected in metadata["source"].items():
                    src = source_root / rel
                    if not src.is_file() or hash_file(src) != expected:
                        raise ValueError(f"Source changed during restoration: {rel}")
                    if mirror is None:
                        # Many crates share the root Cargo.toml and .cargo
                        # configuration. A later package must not move that
                        # shared source *forward* past an earlier crate's
                        # restored fingerprint and trigger recompilation.
                        safe_time = min(baseline, src.stat().st_mtime)
                        os.utime(src, (safe_time, safe_time))
                    else:
                        dst = mirror / rel
                        if any(p.is_symlink() for p in (dst, *dst.parents)):
                            raise ValueError("Refusing a source mirror symlink")
                        dst.parent.mkdir(parents=True, exist_ok=True)
                        if not dst.is_file() or hash_file(dst) != expected:
                            shutil.copyfile(src, dst)
                        safe_time = min(baseline, dst.stat().st_mtime)
                        os.utime(dst, (safe_time, safe_time))
    return len(entries)


def validate_index(folder: Path, root: Path, expected_platform: str | None = None,
                   run_id: str | None = None, head_sha: str | None = None,
                   event: str | None = None) -> tuple[dict, dict]:
    index = json.loads((folder / "index.json").read_text())
    if index.get("schema") != SCHEMA or not re.fullmatch(r"[a-f0-9]{64}", index.get("abi", "")):
        raise ValueError("Invalid OCI package index")
    if expected_platform is not None and index["platform"] != expected_platform:
        raise ValueError("Artifact came from an unexpected platform")
    if head_sha is not None:
        source_sha = index.get("source_sha", "")
        if not re.fullmatch(r"[a-f0-9]{40}", source_sha) or not re.fullmatch(r"[a-f0-9]{40}", head_sha):
            raise ValueError("Invalid source commit provenance")
        actual = subprocess.check_output(["git", "-C", str(root), "rev-parse", "HEAD"], text=True).strip()
        if actual != source_sha:
            raise ValueError("Original CI checkout differs from published source")
        if event == "pull_request" and source_sha != head_sha:
            parents = subprocess.check_output(["git", "-C", str(root), "show", "-s", "--format=%P", "HEAD"], text=True).split()
            if len(parents) != 2 or parents[1] != head_sha:
                raise ValueError("PR test merge is unrelated to its claimed head")
        elif source_sha != head_sha:
            raise ValueError("Publisher source is not the originating workflow commit")
    context = index.get("build_context")
    if not isinstance(context, dict) or (run_id is not None and context.get("MUNINN_BUILD_RUN_ID") not in ("", run_id)):
        raise ValueError("Untrusted originating CI build context")
    if context.get("MUNINN_BUILD_SHA") and context["MUNINN_BUILD_SHA"] != index.get("source_sha"):
        raise ValueError("Embedded app source stamp does not match original test checkout")
    if event == "pull_request" and context.get("MUNINN_BUILD_CHANNEL") not in ("", "pr"):
        raise ValueError("PR application incorrectly claims another update channel")
    expected = plans(root, index["platform"], index["abi"], context)
    seen = set()
    for row in index["packages"]:
        crate = row["crate"]
        if crate not in expected or crate in seen:
            raise ValueError("Unknown or duplicate package in index")
        seen.add(crate)
        filename = crate + ".tar.gz"
        if row["archive"] != filename or row["tag"] != expected[crate]["tag"] or row["image"] != expected[crate]["image"]:
            raise ValueError("Untrusted OCI package identity")
        archive = folder / filename
        if hash_file(archive) != row["sha256"]:
            raise ValueError("Archive SHA mismatch")
        if read_package(archive, expected[crate]) != row["outputs"]:
            raise ValueError("Package output count mismatch")
    if seen != set(expected):
        raise ValueError(f"Missing expected workspace crate packages: {sorted(set(expected) - seen)}")
    return index, expected


def publish_packages(args: argparse.Namespace) -> None:
    folder = Path(args.input).resolve()
    index, _ = validate_index(folder, Path(args.root).resolve(), args.platform,
                              args.run_id, args.head_sha, args.event)
    published = 0
    for row in index["packages"]:
        ref = f"{row['image']}:{row['tag']}"
        check = subprocess.run([args.oras, "manifest", "fetch", ref], capture_output=True, text=True)
        if check.returncode == 0:
            print(f"Already published; preserving immutable identity: {ref}")
            continue
        if not re.search(r"404|MANIFEST_UNKNOWN|NAME_UNKNOWN|not found", check.stderr, re.I):
            raise RuntimeError(f"Cannot query {ref}: {check.stderr}")
        subprocess.run([
            args.oras, "push", "--artifact-type", ARTIFACT_TYPE,
            "--annotation", f"org.opencontainers.image.source=https://github.com/iarnvidr/muninn",
            # Validate the archive under the verified input directory and ask
            # ORAS to resolve a relative filename there. ORAS rejects absolute
            # layer paths by design; disabling its path protection is unnecessary.
            ref, f"{row['archive']}:{LAYER_TYPE}",
        ], check=True, cwd=folder)
        published += 1
    print(f"Published {published} new immutable per-crate OCI artifacts ({len(index['packages'])} checked)")


def pull_packages(args: argparse.Namespace) -> None:
    root = Path(args.root).resolve()
    target = Path(args.target).resolve()
    mirror = Path(args.mirror).resolve() if args.mirror else None
    expected = plans(root, args.platform, args.abi)
    restored = 0
    for crate, plan in sorted(expected.items()):
        ref = f"{plan['image']}:{plan['tag']}"
        with tempfile.TemporaryDirectory(prefix="muninn-crate-") as folder:
            command = subprocess.run([args.oras, "pull", "--output", folder, ref], capture_output=True, text=True)
            if command.returncode != 0:
                if re.search(r"404|MANIFEST_UNKNOWN|NAME_UNKNOWN|not found", command.stderr, re.I):
                    print(f"MISS {crate} ({plan['identity'][:12]})")
                    continue
                raise RuntimeError(f"Cannot fetch trusted crate {crate}: {command.stderr}")
            archive = Path(folder) / (crate + ".tar.gz")
            if not archive.is_file():
                raise ValueError(f"Published artifact lacks {archive.name}")
            count = read_package(archive, plan, target, root, mirror)
            print(f"HIT {crate} ({count} compiled outputs)")
            restored += 1
    print(f"Reused {restored}/{len(expected)} independently published Rust crates")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="operation", required=True)
    for name in ("export", "pull", "publish"):
        sub = commands.add_parser(name)
        sub.add_argument("--root", required=True)
        sub.add_argument("--platform", required=True)
        if name in {"export", "pull"}:
            sub.add_argument("--abi", required=True)
            sub.add_argument("--target", required=True)
        if name == "export":
            sub.add_argument("--source-root", required=True)
            sub.add_argument("--output", required=True)
            sub.add_argument("--require-all", action="store_true")
        if name == "pull":
            sub.add_argument("--mirror", help="Dagger's persisted source mirror (Linux only)")
            sub.add_argument("--oras", default="oras")
        if name == "publish":
            sub.add_argument("--input", required=True)
            sub.add_argument("--oras", default="oras")
            sub.add_argument("--run-id", required=True)
            sub.add_argument("--head-sha", required=True)
            sub.add_argument("--event", choices=("pull_request", "push", "workflow_dispatch"), required=True)
    args = parser.parse_args()
    try:
        if args.operation == "export":
            export_packages(args)
        elif args.operation == "publish":
            publish_packages(args)
        else:
            pull_packages(args)
    except (ValueError, OSError, RuntimeError, tarfile.TarError, subprocess.CalledProcessError) as exc:
        parser.exit(1, f"muninn compiled crate artifact error: {exc}\n")


if __name__ == "__main__":
    main()
