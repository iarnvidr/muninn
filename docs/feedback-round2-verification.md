# Feedback round 2: verification and readiness evidence

## Settings audit (read-only)

Inspected `ui/pages.js` Settings implementation and `ui/lib.js` account/readiness helpers. Account state is deliberately observation-based: `signed_in` and `signed_out` are returned only for those exact reported states; null, missing, or unrecognized account state maps to `not_observed` (“YouTube sign-in has not been observed yet”). The UI separately says status is temporarily unavailable on request failure and does not expose identifiers. This correctly avoids representing absence of evidence as signed-out.

The account poller requests `/api/channels/status` every 4 seconds and serializes overlapping requests (`accountBusy`). Setup refresh calls `M.pollSystemNow`; the setup poller runs every 5 seconds. These are live refresh mechanisms, not proof a server/native integration works in a running installation.

Existing unit evidence: `ui/test/lib.test.js` checks null/missing/unrecognized account states, exact signed-in/signed-out handling, and omission of an account identifier from serialized output. It also tests shortcut guards and actions. These are source-level tests only; no test command was run as part of this audit, so no tests are claimed as passing.

Readiness is presented as independent dimensions: downloader installation, FFmpeg availability, secure-browsing trust, and playback observation. UI explicitly says helper availability does not establish playback health; playback is not inferred absent active-playback evidence. Trust unavailable is distinguished from a checked state. No observed defect in these semantics from the audited files.

Potential gap: a failed account-status request changes the status text but does not update the sign-in button state; on a later failure the prior button label/visibility may remain, which could imply stale status. Consider explicitly marking the button/status as stale or disabling it after failures. This is a UI resilience concern, not evidence that the signed-out/unknown mapping is wrong.

## Verification limits

This review is code/test-source inspection only. It does not establish native OS shortcut behavior, browser control behavior, actual YouTube session state, secure-browsing installation, or live refresh success. No personal/private identifiers or media evidence are included. No code was edited.

For documented shortcut conflicts and capability boundaries, see `SHORTCUTS.md`; implementation references are `ui/lib.js` and existing assertions in `ui/test/lib.test.js`.
