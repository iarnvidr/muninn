# Release checklist

Build success does not by itself authorize a public release.

## Source

- [x] Apache-2.0 for Muninn's own code (`LICENSE`, package metadata).
- [x] Secret scans and `build/check_publication.py` boundary checks pass.
- [x] CI uses read-only permissions, SHA-pinned actions, no secrets on pull request
  paths and no `pull_request_target`.
- [ ] Review contribution and security documents; enable private vulnerability reports.
- [ ] Review native macOS CI results and update verification notes accurately.

## Binary releases

- [ ] Collect complete Rust and Deno dependency notices for the shipped build.
- [x] FFmpeg is built from the pinned official 9.0.2 source release with an LGPL-only
  configuration, plus dav1d (BSD-2-Clause), by `build/ffmpeg_lgpl.py`; no GPL,
  non-free, x264 or x265.
- [ ] Publish the exact FFmpeg and dav1d source tarballs, configure options and recipe
  next to each binary release (LGPL source offer), and test the build on an Apple
  Silicon Mac.
- [ ] Ship `licenses/` (including `yt-dlp-THIRD_PARTY_LICENSES.txt`, which carries the GPL
  texts for the GPL components bundled in the yt-dlp executable) with every binary.
- [ ] Test the actual Apple Silicon app: imported files, retry, resume, seeking,
  1×/1.5× playback, theatre toggle-back, PiP and login persistence.
- [ ] Publish a clearly labelled preview with known issues and verified checksums.

No release assets are created automatically by the workflows.
