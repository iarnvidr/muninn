# CI action policy

The Linux check uses the official Dagger GitHub Action at the reviewed immutable revision
`dagger/dagger-for-github@496f1b3d8b0d823834c13e67cf8a8e08ca3b9602` and pins Dagger CLI
version `1.0.0-beta.25` in `.github/workflows/check.yml`. It runs `dagger check`, using
the native Dang module declared by the root `dagger.toml` and
`ci/dagger/check/dagger-module.toml`. The matching engine requirement is
`v1.0.0-beta.25`. This is an explicit prerelease pin, not a floating `latest`.

The repository Actions allow-list must include that exact action revision. If the action revision
changes, update the allow-list to the new reviewed full commit SHA before relying on the hosted
check. A denied action appears as a startup failure before any job steps run; it is not evidence
that the Linux checks passed or failed.

The Linux Dagger graph has one first-class `@check` for **each of the thirteen
Rust workspace crates**, with dependency-scoped sources. JavaScript and
packaging checks remain independent. A failing crate fails the job, while
the other crate results are still individually reported.

Both Linux and macOS Cargo caches use the already-reviewed
`actions/cache@55cc8345863c7cc4c66a329aec7e433d2d1c52a9` revision. Cache artifacts
are an optimization only. Linux exports/seeds its Dagger volume through that
action because each GitHub-hosted job has a new engine. The Linux Cargo execs hold
one shared cache lock while checksum-synchronizing their source mirrors and compiling; the
source mirror and compiler output are transported together, with a fail-closed
clock-skew guard. Native macOS uses separate
compiler/SDK/architecture cache keys. Neither path supplies signing credentials or
connects public pull-request code to a private machine, persistent engine or cloud
account. Caches from a pull request stay within GitHub's normal PR cache scope.
The Linux workflow exports compiled data even when an individual crate check
fails and retains it as a data-only Actions artifact. A separate `workflow_run`
publisher on the trusted default branch copies it to GitHub Packages (GHCR)
using `packages:write`—PR code never has that permission. The publisher never
checks out or runs PR scripts, and rejects fork-originating runs. Dagger's seed
can import the same PR's `pr-N` cache image or the latest green `main` image.
macOS candidate ZIP/DMG packages are separate from Linux Cargo caches and from
production releases. OCI cache tags never mean a check or release passed.

Keep the workflow/job identities (`Checks` / `test`, `Apple Silicon build` /
`package`), check receipt path (`check-candidate.json`), retained candidate names,
exact source-tree validation and release promotion policy intact when changing
build orchestration. A restored compiler cache is not a successful check receipt,
and a cached result must never stand in for a native smoke that did not run.

See [the module README](../ci/dagger/check/README.md) for the input graph, hosted
cache transport and required validation before merge.
