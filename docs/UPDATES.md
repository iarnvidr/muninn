# Updater release preparation

The checked-in version is `0.2.19`. A GitHub prerelease can distribute an explicitly ad-hoc-signed, non-notarized build without an Apple Developer account. A prerelease does not activate the stable updater feed at `releases/latest`. Do not treat CI's synthetic updater fixtures or uploaded workflow artifacts as owner acceptance.

Settings defaults to **Stable only**. With no stable release, it explains that no stable update has been published. Choose **Stable and previews** to check published GitHub prereleases as well. This preference is local to the installation. Discovery selects the highest numeric version among the latest 100 published releases with a matching repository-owned `latest.json` asset; drafts and malformed tags are excluded. Preview manifests use their fixed release tag. The pinned verification key and signature checks apply to both channels; older and equal versions cannot be installed. A successful check with no newer version reports up to date. Preview builds may be unfinished and non-notarized; this option does not promote a build to stable.

Versions through `0.2.16` use only the stable endpoint. Install `0.2.17` manually once to gain preview-channel support. The owner confirmed playback works in `0.2.16`; actual owner-Mac updater installation acceptance is still pending.

Releases attach exactly three files: `Muninn_<version>_macos-arm64.dmg` for installation, `latest.json` for the updater, and `Muninn.app.tar.gz` as the update payload. The legacy repack workflow converts the initial `0.2.14` layout without rebuilding its app or changing updater files. The updater reads the signature embedded in `latest.json`; a separately attached `.sig` is unnecessary. Its existing endpoint and pinned verification key remain unchanged.

Checksums, detached signatures, signing evidence and review metadata stay in CI artifacts. The DMG contains the app, an Applications shortcut, and a `Corresponding Source` folder with pinned source archives, recipe, manifest and notices. Packaging mounts the finished image read-only and verifies the app signature/version and source archive digests before publication.

Tauri v2 updater format uses `latest.json` with `version`, `notes`, `pub_date`, and `platforms`; macOS Apple Silicon is `darwin-aarch64` with `url` and `signature`. The signed gzip tar update payload is separate from the manual-install DMG. Workflow validates stable version, existing `v<version>` tag and exact equality of tag, protected-main commit and dispatched SHA. It prepares and verifies artifacts with `publish=false`; only the separate `publish=true` job has write permission and uploads to an already existing release. No tag/release is created by workflow. A GitHub prerelease still does not activate the stable `releases/latest` updater feed.

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

For production promotion, review the intended protected-main commit and matching source version/tag, finish the target-specific notice/source audit, and complete actual Mac acceptance. Dispatch from main with matching version/tag and publish false; inspect notarization/stapling/Gatekeeper evidence, bundle, updater signature verification against pinned public key, checksums, manifest, licenses and source bundle. Only after owner/reviewer approval of all evidence configure/approve environment and dispatch publish true. A prerelease can be published meanwhile, but the stable feed remains inactive until promotion. Keep notices, helper licenses and sources intact; see [PUBLICATION.md](PUBLICATION.md) and [RELEASE-CHECKLIST.md](RELEASE-CHECKLIST.md).
