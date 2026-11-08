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
| `desktop-tests` | Tauri composition crate with the client/server/UI/native shell closure, including `shell/` referenced by `tauri.conf.json`'s `frontendDist` and the root license. Mac-specific validation still runs on Apple Silicon. |
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

The initial hosted desktop check failed because the source-scoped Dagger input
omitted the build-time Tauri `frontendDist: ../shell` path, although the full
checkout and native Mac build both had it. Always account for files read by
build scripts, proc macros and configuration outside a crate's Rust imports.

## Caching on a fresh hosted runner

### Published, independently reusable compiled crate binaries

Every passing Linux/Dagger check and Apple Silicon build exports one archive
**per compiled Rust workspace crate**. A trusted `workflow_run` job verifies the
original CI-tested Git tree and publishes each crate's real `.rlib`/`.rmeta`,
test outputs, `.fingerprint` metadata and dep-info as a separate ORAS OCI
artifact in GHCR:

`ghcr.io/iarnvidr/muninn-crate-<crate>:<platform>-<sha256-identity>`

`<crate>` is the workspace package name without `muninn-`. Identities include
the crate source, recursively resolved local dependencies, its **own Cargo.lock
dependency subtree** (rather than the entire lockfile), the target platform,
and the compiler/system ABI (installed Debian packages for Linux; Rust/Xcode/
macOS SDK/deployment target/flags for Apple Silicon). The desktop `muninn`
identity also includes its embedded CI provenance/run, so a previous app binary
cannot be substituted for the current PR's build.

`ci/dagger/check/crate_packages.py` resolves each key before compilation and
pulls matching packages into Cargo's target directory. On Linux it also primes
the Dagger source mirror; on macOS it adjusts **only verified-identical source
mtimes**. This preserves Cargo's native fingerprint decisions. A changed crate
or dependency gets a new key; independent crates retain their keys even after
an unrelated lockfile change. Missing packages compile normally. Linux and
macOS use separate identities; binaries are never reused across platforms.

The publisher has `packages:write` only on the trusted default branch. It
checks the originating workflow commit (including the PR merge tree), source
hashes, archive identity and every output digest. It checks out PR source **as
data only** and executes only the trusted main-branch publisher. Existing
package tags are never overwritten; failed workflows cannot publish binaries.
These packages are compilation inputs, **not** green check receipts or app
releases.

The `test_real_cargo_reuses_published_rlibs_from_empty_target` acceptance test
compiles three real crates, deletes all target output, restores only their
published-style packages and checks Cargo reports `Fresh` for each. Other
tests check dependency invalidation and malicious archive rejection.

### First-class compiled crate packages and fallback diagnostics

GitHub-hosted runners have fresh engines, so a named Dagger `CacheVolume` does not
automatically persist across jobs. The primary reusable first-party build
products are **independently published GHCR crate packages**, not an aggregate
target snapshot.

1. Before compilation, the workflow resolves the exact Linux compiler/system ABI
   using Dagger, then imports matching GHCR crate packages into the Cargo target
   directory. For a given source/dependency closure, unchanged crates keep the
   same OCI identity; changed crates and their dependants miss and rebuild.
   Cargo independently verifies each restored fingerprint.
2. The Actions cache and earlier `muninn-ci-cache` snapshots can still seed
   third-party downloads and cold starts. They are optional optimizations; a
   cache miss must not prevent separately published crate packages being read.
3. `cacheVolume(..., source: cargoSeed, sharing: LOCKED)` initializes the
   engine's Rust volume from that seed. `dagger check` executes per-crate checks,
   synchronizing source into `/cache/worktree` without wiping unchanged
   unrelated crates. The tracked source manifest identifies genuine deletions.
4. **On successful checks**, `export-crate-packages` mounts that same volume
   directly inside Dagger. It exports each crate's compiled fingerprints,
   `.rlib`/`.rmeta` and other real output as a separate compressed payload.
   Only those payloads (about a few hundred MB in the initial hosted run) cross
   the Dagger/host boundary. The workflow retains them as `muninn-crates-linux-*`;
   a trusted default-branch `workflow_run` job verifies source-tree identity,
   archive digests and original CI result before publishing immutable GHCR tags.
5. **Only after failed checks**, `export-cargo-cache` optionally copies the full
   multi-GB volume to preserve failing diagnostic state. The failure path can
   archive its raw data as `tar.zst`, upload it, and save the Actions cache for a
   subsequent debugging attempt. This cannot turn a failing check green.
   Stale check logs are separated from reusable Cargo data.

Neither a package download nor a cache hit is itself a test receipt: all checks
still execute for each PR/commit and Cargo decides whether any compiled unit
must be rebuilt.

The Linux Tauri check also writes full Cargo stdout/stderr to
`/cache/check-logs/desktop-tests.log` under the locked cache mount. The exporter
includes that file, then the host moves it out of the **reusable compilation
cache** before archive/save, to avoid presenting a previous run's error as
current evidence. A failure-only workflow step prints its last 200 lines and
uploads the small log as its own artifact. The diagnostic preserves Cargo's
failure status and gives an error report even when the Dagger Actions summary
shows only `desktop-tests: exit code 101`. The log is not a passing receipt.

Cache transfer cost is real: hosted run #191 exported approximately 4.8 GB and
uploaded a 1.21 GB compressed snapshot. Compare subsequent warm-run compilation
savings with download, extraction, compression and upload time before changing
retention or cache granularity.

The registry packages are:

| GitHub Package | Stored data | Reusable tags |
| --- | --- | --- |
| `ghcr.io/iarnvidr/muninn-crate-<crate>` | Individually compiled crate output and verified fingerprints, isolated per Linux/Apple Silicon ABI | Immutable `<platform>-<source/dependency-identity>` per crate |
| `ghcr.io/iarnvidr/muninn-ci-cache` | Linux Cargo registry, compiled target, stable source mirror | Exact `run-ID-attempt`, source `sha-<commit>`, moving `pr-<number>` for same-PR reuse, `main` only after a green default-branch check |
| `ghcr.io/iarnvidr/muninn-macos-candidate` | The ad-hoc signed Apple Silicon ZIP/DMG, native candidate archive/receipt, and native smoke evidence (not a release) | Exact `run-ID-attempt`, source `sha-<commit>`, `pr-<number>` |
| `ghcr.io/iarnvidr/muninn-check-evidence` | Successful Dagger `check-candidate.json` receipt; **must still be verified** against the exact Git tree using existing promotion policy | Exact `run-ID-attempt`, source `sha-<commit>`, `pr-<number>` |

The aggregate packages link to the repository via OCI labels. GitHub's Package access
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
docker cp "$id:/cache/snapshot.tar.zst" ./muninn-linux-cargo-snapshot.tar.zst
docker rm "$id"
tar --use-compress-program='zstd -d -T2' -xf muninn-linux-cargo-snapshot.tar.zst -C ci/cache/linux-cargo
dagger check  # must still test! Stored compilation != successful evidence.
```

For a macOS test build, pull the `muninn-macos-candidate` image and copy
`/artifacts/.` instead; it remains a non-notarized, non-production candidate.

The hosted key has compiler/container configuration, dependency-manifest and source parts.
Restore prefixes allow unchanged dependencies to survive a lockfile change and
include a final platform-only prefix so edits to the Dagger check implementation
can still seed validated Cargo artifacts from an older check graph. Cargo and
the native ABI-partitioned target decide what can actually be reused. The
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

The old aggregate cache remains an optional third-party dependency seed. Each
compiled crate package has its own GHCR reference and is independently restored
when available. Missing packages rebuild naturally. Dagger checks retain
unrelated source files and delete only paths absent from the complete checkout
manifest. GitHub Packages stores compilation products—not authority to skip
tests or bypass the merge gate.

```sh
# Optional when moving a successful local run's cache. A new nonce is required.
# Ordinary local dagger check runs do not need this; their engine retains it.
dagger -m ci/dagger/check api call export-cargo-cache --nonce local-snapshot-1 \
  --output ci/cache/linux-cargo
```

The action's branch scoping keeps legacy snapshots separate from trusted
default-branch caches; immutable crate-package identities are independent of PR numbers. No signing
secrets, Cargo credentials, private runner or remote engine
endpoint is passed into public PR checks. Local and CI use the same graph, but
different engines do not share result caches unless configured to do so; this
repository does not silently enroll in Dagger Cloud. Checks also run on pushes to `main` to establish trusted compiled crate
packages. The legacy `:main` aggregate cache remains an optional fallback,
not the primary publishing mechanism. Cache misses and eviction remain safe.

## Native macOS and provenance

macOS frameworks, code signing, DMG mounting and the existing UI/updater smokes
still run on the GitHub-hosted Apple Silicon runner. Linux Dagger checks cannot
replace those tests. The macOS workflow caches Cargo registry/git data and `target`
separately, keyed on Rust, Xcode/macOS SDK, target OS/architecture, deployment target
and relevant flags. Both release and debug steps use deployment target `14.0`.
This native cache does not use the Linux checksum mirror; native packages prime
verified-identical source timestamps instead. Missing per-crate packages can
still cause local crate recompilation, and no cross-platform binary reuse is
claimed.

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
