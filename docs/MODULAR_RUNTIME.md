# Modular runtime: implementation and acceptance

This change rebuilds the existing Muninn app around explicit Rust compilation and
client/server boundaries. It does not replace its UI, require a server, change
download/cleanup defaults, introduce a remote endpoint, or ship an iOS PWA.

The desktop remains the default: one Muninn application process composes the
client, the backend and the loopback HTTP service. Native WebKit and bundled
media helpers retain their existing process model. Crates are build boundaries,
not a requirement to run a collection of microservices.

For capture/TLS routing, authentication, playback and database details, see
[Architecture](ARCHITECTURE.md). For the exact Dagger graph and cache setup, see
[Muninn checks](../ci/dagger/check/README.md).

## The product contract stays the same

| Behaviour | Contract for this rebuild |
| --- | --- |
| Playback | **Always device-local cached bytes**, from a completed media file or the already-written segments of an in-progress HLS cache. Never upstream streaming or a remote-server URL passed to the player. |
| Open a video in a background tab | Request acquisition without taking focus or autoplaying. Deduplicate an already-open destination rather than creating another tab/job. |
| Browse feeds, restore tabs, import a tab collection | Keep existing lightweight browsing/restoration/import semantics; do not download every listed or restored video. |
| Keep in the player | Protect the current copy without pausing playback, replacing its source, changing quality or downloading another copy. |
| After watching | Preserve the existing completion policy: by default 95% completion (or ended), followed by a 24-hour cleanup grace period. Respect Keep, active playback/downloads and existing user overrides. |
| Rules and configuration | Existing values and defaults remain authoritative. Ordinary use does not acquire a new rules/setup step. |
| Desktop UX | Preserve tabs, feeds, search, player, prompts, sign-in, keyboard behaviour and layout. New remote/PWA UI must not displace these defaults. |
| Existing installation | Preserve the app identifier, data-directory layout, database schema, local API routes and installed `--cli` entry point. |

The background-watch acquisition hook is an explicit client intent. It is not a
change from hidden tabs to active tabs, and it is not a new rule that prefetches
all sleeping tabs. Existing active-watch handling still drives foreground
playback. Acquisition calls that may read native cookies run off the main thread.

Background intents are durable queue entries, admitted by the existing scheduler
into the same growing HLS/cache pipeline. They respect normal download concurrency
and the HLS stream limit without preempting other work; overflow stays queued.
Selecting an admitted tab joins that acquisition, not a second download. A queued
intent keeps its chosen quality if the global preference changes before admission.

## Crate ownership

Every package uses the workspace version and lockfile. The workspace retains
resolver `3` and the pinned [Rust toolchain](../rust-toolchain.toml).

| Package | Owns | Boundary |
| --- | --- | --- |
| [`muninn-domain`](../crates/domain/) | Shared models, browsing rows, settings, quality, playlist types, errors and existing native utility helpers | No application, UI or adapter dependency. SQLite error conversion is an optional feature, enabled by storage. This is not yet a claim of browser/WASM portability. |
| [`muninn-policy`](../crates/policy/) | Rule matching/admission and eviction decisions | Depends on domain, not storage or the application runtime. |
| [`muninn-capture`](../crates/capture/) | Bounded response parsing, extraction and merge logic | Depends on domain; no HTTP service, native webview or database ownership. |
| [`muninn-storage`](../crates/storage/) | Video, content and browsing SQLite stores and migrations | Depends on domain and policy; no native UI or client dependency. |
| [`muninn-media`](../crates/media/) | yt-dlp/tool helpers, cached-file range serving and stream diagnostics | Does not own application scheduling, tab state or the desktop shell. |
| [`muninn-navigation`](../crates/navigation/) | Shared tab/destination model, URL classification, history and restoration | Production code depends on domain/capture parsing, not a database or native webview. Database-backed restoration tests are development-only dependencies. |
| [`muninn-protocol`](../crates/protocol/) | `WatchRequest`, `WatchBackend` and validated `CacheSource` | Defines the watch boundary without importing the backend implementation or native shell. |
| [`muninn-core`](../core/) | Application orchestration: feeds, capture proxy, cookies/hooks, jobs, cache lifecycle, watch state, subscriptions, search and automation | Uses reusable libraries; does not own HTTP API routing or embedded UI assets, and does not depend on client/server/desktop adapters. |
| [`muninn-client`](../crates/client/) | Client acquisition intents and navigation exports | Uses protocol/domain/navigation, not `Core`, SQLite, the HTTP router or Tauri. |
| [`muninn-server`](../crates/server/) | Existing loopback HTTP routes, authentication, local media/UI serving, embedded backend adapter and service lifetime | Composes core/protocol/UI assets. Also owns the existing `muninn-service` binary. |
| [`muninn-ui-assets`](../crates/ui-assets/) | Compile-time embedding of the existing HTML, JS and CSS | An asset-only package. Editing UI bytes no longer recompiles the core just because it embeds them. |
| [`muninn-cli`](../crates/cli/) | Existing command mapping and loopback API client | Keeps the core dependency for read-only offline diagnostics. The desktop `--cli` path calls the same library; it does not start a second backend. |
| [`muninn`](../src-tauri/) | Tauri composition root, native webviews, menus, trust, IPC, windows and platform hooks | Constructs the embedded service and client. Native/platform dependencies stay here. |

Existing `muninn_core` module reexports preserve internal call sites for the moved
models and helpers. They do not copy implementations back into the core. The
stateful `impl Core` operations deliberately remain together: this change does
not replace the download lifecycle with a second implementation merely to create
more crates.

## The first client/server boundary

The shared `WatchBackend` contract expresses opening/preparing a watch, querying
status and keeping an entry. Its first implementation uses the existing local
`Core`. Both native background-open intents and the watch API use that embedded
backend. Other existing UI APIs and the desktop's native cookie/feed/trust hooks
remain in place; this is not a claim that every command has already acquired a
remote transport.

The existing JSON watch response stays wire-compatible. When a response claims
playback readiness, `CacheSource` accepts only the two disk-backed playback path
shapes:

- `/media/{entry_id}` for a complete managed file.
- `/h/{token}/{job}/index.m3u8` for a local HLS cache, which may still be growing.

Absolute network URLs, authority-relative URLs, traversal, unknown kinds and
internal `/s/` downloader-feeder URLs are not valid playback sources. The UI's
[`mediaUrl`](../ui/lib.js) applies the corresponding local-origin/path check at
the player boundary. These guards do not manufacture cached bytes: existing
readiness and file-serving checks still decide when media is available.

The shared types provide a seam for later deployments. They do **not** enable
remote access to today's localhost API, and do not by themselves constitute a
device-cache replica, authenticated remote transport or PWA.

## What the compilation split saves—and does not save

Rust modules within one crate are not independently reusable compilation units.
Separating these responsibilities lets Cargo retain unaffected crate artifacts,
subject to its normal source/dependency/feature/toolchain fingerprints.

| Edit | Direct crate impact | Reuse to inspect |
| --- | --- | --- |
| `ui/` HTML/JS/CSS | `muninn-ui-assets`; its consumers can rebuild/relink | Core and its parsing/storage/policy/media libraries should not compile solely because UI bytes changed. JS checks and UI/native validation still run. |
| Core scheduling or watch logic | `muninn-core`, then dependent adapters/binaries | Independent domain/parser/media/client artifacts can be reused when their own inputs and enabled features are unchanged. |
| Policy logic | `muninn-policy`, then storage/core and downstream consumers | Unrelated capture/media logic should stay reusable. Do not expect storage or the final app to stay untouched when their dependency changed. |
| HTTP adapter only | `muninn-server` and its consumers | Core and client libraries should stay reusable. Embedded desktop linking can still repeat. |
| Client intent only | `muninn-client` and its desktop consumer | The server/core implementation does not acquire a dependency on the client. |
| Shared domain/protocol or Cargo/toolchain flags | All consumers of the changed contract or build configuration may be invalidated | Wider rebuilds are legitimate; do not disguise incompatible artifacts as cache hits. |

An unchanged crate is not guaranteed to stay fresh in every invocation. Changing
feature unification, profile, target, compiler, `RUSTFLAGS`, build scripts or
checkout timestamps can change Cargo's decision. Most importantly, changed
dependencies can invalidate dependent crates, and a final native link can still
be necessary. The existing release profile keeps whole-program LTO; that work
may repeat even when many intermediate crates were reused. No benchmark or
zero-recompilation guarantee is implied by the package layout.

## Dagger and native CI

[The native Dang module](../ci/dagger/check/main.dang), registered by
[`dagger.toml`](../dagger.toml), provides four first-class checks:

1. `core-tests`: headless libraries, client and application core; no UI asset bytes.
2. `desktop-tests`: HTTP/UI adapters, CLI integration tests and Tauri composition.
3. `javascript`: the existing browser/UI/player tests plus cache-only/Keep regressions.
4. `packaging`: packaging, architecture and release/provenance contracts.

Checks have separate source inputs; splitting functions alone would not stop an
entire-workspace mount from invalidating each one. Cargo still needs real
workspace manifests/entrypoints to discover packages, so some manifest/entrypoint
edits can invalidate `core-tests` even when that check does not build the edited
adapter. Dagger node invalidation and individual Cargo crate rebuilding are
different measurements.

The module pins Dagger `1.0.0-beta.25` and container images, uses `--locked`, and
shares a locked, bounded-concurrency Cargo artifact volume. A Dagger cache volume
belongs to its engine. The public hosted workflow therefore restores/exports
Cargo artifacts through Actions cache so a new runner need not start with an
empty compiler cache. macOS has its own native Cargo cache; Linux outputs are
not reused as Apple Silicon binaries. Cache identities exclude a commit SHA from
the persistent engine-volume name, but include the compiler/platform/ABI scope.
See the [check README](../ci/dagger/check/README.md) for exact cache seeding,
export, version pins and commands.

The public project continues to use isolated GitHub-hosted CI. This change does
not enroll PRs in a private/home Dagger engine or expose deployment/signing
credentials. The existing `Checks / test` and `Apple Silicon build / package`
names and native artifact/receipt promotion flow remain the gates. Cache data is
neither an accepted test result nor release evidence for a different source tree.

## Acceptance checklist

This is a checklist to execute and record, **not a report that these checks have
passed**. The PR must include its actual command results and hosted evidence.

### Automated and native evidence

- [ ] Run `dagger check -l` using the pinned beta and verify all four checks are
  discovered; then run `dagger check`. Static checks of the module are not proof
  that the Dagger engine executed it.
- [ ] Run the crate and source ownership guards in
  [`test_architecture_contract.py`](../build/test_architecture_contract.py),
  including a lockfile entry for every workspace package.
- [ ] Run the protocol/client tests, existing backend/media tests, server tests,
  and moved [workflow tests](../crates/cli/tests/workflows.rs). The
  [CLI entry-point tests](../crates/cli/tests/entry_points.rs) must also prove that
  help and ordinary command failures do not initialize another service.
- [ ] Run [cache-only and Keep UI regressions](../ui/test/cache-only.test.js),
  the existing injected-page/UI tests, packaging contracts and updater tests.
- [ ] Pass the hosted native Apple Silicon package job, including the actual
  release app health check and isolated native UI/updater smokes. A Linux pass
  cannot stand in for WebKit, Tauri, macOS signing or native playback coverage.

### Measure build reuse

Use a disposable worktree and dedicated target/cache directory, the same pinned
toolchain, target, package selection, features and flags throughout each series.
Do not clean a user's normal cache to manufacture a cold run. Retain Cargo `-vv`
(`Fresh`/`Compiling`) and `--timings` output as well as Dagger operation cache hits;
elapsed time alone cannot explain why work repeated.

- [ ] **Cold:** use a new isolated engine/target cache and record work performed.
- [ ] **Identical warm:** repeat the same inputs on the same engine. Separately
  test a fresh hosted runner seeded from the previous successful Actions cache.
  Do not conflate engine-result reuse with portable Cargo artifact reuse.
- [ ] **UI-only:** make an isolated UI edit; check that the headless check has no
  UI-byte input and that Cargo does not recompile core merely to embed the UI.
  Asset compilation, server/native relinking and UI tests are legitimate work.
- [ ] **Backend-only:** change a core implementation detail; inspect reuse of
  unchanged leaf/client crates while accepting downstream server/app rebuilds.
- [ ] **Leaf-only:** change policy implementation; inspect policy/storage/core
  invalidation and reuse of unrelated capture/media crates.
- [ ] **Cache unavailable:** remove only the disposable seed or use a new cache
  namespace, then verify a correct cold build. Correctness must not depend on
  previously cached outputs or a private machine.

### Existing Mac experience

Use an isolated profile or a backed-up test copy of existing state. The same
screens and defaults must work without a server-selection or rules-configuration
step. Record the actual Mac/OS/app build used for the check.

- [ ] Open videos in background tabs from Browse and cached feeds. Acquisition
  starts, the current tab keeps focus, no hidden player autoplays, and reopening
  a destination does not create a duplicate tab/download.
- [ ] Activate the tab during acquisition. Playback reads the growing local HLS
  cache; confirm seeking, pause/resume and subsequent complete-file playback.
  If cached bytes are unavailable, show pending/error state—not a remote fallback.
- [ ] Play already-cached media offline, including the existing compatibility
  conversion path; verify metadata/feed restoration separately from media bytes.
- [ ] Reach the existing completion/Keep prompt. Keep the exact playing copy
  without interruption; verify ordinary completion schedules cleanup, Keep
  prevents automatic deletion, and replay/active playback retain their safeguards.
- [ ] Verify tabs/history/scroll/hibernation, playlists, search, library/feeds,
  sign-in/cookie handoff, keyboard actions and window/player layout against the
  current app. Check a custom rule and cleanup override survive unchanged.
- [ ] Verify the installed app's `--cli` entry point and updater/native artifact
  checks; confirm the app still works without any separately deployed backend.

**No cutover or release based only on the refactor compiling.** Keep this change
in the PR until the required hosted/native smokes and the low-effort workflow
parity checks have evidence. Merging, releasing, or enabling a different runtime
mode is a separate decision; this PR does not switch installed users to remote
operation.

## Future remote mode and PWA: a separate implementation

The intended future data path is acquisition on the server, transfer into the
client device's managed cache, then playback from that device cache. Server and
device policies are distinct: server retention does not mean a device has a
copy, and deleting a watched device copy must not silently delete the shared
server copy. Define the scope of Keep, watch progress and retention explicitly
before syncing them.

A later remote implementation needs authenticated/versioned transport, resumable
transfers, stable media identity, device-local availability/readiness, disk-budget
and failure handling, and separate server/device retention. It must not implement
the feature by changing the player's base URL to the remote server. The current
headless loopback binary is not already that secure remote service.

Existing feeds, library metadata and the player are building blocks for a PWA;
reproducing arbitrary YouTube-page interception is not a prerequisite for that
feed-based client. Browser-origin storage and actual iOS storage/eviction,
suspension, background acquisition and media-format behaviour still need their
own tested design. No iOS/native parity or reliable background-download promise
is made here, and no PWA replaces the Mac UX before that evidence exists.

## Local verification recorded for the draft (2026-11-07)

The unmodified base `f1528c7c71cd43eb70c578f5e9f2fa75c1ff9953` passed all 307
core/integration Rust tests before extraction. The following checks then passed
on Linux x86-64 with the pinned Rust/Cargo 1.99.1 toolchain:

| Check | Result |
| --- | --- |
| `cargo test --locked --workspace --exclude muninn` | 327 tests passed; includes all extracted crates, server/CLI, existing workflows, and new background/cache contracts |
| `cargo test --locked -p muninn-domain --no-default-features` | 14 tests passed without enabling SQLite |
| Existing UI, injected-page and native-player JavaScript suite | 100 tests passed |
| Python build/packaging/provenance suite | 99 tests passed |
| Dagger helper tests | 8 real offline Cargo cache tests and 9 static pipeline tests passed |
| Updater protocol suite | 7 tests passed |
| Publication boundary and `git diff --check` | Passed |

A disposable snapshot of the actual client/UI crates was also built through
`run-rust.sh`, using Cargo JSON artifact `fresh` flags rather than elapsed time:
the cold run compiled all six selected workspace crates; the identical warm run
reported all six fresh; changing only an embedded CSS comment rebuilt
`muninn-ui-assets` while domain, capture, protocol, navigation and client stayed
fresh. This was a client/UI subset experiment, not a full native application or
Dagger-engine cache measurement.

**Not yet established by these local results:** execution of the pinned Dagger
engine/module, the native Apple Silicon build and WebKit/packaging/updater smokes,
or full native cross-run cache reuse. The PR remains a draft and the no-cutover
acceptance gates above still apply. No installed app or release was changed.
