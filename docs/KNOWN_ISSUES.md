# Known issues and support scope

This is an early preview for Apple Silicon Macs running macOS 14 or newer.
The app is ad-hoc signed and is not notarized. No Windows or Linux end-user
packages are supported; Linux is used for core and WebKitGTK regression testing.

- **Local playback on macOS:** direct AV1/WebM decoding has stalled in field logs.
  Local H.264/AAC HLS conversion played the same file in earlier diagnostics, but
  the later black-spinner report is not established as fixed. Version 0.2.8 fixes
  demonstrated stop/retry, resume and source-selection lifecycle defects and adds
  tracing. Fresh macOS evidence is still needed for the remaining native HLS issue.
  Late resumes of MKV/WebM copies now prepare a window near the resume position
  ([timeline contract](LOCAL_PLAYBACK_WINDOW.md)); this is verified only with
  synthetic media on Linux, not yet on a Mac.
- **Theatre:** pane-height expansion and toggle-back pass the real Linux Tauri
  regression at three sizes. Native macOS child-webview layout (YouTube view vs. toolbar, zoom, display change,
  favourites carousel) still needs validation on a Mac; see `webview.layout` log entries.
- **Google sign-in and YouTube integration:** embedded sign-in can be rejected.
  YouTube page/API changes can affect browsing, subscriptions, comments or downloads.
- **PiP:** availability and automatic activation depend on the webview engine and
  operating system. Background playback must be tested independently of PiP.
- **Automation:** channel polling runs while Muninn is open. It is not a system
  daemon that keeps downloading after the application quits.

For a bug report, include the version and a reviewed diagnostic export. See
[CLI.md](CLI.md) for export commands and [VERIFY_0.2.8.md](VERIFY_0.2.8.md) for
the actual production-player checks and their limits.
