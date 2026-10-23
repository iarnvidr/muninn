# License inventory provenance

These texts were obtained from the upstream versions pinned in
`build/tools.lock.json`, except hls.js which is vendored at 1.6.15:

- hls.js: https://github.com/video-dev/hls.js/blob/v1.6.15/LICENSE
- Apache-2.0 full text: https://www.apache.org/licenses/LICENSE-2.0.txt
- yt-dlp: https://github.com/yt-dlp/yt-dlp/tree/2026.08.19 (LICENSE and THIRD_PARTY_LICENSES.txt)
- Deno: https://github.com/denoland/deno/blob/v2.9.7/LICENSE.md
- FFmpeg (LGPL 2.1 text; the bundled build is LGPL-only): https://github.com/FFmpeg/FFmpeg/blob/n9.0.2/COPYING.LGPLv2.1
- dav1d 1.5.4 (BSD-2-Clause, linked into FFmpeg): COPYING from https://downloads.videolan.org/pub/videolan/dav1d/1.5.4/dav1d-1.5.4.tar.xz

`rust-packages.json` records the 507 registry packages in the locked Cargo graph,
including platform-specific and build dependencies. It contains no registry paths.
`build/license_inventory.py --collect DIR` additionally copies available upstream
notice files into a bundle. Some crate archives omit notice files; this collector
is not a complete redistribution audit. See docs/PUBLICATION.md before releasing
binaries. Deno requires further dependency/source provenance review; the short top-level licenses alone do not close that work.

The vendored hls.js SHA-256 is
`413a83e2bb0c77ed0bf0be105d539d17ef45dfd984a0b13ecd3b14a901383938`.
It matches the upstream 1.6.15 minified distribution byte for byte.
