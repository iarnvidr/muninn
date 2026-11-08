# Updater release preparation

Search places online results above the query field and local results below it. Both use media cards with thumbnails, creators and explicit Saved/Not saved labels; the side filters remain visible. Up from the field enters online results and Down enters local results. Typing stays local; Enter fetches only the selected online type, Videos by default. Visible Videos, Live and Channels controls switch types without fetching; current-query results are cached separately. Left/Right or h/l also switch online types. Enter on an empty type heading explicitly searches that type and enters its results.

The checked-in version is `0.2.23`. A GitHub prerelease can distribute an explicitly ad-hoc-signed, non-notarized build without an Apple Developer account. A prerelease does not activate the stable updater feed at `releases/latest`. Do not treat CI's synthetic updater fixtures or uploaded workflow artifacts as owner acceptance.

Settings defaults to **Stable only**. With no stable release, it explains that no stable update has been published. Choose **Stable and previews** to check published GitHub prereleases as well. This preference is local to the installation. Discovery selects the highest numeric version among the latest 100 published releases with a matching repository-owned `latest.json` asset; drafts and malformed tags are excluded. Preview manifests use their fixed release tag. The pinned verification key and signature checks apply to both channels; older and equal versions cannot be installed. A successful check with no newer version reports up to date. Preview builds may be unfinished and non-notarized; this option does not promote a build to stable.

**PR builds (separate app)** is a third, explicit opt-in discovery channel. It
requires matching successful Linux/Dagger and Apple Silicon GitHub Actions checks
for the current head of a same-repository pull request, plus an unexpired
`muninn-pr-preview-arm64` CI artifact. The UI identifies the PR, exact commit and
run, then opens the GitHub Actions page for an authenticated download. It never
feeds a PR artifact to the production updater or installs it in place: CI
artifacts lack a release updater signature. The PR artifact is a separately
signed `Muninn Preview.app`/DMG with bundle ID
`io.github.iarnvidr.muninn.preview`, separate data path, and no `muninn://`
URL registration. On the preview app, production update installation is blocked.
This does not automatically publish a release or merge the PR. The existing
`Stable only` and `Stable and previews` channels retain their semantics.

All new native CI builds embed their actual checkout SHA, PR head, PR number,
GitHub Actions run and channel in Settings and support diagnostics. The version
alone does not distinguish a PR build from an older release built with the same
SemVer. Before this change, diagnostic exports include only version/architecture.

Versions through `0.2.16` use only the stable endpoint. Install `0.2.17` manually once to gain preview-channel support. The owner confirmed playback works in `0.2.16`; actual owner-Mac updater installation acceptance is still pending.

Releases attach exactly three files: `Muninn_<version>_macos-arm64.dmg` for installation, `latest.json` for the updater, and `Muninn.app.tar.gz` as the update payload. The legacy repack workflow converts the initial `0.2.14` layout without rebuilding its app or changing updater files. The updater reads the signature embedded in `latest.json`; a separately attached `.sig` is unnecessary. Its existing endpoint and pinned verification key remain unchanged.

Checksums, detached signatures, signing evidence and review metadata stay in CI artifacts. The DMG contains the app, an Applications shortcut, and a `Corresponding Source` folder with pinned source archives, recipe, manifest and notices. Packaging mounts the finished image read-only and verifies the app signature/version and source archive digests before publication.

Tauri v2 updater format uses `latest.json` with `version`, `notes`, `pub_date`, and `platforms`; macOS Apple Silicon is `darwin-aarch64` with `url` and `signature`. The signed gzip tar update payload is separate from the manual-install DMG. **A successful same-repository PR merge to `main` now automatically publishes a prerelease when its source version is newer than the latest published version.** The post-merge `Map accepted Muninn native artifact` workflow first verifies the PR's exact native artifact and Linux/Dagger check receipts. Only its successful `workflow_run` event can trigger the trusted `Publish accepted Muninn preview` workflow. It retrieves the accepted app rather than compiling again, verifies its ad-hoc Apple signature, signs and verifies the Tauri updater archive, mount-verifies the DMG and corresponding sources, and creates a draft release. Publication happens only after the DMG, `latest.json` and `Muninn.app.tar.gz` all exist with matching SHA-256 digests; repeated runs never overwrite published assets. The workflow skips merges with an already published version and rejects version regressions, unexpected tag targets, stale `main` commits or missing signing credentials. A GitHub prerelease still does not activate the stable `releases/latest` updater feed.

Automatic preview publication needs `TAURI_SIGNING_PRIVATE_KEY` (and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` if the key is encrypted) accessible to the trusted workflow as repository-level Actions secrets. It never sends these secrets to PR checks or GitHub Packages cache publishers. A preview remains ad-hoc signed and not notarized. Its release tag is pinned to the accepted merge commit. For new releases, bump the checked-in Tauri version in the PR so that the merge will publish a new `v<version>` prerelease; no manual tag, release or workflow dispatch is needed.

## Provisioning and release gate

The automatic path above handles ad-hoc-signed *prereleases*. For the separate
protected production workflow, select `apple_signing=ad-hoc` explicitly if using
that distribution path; its default `developer-id` mode still requires all six
Apple secrets and notarization. Both protected modes retain updater signing,
matching version/tag/main checks and environment review. Use stable publication
only after production acceptance; see the ad-hoc section of
[RELEASE-CHECKLIST.md](RELEASE-CHECKLIST.md). The remaining production gate below
describes promotion to a notarized, accepted stable release.

Configure GitHub environment exactly `muninn-release`, required reviewer `efthomsen`, deployment branch limited to protected `main`. Verify current active main allowlist/ruleset; legacy branch-protection API 404 is not proof of missing protection.

Real Developer ID signing/notarization requires an authorized Apple account holder and actual Apple credentials. Exact six secret names required by workflow/helper: `APPLE_DEVELOPER_ID_APPLICATION_P12_BASE64` (base64 password-protected Developer ID Application PKCS#12), `APPLE_DEVELOPER_ID_APPLICATION_P12_PASSWORD`, `APPLE_TEAM_ID`, `APPLE_NOTARY_API_KEY_ID`, `APPLE_NOTARY_API_ISSUER_ID`, `APPLE_NOTARY_API_PRIVATE_KEY` (App Store Connect API private-key PEM). This is a genuine external Apple-account dependency; no Apple keys were supplied, accessed or tested. The separate updater signing secrets are `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`; this task did not inspect their values.

For production promotion, review the intended protected-main commit and matching source version/tag, finish the target-specific notice/source audit, and complete actual Mac acceptance. Dispatch from main with matching version/tag and publish false; inspect notarization/stapling/Gatekeeper evidence, bundle, updater signature verification against pinned public key, checksums, manifest, licenses and source bundle. Only after owner/reviewer approval of all evidence configure/approve environment and dispatch publish true. A prerelease can be published meanwhile, but the stable feed remains inactive until promotion. Keep notices, helper licenses and sources intact; see [PUBLICATION.md](PUBLICATION.md) and [RELEASE-CHECKLIST.md](RELEASE-CHECKLIST.md).
