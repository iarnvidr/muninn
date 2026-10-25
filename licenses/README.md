# License inventory provenance and audit status

These files provide concrete license material for components identified by the current source/pin inventory; they do not constitute legal advice or certify a complete redistribution audit.

- hls.js is vendored at 1.6.15; license at https://github.com/video-dev/hls.js/blob/v1.6.15/LICENSE
- Apache-2.0 full text: https://www.apache.org/licenses/LICENSE-2.0.txt
- yt-dlp 2026.08.19: `yt-dlp-LICENSE` and `yt-dlp-THIRD_PARTY_LICENSES.txt`; upstream https://github.com/yt-dlp/yt-dlp/tree/2026.08.19
- Deno 2.9.7 top-level license: `deno-LICENSE.md`; upstream https://github.com/denoland/deno/blob/v2.9.7/LICENSE.md
- FFmpeg 9.0.2 LGPL 2.1 text: `ffmpeg-COPYING.LGPLv2.1`; upstream https://github.com/FFmpeg/FFmpeg/blob/n9.0.2/COPYING.LGPLv2.1
- dav1d 1.5.4 BSD-2-Clause text: `dav1d-COPYING`; publisher source archive https://downloads.videolan.org/pub/videolan/dav1d/1.5.4/dav1d-1.5.4.tar.xz
- Full Apache-2.0 text: `Apache-2.0.txt`.

`build/tools.lock.json` pins yt-dlp 2026.08.19, Deno 2.9.7 and FFmpeg 9.0.2; FFmpeg configuration is in `THIRD_PARTY_NOTICES.md` and `build/ffmpeg_lgpl.py`. `THIRD_PARTY_NOTICES.md` identifies macOS yt-dlp's bundled mutagen (GPL-2.0-or-later) and certifi (MPL-2.0), with Linux-only entries noted in its upstream component list. The corresponding third-party texts are in `yt-dlp-THIRD_PARTY_LICENSES.txt`.

`rust-packages.json` lists 507 registry crates from Cargo.lock, including platform/build dependencies; all 507 currently have `license_file_declared: false`. This means the inventory is package/license/repository metadata, not full texts or proof every package ships in the Apple Silicon app. `build/license_inventory.py --collect DIR` copies available upstream notices only and explicitly cannot close the audit. Required remaining work: identify actual macOS shipped/linked Rust dependencies from build provenance; check applicable crate license texts and attributions; inspect Deno's bundled dependency notices and source provenance (its top-level MIT text is not enough); compare complete, applicable notices to the packaged `Contents/Resources/licenses` and correct any gaps. Preserve GPL/MPL helper component texts where applicable.

For each release, inspect the actual `build/release_sources.py prepare` output, source archives, recipes/configuration, notices, binary and distribution method. FFmpeg/dav1d pinned source and build recipe do not automatically establish a complete corresponding-source offer. No generic inventory should be presented as legal-compliance sign-off. See [publication gates](../docs/PUBLICATION.md).
