# Updater release preparation

The checked-in version is `0.2.14`. A GitHub prerelease can distribute an explicitly ad-hoc-signed, non-notarized build without an Apple Developer account. A prerelease does not activate the stable updater feed at `releases/latest`. Do not treat CI's synthetic updater fixtures or uploaded workflow artifacts as owner acceptance.

Tauri v2 updater format uses GitHub Releases `latest.json` with `version`, `notes`, `pub_date`, and `platforms`; macOS Apple Silicon is `darwin-aarch64` with `url` and `signature`. Payload is gzip tar `Muninn.app.tar.gz` of exact bundle plus detached Tauri signature; it is not the manual-install ZIP. Workflow validates stable version, existing `v<version>` tag and exact equality of tag, protected-main commit and dispatched SHA. It prepares and verifies artifacts with `publish=false`; only the separate `publish=true` job has write permission and uploads to an already existing release. No tag/release is created by workflow.

## Provisioning and release gate

Select `apple_signing=ad-hoc` explicitly for that distribution path; the default
`developer-id` mode still requires all six Apple secrets and notarization. Both
modes retain updater signing and verification, matching version/tag/main checks,
and protected environment review. Publish ad-hoc candidates as GitHub prereleases
while production acceptance remains outstanding; see the ad-hoc section of
[RELEASE-CHECKLIST.md](RELEASE-CHECKLIST.md). The remaining production gate below
describes promotion to a notarized, accepted stable release.

Configure GitHub environment exactly `muninn-release`, required reviewer `efthomsen`, deployment branch limited to protected `main`. Verify current active main allowlist/ruleset; legacy branch-protection API 404 is not proof of missing protection.

Real Developer ID signing/notarization requires an authorized Apple account holder and actual Apple credentials. Exact six secret names required by workflow/helper: `APPLE_DEVELOPER_ID_APPLICATION_P12_BASE64` (base64 password-protected Developer ID Application PKCS#12), `APPLE_DEVELOPER_ID_APPLICATION_P12_PASSWORD`, `APPLE_TEAM_ID`, `APPLE_NOTARY_API_KEY_ID`, `APPLE_NOTARY_API_ISSUER_ID`, `APPLE_NOTARY_API_PRIVATE_KEY` (App Store Connect API private-key PEM). This is a genuine external Apple-account dependency; no Apple keys were supplied, accessed or tested. The separate updater signing secrets are `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`; this task did not inspect their values.

Owner must first update version to `0.2.14`, review a protected-main commit, create `v0.2.14` and its GitHub Release, finish the target-specific notice/source audit, and complete actual Mac acceptance. Dispatch from main with matching version/tag and publish false; inspect notarization/stapling/Gatekeeper evidence, bundle, updater signature verification against pinned public key, checksums, manifest, licenses and source bundle. Only after owner/reviewer approval of all evidence configure/approve environment and dispatch publish true. Until then there is no active feed or release. Keep notices, helper licenses and source assets intact; see [PUBLICATION.md](PUBLICATION.md) and [RELEASE-CHECKLIST.md](RELEASE-CHECKLIST.md).
