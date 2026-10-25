# Release-readiness review — 2026-10-25

This is a documentation/readiness review, not approval to publish. Intended stable target is `0.2.14`; repository Cargo/Tauri metadata still says `0.2.14-preview.4`. No `v0.2.14` release tag or published release was found in the local checkout; no release publication is claimed. The owner must bump/tag on protected main and create the Release before workflow dispatch.

## Current controls and external dependency

The protected updater release workflow is manual-only, requires dispatch from `main`, exact matching existing tag/commit/version, builds on hosted macOS, signs/notarizes/staples the app, prepares updater assets plus corresponding source, and separates read-only prepare from write-capable upload. Publishing targets an existing release only. `muninn-release` must be configured to require reviewer `efthomsen`, with deployment branches restricted to protected `main`. Confirm active protected-main allowlist/ruleset directly; a legacy protection API HTTP 404 is not evidence main lacks protection.

Actual production signing is contingent on the account holder providing a valid Developer ID Application PKCS#12 certificate/password and App Store Connect notarization API credentials. Required exact environment secret names: `APPLE_DEVELOPER_ID_APPLICATION_P12_BASE64`, `APPLE_DEVELOPER_ID_APPLICATION_P12_PASSWORD`, `APPLE_TEAM_ID`, `APPLE_NOTARY_API_KEY_ID`, `APPLE_NOTARY_API_ISSUER_ID`, `APPLE_NOTARY_API_PRIVATE_KEY`. No Apple account keys were supplied, accessed, or validated. Updater secrets `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` are separate. Stubbed helper tests are contract-only and are not evidence of a signed or notarized artifact.

## Notice/source audit facts and remaining work

The repository contains license texts/notices for hls.js, the bundled fonts, yt-dlp including its third-party license file, Deno's top-level MIT license, FFmpeg LGPL-2.1 and dav1d BSD-2-Clause. THIRD_PARTY_NOTICES.md describes helper versions, provenance and FFmpeg configuration; `licenses/rust-packages.json` records 507 locked crates. Its `license_file_declared` field is false for all 507 records, and `build/license_inventory.py` only collects available files. Thus crate metadata is not a complete notice bundle. Deno's top-level license does not resolve its bundled dependency notices/source provenance. These are concrete remaining audit blockers, not a blanket claim that no shipped notices exist.

Before binary publication: derive actual Apple Silicon shipped/linked dependency set from build provenance; check applicable Rust crate notices and collect missing license texts/attributions; audit Deno bundled dependency notice/source provenance; compare every required notice with bundle contents. Inspect `build/release_sources.py prepare` output against actual binary and build recipe, including exact FFmpeg/dav1d archives/configuration and corresponding source/redistribution obligations for bundled helper components. Have owner/reviewer record the source and distribution review. This is preparation, not a legal-compliance determination.

Automated macOS CI produces a build artifact and synthetic updater checks; neither is owner acceptance of live Mac playback, import/retry/resume/seeking, app-link delivery, login persistence, theatre/PiP, or production notarization. Follow [RELEASE-CHECKLIST.md](RELEASE-CHECKLIST.md), retain real owner evidence, and keep `publish=false` until reviewed.

## Decision

**Not ready for production binary publication.** Blockers: version/tag/release not prepared; protected environment/reviewer configuration must be confirmed; real Apple account credentials are an external prerequisite; target-specific notices and corresponding-source review are unfinished; owner Mac acceptance is not recorded. No checks are marked complete by this review.
