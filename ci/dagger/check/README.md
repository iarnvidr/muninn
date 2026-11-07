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
dagger check -l --all
dagger check
# Run just one input-scoped check while iterating:
dagger check --check=domain-tests
dagger check --check=server-tests
dagger check dag://muninn-ci/javascript
```

Fifteen first-class checks are independent nodes, not one shell script hidden
behind `dagger call`: **one per Rust workspace crate**, plus JavaScript and
packaging. Each Rust check invokes `cargo test --locked -p` for one package;
`muninn-domain` also runs with SQLite disabled.

| Check | Inputs and responsibility |
| --- | --- |
| `domain-tests`, `policy-tests`, `capture-tests`, `storage-tests`, `media-tests` | Each headless crate with its transitive Rust source inputs, not the entire workspace. |
| `navigation-tests`, `protocol-tests`, `client-tests`, `core-tests` | Additional crate-level tests; UI changes cannot invalidate these source scopes. |
| `ui-assets-tests`, `server-tests`, `cli-tests` | UI resource crate, embedded HTTP adapter, and CLI including its integration-test server dependency. |
| `desktop-tests` | Tauri composition crate with the client/server/UI/native shell closure. Mac-specific validation still runs on Apple Silicon. |
| `javascript` | The existing injected-page, UI and native-player JavaScript tests. Rust-only edits do not invalidate this container. |
| `packaging` | Python packaging, architecture and release/provenance tests, plus updater protocol tests. Workflow and build-script inputs are intentional; those tests inspect them. |

The Cargo commands use `--locked`, stable `/cache/worktree` and `/cache/target`
paths, a pinned Rust toolchain/container base, and the shared compiled-artifact
volume. Because Debian
package repositories can advance independently of the pinned image, the compiled
target subdirectory is keyed by the installed system-package fingerprint; downloaded
crates remain reusable across that change. All thirteen Cargo
checks lock that volume for the entire source synchronization and Cargo invocation;
each uses at most two compiler jobs. JavaScript and
packaging checks can run alongside them. Checks fail normally on nonzero exit;
there is no changed-path script that can silently omit a required check. The
selected packages cover every workspace package and must be updated when adding
a crate. Each source scope includes real workspace metadata/entrypoints and the
crate's transitive path dependencies; changing `capture` doesn't invalidate
`policy-tests`, and changing UI doesn't invalidate the headless crates.
`build/test_architecture_contract.py` protects the crate graph;
`test_contract.py` protects the Dagger checks and source scopes.

## Caching on a fresh hosted runner

A named Dagger `CacheVolume` persists on **one engine**, not magically across new
GitHub-hosted VMs. `.github/workflows/check.yml` bridges the engine volume
to GitHub Actions' normal cache and GitHub Packages' OCI container registry:

1. The pinned cache action restores downloaded crates, the source mirror and
   compiled outputs into `ci/cache/linux-cargo`. If no files were restored,
   `restore-ghcr-cache.sh` tries the same PR's `:pr-N` cache package and then
   the latest passing main cache package (`:main`). The script never overlays
   one source mirror/target pair with another. A registry miss is a cold build.
   The tracked `.gitignore` makes an empty seed valid locally.
2. `cacheVolume(..., source: cargoSeed, sharing: LOCKED)` initializes the volume
   from that seed. Tests and export mount the same object: source and sharing are
   part of the v1 cache identity. A long-lived engine with the same seed retains
   its existing volume; a changed seed creates a new volume from those artifacts.
3. `dagger check` runs each crate check independently. For Rust, `run-rust.sh` uses checksum-based
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
5. **Even on test failure**, the workflow exports and uploads the compilation
   snapshot *before* enforcing the failed check result. A privileged, separate
   `workflow_run` publisher (running only trusted default-branch code, never a
   PR checkout) copies the snapshot as inert data into GHCR. It does not run
   code from the artifact or turn a failed test into a passing check. Fork PR
   runs are excluded.
6. Successful jobs can additionally save their updated `actions/cache` entries.

The registry packages are:

| GitHub Package | Stored data | Reusable tags |
| --- | --- | --- |
| `ghcr.io/iarnvidr/muninn-ci-cache` | Linux Cargo registry, compiled target, stable source mirror | Exact `run-ID-attempt`, source `sha-<commit>`, moving `pr-<number>` for same-PR reuse, `main` only after a green default-branch check |
| `ghcr.io/iarnvidr/muninn-macos-candidate` | The ad-hoc signed Apple Silicon ZIP/DMG, native candidate archive/receipt, and native smoke evidence (not a release) | Exact `run-ID-attempt`, source `sha-<commit>`, `pr-<number>` |
| `ghcr.io/iarnvidr/muninn-check-evidence` | Successful Dagger `check-candidate.json` receipt; **must still be verified** against the exact Git tree using existing promotion policy | Exact `run-ID-attempt`, source `sha-<commit>`, `pr-<number>` |

Both packages link to the repository via OCI labels. GitHub's Package access
settings must grant the repository Actions read/write access. The publisher
workflow must be present on the **default branch** before `workflow_run` fires:
this PR can upload snapshots while unmerged, but GHCR publication starts after
the publisher lands on `main`.

Use the run-ID/attempt tag for the exact build; a PR's `sha-` tag alone does
not attest to which moving merge base was tested. To load an exact cache:

```sh
image="ghcr.io/iarnvidr/muninn-ci-cache:run-42417777767-2"  # example run/attempt
docker pull "$image"
id=$(docker create --entrypoint /bin/true "$image")
docker cp "$id:/cache/." ci/cache/linux-cargo/
docker rm "$id"
dagger check  # must still test! Stored compilation != successful evidence.
```

For a macOS test build, pull the `muninn-macos-candidate` image and copy
`/artifacts/.` instead; it remains a non-notarized, non-production candidate.

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
always be reused. All Rust scopes share a worktree: switching scopes removes
out-of-scope files and switching back restores them. Adapter crates may rebuild;
unchanged shared source bytes retain their timestamps. Each hosted engine is
new, so its previous *Dagger check-result* cache isn't in GHCR. GHCR contains
Cargo-validated compilation products, not permission to skip a test.

```sh
# Optional when moving a successful local run's cache. A new nonce is required.
# Ordinary local dagger check runs do not need this; their engine retains it.
dagger api call muninn-ci export-cargo-cache --nonce local-snapshot-1 \
  export --path ci/cache/linux-cargo
```

The action's normal branch scoping keeps PR caches separate from trusted
default-branch caches; GHCR's moving PR tag stays within one PR. No signing
secrets, Cargo credentials, private runner or remote engine
endpoint is passed into public PR checks. Local and CI use the same graph, but
different engines do not share result caches unless configured to do so; this
repository does not silently enroll in Dagger Cloud. Checks now also run on
pushes to `main` to populate the trusted `:main` cache image after success.
Cache misses and eviction must remain safe.

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

The hosted PR must pass all fifteen Dagger checks, the publication boundary check and
the unchanged native smoke suite. Capture a cold and warm hosted run and a
UI-only/core-only edit when checking cache effectiveness; inspect Cargo's actual
compiled crates, not just elapsed wall time. A local static test of this file is
not evidence that the Dagger engine executed it. Run `dagger check -l` with the
pinned beta before declaring this migration verified.
