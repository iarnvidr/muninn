# Muninn 0.2.2 test build

This is an Apple Silicon Tauri build. The bundle includes all helpers.

Changes: comment expansion and reply pagination; more player controls; per-copy
quality, retention and deletion; named tab groups; library collections and tags;
manual/automatic/disabled picture-in-picture; signed-in feed refresh; offline
CLI diagnostics packages with instructions for local Codex.

Verified on Linux:

- 191 Rust core unit tests and 13 integration tests, including independent comment
  and reply continuations, organization/CLI operations, feed refresh dispatch,
  credential redaction and offline support export.
- 33 JavaScript tests and 14 packaging tests.
- Live public YouTube: top-level comments advanced from 20 to 40 to 60; replies
  advanced from 10 to 57 without mixing into the top-level comment cache.
- Browser UI against the real core API: saved MP4 playback and retention changes,
  tab group collapse and tags. Manual and automatic PiP continued playing while
  the library was open; closing PiP paused the background video as configured.
- A support ZIP was generated from the real test data directory and inspected:
  report, logs, AGENTS.md and README only, with settings and entry-state counts.

The browser checks use Chromium with a simulated native tab shell. They do not
verify WKWebView PiP or the macOS session. Sign-in is reported working by the
user; authenticated subscription refresh must be tried on that Mac. A refresh
creates/reuses a background tab with the existing browser session and waits for
its captured snapshot; it reports timeout instead of claiming success.

The Mac app is cross-compiled on Linux. Packaging checks validate arm64 helpers,
bundle metadata and ad-hoc signature hashes. It is not notarized, and this new
build has not been launched on a Mac by the developer.

For local troubleshooting, run:

```sh
/Applications/Muninn.app/Contents/MacOS/muninn --cli diagnostics export "$HOME/Desktop/muninn-support.zip"
```

Unzip the package, run Codex in that folder and ask it to read AGENTS.md and
diagnose the report. The export also works after the app closes or crashes.
Review error details/video IDs before sharing the package. Nothing is uploaded.
