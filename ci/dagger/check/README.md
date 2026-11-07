# Muninn checks

The root `dagger.toml` installs this native Dang module. It requires Dagger
`1.0.0-beta.25`; the CLI/engine version is the same in hosted CI and the module
manifest. No Go SDK, generated client, external module or cloud account is needed.
Every container image is pinned by digest in `main.dang`. There are no floating
image tags or external module references for a `dagger.lock` to resolve. If a
future change adds any, resolve and commit the generated lockfile with that change.

Run from the repository root with the pinned Dagger CLI and a Linux container
runtime available:

```sh
dagger check -l
dagger check
# Run just one input-scoped check while iterating:
dagger check dag://muninn-ci/core-tests
dagger check dag://muninn-ci/javascript
```

The four first-class checks are independent nodes, not one shell script hidden
behind `dagger call`:

| Check | Inputs and responsibility |
| --- | --- |
| `core-tests` | Headless crates and their dependency closure; also test domain without the SQLite feature. Cargo reads the real manifests/entrypoints of the other workspace members without building them. UI files are not an input. |
| `desktop-tests` | Server, UI assets, CLI and Tauri composition; these include the actual UI and native shell inputs. |
| `javascript` | The existing injected-page, UI and native-player JavaScript tests. Rust-only edits do not invalidate this container. |
| `packaging` | Python packaging, architecture and release/provenance tests, plus updater protocol tests. Workflow and build-script inputs are intentional; those tests inspect them. |

The Cargo commands use `--locked`, stable `/cache/worktree` and `/cache/target`
paths, a pinned Rust toolchain/container base, and the shared compiled-artifact
volume. Because Debian
package repositories can advance independently of the pinned image, the compiled
target subdirectory is keyed by the installed system-package fingerprint; downloaded
crates remain reusable across that change. The two Cargo
checks lock that volume for the entire source synchronization and Cargo invocation;
each uses at most two compiler jobs. JavaScript and
packaging checks can run alongside them. Checks fail normally on nonzero exit;
there is no changed-path script that can silently omit a required check. The
selected package lists together cover every workspace package and must be updated
when adding a crate. `build/test_architecture_contract.py` protects the crate graph.

## Caching on a fresh hosted runner

A named Dagger `CacheVolume` persists on **one engine**, not magically across new
GitHub-hosted VMs. `.github/workflows/check.yml` therefore bridges the engine volume
to GitHub Actions' normal cache:

1. The pinned cache action restores downloaded crates, the content-checked source
   mirror and compiled outputs into
   `ci/cache/linux-cargo`. Its tracked `.gitignore` makes an empty seed valid locally.
2. `cacheVolume(..., source: cargoSeed, sharing: LOCKED)` initializes the volume
   from that seed. Tests and export mount the same object: source and sharing are
   part of the v1 cache identity. A long-lived engine with the same seed retains
   its existing volume; a changed seed creates a new volume from those artifacts.
3. `dagger check` runs the checks. For Rust, `run-rust.sh` uses checksum-based
   `rsync` from the immutable `/input` snapshot to `/cache/worktree`, deliberately
   without preserving input timestamps. Unchanged bytes retain their prior
   timestamps; changed bytes get current timestamps and deleted files disappear.
   Synchronization and Cargo run inside one locked exec. No cache entry is itself
   a check result.
4. `export-cargo-cache` copies Cargo registry/git data, the source mirror and
   compiled `target` artifacts (preserving timestamps) from the volume into a
   normal Dagger directory, then exports that to
   the seed directory. A fresh run/attempt nonce invalidates **only the copy**, so
   cached Dagger operations cannot return an older snapshot of the mutable volume.
5. The cache action saves the successful job's updated data for later hosted runs.

The hosted key has compiler/container configuration, dependency-manifest and source parts.
Restore prefixes allow unchanged dependencies to survive a lockfile change. The
engine's volume name does not include a commit SHA or lockfile hash. Compiled
artifacts are still validated by Cargo; changed source, compiler flags, features,
target ABI or dependencies can legitimately rebuild a crate. The source mirror
avoids fresh-checkout timestamps forcing every local crate to rebuild, without
trusting normalized source timestamps that could hide actual changes. Since
stable Cargo uses mtimes, a restored target with future timestamps fails closed
with a clear fresh-cache instruction. Source symlinks are rejected rather than
following hidden out-of-tree inputs. The real offline Cargo fixture in
`test_cargo_cache.py` tests cold/warm builds, timestamp-only input changes,
same-size changed content, deleted sources and both safety guards.

The cache is an optimization, not a promise that every unchanged crate will
always be reused. The two Rust scopes share a worktree: switching from the broad
desktop scope to the smaller core scope removes out-of-scope files, and switching
back restores them. Adapter crates may rebuild then; unchanged core files retain
their timestamps. An empty cache remains a normal cold build, not a correctness
failure.

```sh
# Optional when moving a successful local run's cache. A new nonce is required.
# Ordinary local dagger check runs do not need this; their engine retains it.
dagger api call muninn-ci export-cargo-cache --nonce local-snapshot-1 \
  export --path ci/cache/linux-cargo
```

The action's branch scoping keeps PR caches separate from trusted default-branch
caches. No signing secrets, Cargo credentials, private runner or remote engine
endpoint is passed into public PR checks. Local and CI use the same graph, but
different engines do not share result caches unless configured to do so; this
repository does not silently enroll in Dagger Cloud. The current workflow still
runs only on PRs/manual dispatch, so a default-branch cache is populated only by a
successful manual run there. Cache misses and eviction must remain safe.

## Native macOS and provenance

macOS frameworks, code signing, DMG mounting and the existing UI/updater smokes
still run on the GitHub-hosted Apple Silicon runner. Linux Dagger checks cannot
replace those tests. The macOS workflow caches Cargo registry/git data and `target`
separately, keyed on Rust, Xcode/macOS SDK, target OS/architecture, deployment target
and relevant flags. Both release and debug steps use deployment target `14.0`.
This native cache does not use the Linux checksum mirror: a fresh native checkout
can still make Cargo rebuild local workspace crates, while preserving dependency
compilation. No cross-platform compiler-cache reuse is claimed.

`Checks / test` and `Apple Silicon build / package` retain their workflow/job names.
Check receipts, native candidate artifacts, exact source-tree verification and
production promotion remain separate from build caches. The source-history secret
scan and Git-dependent publication-boundary check run on the host checkout. Nothing
here publishes a release, installs an app or changes a running user's defaults.

## Validation before merge

The hosted PR must pass all four Dagger checks, the publication boundary check and
the unchanged native smoke suite. Capture a cold and warm hosted run and a
UI-only/core-only edit when checking cache effectiveness; inspect Cargo's actual
compiled crates, not just elapsed wall time. A local static test of this file is
not evidence that the Dagger engine executed it. Run `dagger check -l` with the
pinned beta before declaring this migration verified.
