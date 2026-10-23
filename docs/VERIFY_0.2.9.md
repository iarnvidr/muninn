# Muninn 0.2.9: outside URL handoff

The app accepts a direct HTTP(S) YouTube URL or the registered
`muninn://open?url=<encoded target>` action. Cold executable arguments, second
instance arguments and macOS opened-URL events use the same parser and dispatcher.
Requests received before the shell exists are queued. The shell retains a handoff
revision until the chrome observes it, so an unloaded chrome or an internal page
does not hide the requested tab. Later polls do not repeatedly steal focus from
Library or Settings. Accepted opens log the tab ID, not the full supplied URL.

Only YouTube destinations are admitted. Invalid short links are rejected because
the normal tab router would otherwise send them back to an external browser.
Other schemes, credentials, foreign/lookalike hosts, unsupported app actions and
ambiguous app parameters are rejected. Direct YouTube arguments use the same
validation as encoded app links. No browser extension or routing configuration
is part of this feature.

## Completed checks

- 224 Rust tests passed, including four parser/normal-router boundary tests.
- 36 JavaScript tests passed.
- 17 packaging tests passed, including the actual URL registration shared by both
  packaging paths and Tauri.
- The production Tauri/WebKitGTK app passed `e2e/app-links.py`: cold executable
  launch with an app link, a second process delivering an app link to the running
  instance, direct-URL delivery while the window is hidden, restoration of that
  window, leaving Library, rejection without a new tab, and consuming the focus
  request once.
- Static shell/workflow checks and a redacted source secret scan passed.
- The final Apple Silicon cross-build passed bundle metadata, helper architecture,
  signature page-hash and resource-seal verification. Its 111,380,283-byte ZIP
  passed SHA-256 verification. Inspection of the packaged Info.plist confirms
  version 0.2.9, the unchanged application identifier and the `muninn` URL scheme.

The Linux regression uses the actual executable, native window and production UI,
not a standalone mock page. Its disposable test HOME contains no user account data.

## Platform limit

Linux argv/single-instance results do not verify macOS Launch Services. The
installed Mac application still needs the cold/running/hidden-window checks in
[APP_LINKS.md](APP_LINKS.md). Cross-compilation and bundle registration checks
cannot substitute for that native run. Hosted Mac CI remains blocked by the
account's billing/spending limit.

This feature does not establish a fix for the outstanding macOS playback report,
change import behavior, add timestamp seeking, or make the app the default HTTPS
handler. Existing playback and cache rules apply when a video is opened.
