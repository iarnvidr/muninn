# Owner production release gate

## Ad-hoc distribution

An Apple Developer account is optional for distributing a GitHub release. Select
`apple_signing=ad-hoc` explicitly to retain the native build's ad-hoc signatures
and skip Developer ID signing/notarization. The workflow still verifies the app's
code signatures and signs/verifies the updater archive against the pinned public
key. Protected main, matching source version/tag and environment approval still
apply. `developer-id` remains the default and requires the Apple secrets below.

Ad-hoc release notes must disclose that the app is not notarized and macOS may
block the initial launch. Use a GitHub prerelease while the owner Mac acceptance
and full notice/source review below remain outstanding; it must not be described
as accepted production software. The prerelease is excluded from GitHub's
`releases/latest` endpoint, so it does not activate the stable updater feed.

This is an owner acceptance checklist for the exact protected-main commit intended for a production macOS release. Record commit, macOS version/hardware, and evidence. A CI build, CI launch smoke, synthetic updater artifacts, or this checklist text are not owner-live acceptance. No release has been published; current version is `0.2.14-preview.4`, and intended `0.2.14` is not yet bumped/tagged.

## Before dispatch

- [ ] Owner confirms `main` protection via active allowlist/ruleset (legacy branch-protection API 404 does not prove unprotected).
- [ ] `muninn-release` environment has required reviewer `efthomsen` and a deployment branch rule limited to protected `main`.
- [ ] Account holder provisions Developer ID Application signing certificate and App Store Connect notarization API key from the active Apple Developer account into these exact environment secrets: `APPLE_DEVELOPER_ID_APPLICATION_P12_BASE64`, `APPLE_DEVELOPER_ID_APPLICATION_P12_PASSWORD`, `APPLE_TEAM_ID`, `APPLE_NOTARY_API_KEY_ID`, `APPLE_NOTARY_API_ISSUER_ID`, `APPLE_NOTARY_API_PRIVATE_KEY`. No actual Apple keys are included/provided here. Existing updater secrets are separate: `TAURI_SIGNING_PRIVATE_KEY`, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`.
- [ ] Owner updates Cargo/Tauri version to stable `0.2.14`, reviews commit on protected main, creates matching `v0.2.14` tag and GitHub Release. Workflow requires an existing tag/Release and never creates them.
- [ ] Complete target-specific Rust and Deno notice audit; inspect generated corresponding-source bundle and confirm exact licenses, notices, build recipe and distribution method. See [PUBLICATION.md](PUBLICATION.md).

## Workflow preparation evidence

Dispatch `Protected updater release` from `main` with version `0.2.14`, tag `v0.2.14`, `publish=false`. Verify exact tag commit equals workflow `github.sha` and protected main commit; version matches config and app. Review downloadable artifacts: production app signing/notarization evidence, `Muninn.app.tar.gz`, detached updater signature and successful verification against configured public key, `latest.json`, checksums, source archives/recipe and review metadata. Confirm app bundle contains `Contents/Resources/licenses`. Any missing/failed verification blocks release. The production workflow requires Developer ID signing, notarization Accepted, stapling and Gatekeeper assessment; helper-only stub tests do not establish any actual Apple signature or notarization.

## Owner Mac acceptance

Perform on Apple Silicon with macOS 14+ using the exact candidate artifact/commit; retain actual outputs and observations. CI 40-test claims and synthetic updater workflow tests do not satisfy these checks.

- [ ] Record `uname -sm`, `sw_vers -productVersion`, commit and artifact checksum. Confirm signature identity/team, notarization ticket and Gatekeeper assessment on the actual app.
- [ ] Install/open from a clean download; verify launch, helper execution and no quarantine workaround requirement.
- [ ] Verify app-link and cold/warm URL delivery, local certificate prompt/trust behavior and login persistence over restart.
- [ ] Verify imported/local file, retry, resume, seeking and 1×/1.5× playback; perform actual compatibility conversion and confirm audio/video, theatre toggle-back and PiP.
- [ ] Exercise feature smoke paths relevant to the release, including library/channel/offline policy and sign-out/account switching.
- [ ] Play a VideoToolbox-produced test clip and a real HTTPS source; record results and any failures.
- [ ] Review final bundled notices, source assets and exact checksums; do not equate file presence or a generic inventory with legal compliance.

## Explicit publish authorization

- [ ] Owner/reviewer `efthomsen` has reviewed owner-Mac evidence, release assets, sources/notices, checksum and manifest.
- [ ] Only then dispatch again with `publish=true`; verify environment approval and upload to the already existing release. Record resulting release URL and asset list. Until these actions occur, do not describe the release as published or accepted.
