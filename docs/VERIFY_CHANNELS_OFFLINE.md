# Channel and offline change verification

Implemented on top of the 0.2.11 player Keep-point work. Repository visibility
and the release pipeline are unchanged.

Automated checks:

- `cargo test --locked -p muninn-core`: 204 unit tests and 28 integration tests
  passed, including stale catalogue refresh/coalescing/fallback/restart, offline
  network denial, local playback, cancellation/requeue and featured expiry.
- Workspace tests in the existing Linux builder also include four shell tests;
  see [recorded startup verification](VERIFY_RECORDED_START.md) for the playback
  regression added after the account-state changes.
- `node --test ui/test/*.test.js src-tauri/test/*.test.js`: 47 passed. The new
  unsubscribe cases use synthetic responses and never access an account.
- Apple Silicon release cross-compilation with the production compiler image
  succeeded. Cross-compilation does not verify WKWebView runtime behaviour.
- Chromium with the production UI and a synthetic local API/native bridge:
  measured carousel/native-pane separation, reopened an upload list without
  another request, confirmed unsubscribe and preserved policies on rejection,
  autosaved offline mode, displayed cached Browse and reconnected. No page
  JavaScript errors occurred in the successful run.

Still unverified: live YouTube unsubscribe acknowledgement/session behaviour,
native macOS tab hibernation/reconnect and real network interruption timing.
No YouTube account action was performed during verification.

Sign-in extension checks cover unknown/signed-in/signed-out state, account
switching, persisted caches after restart, rejected stale background observations,
opaque identity reporting, same-document stale subscription data, empty lists,
offline account invalidation, unsubscribe account checks and signed-out public
downloads/favourite discovery. Chromium used the production UI with a synthetic
session API: unknown and mismatched accounts disabled unsubscribe, matching
identity enabled it, sign-out preserved local downloads, and offline disabled
account actions. Live Google login/logout and native WKWebView remain unverified.
