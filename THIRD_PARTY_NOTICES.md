# Third-party components

The Apache-2.0 license of Muninn's own code does not replace these terms.
This inventory is not a claim that binary-distribution obligations have been fully
satisfied. See the [release checklist](docs/PUBLICATION.md).

| Component | Shipped form | License / provenance |
| --- | --- | --- |
| hls.js 1.6.15 | `ui/hls.min.js`, embedded in Muninn | Apache-2.0; copyright and attribution in `licenses/hls.js-LICENSE`, full text in `licenses/Apache-2.0.txt`; https://github.com/video-dev/hls.js/tree/v1.6.15 |
| Cinzel | bundled font | SIL OFL-1.1, `ui/fonts/Cinzel-OFL.txt` |
| Inter | bundled font | SIL OFL-1.1, `ui/fonts/Inter-OFL.txt` |
| IBM Plex Mono | bundled font | SIL OFL-1.1, `ui/fonts/IBMPlexMono-OFL.txt` |
| yt-dlp 2026.08.19 | official macOS executable, started as a separate program | Source Unlicense (`licenses/yt-dlp-LICENSE`). The PyInstaller executable bundles third-party components, all with full texts in `licenses/yt-dlp-THIRD_PARTY_LICENSES.txt`; in the macOS build the copyleft ones are mutagen (GPL-2.0-or-later; GPL v2 text in that file; source https://github.com/quodlibet/mutagen) and certifi (MPL-2.0). The file's GNU Readline, libstdc++, libgcc, libintl, libidn2 and libunistring entries are marked there as Linux-only. Source for each component is available from the URLs in that file; yt-dlp source: https://github.com/yt-dlp/yt-dlp/tree/2026.08.19. Muninn runs the executable as an independent process and does not link or incorporate these components. |
| Deno 2.9.7 | official executable | MIT plus bundled dependencies; `licenses/deno-LICENSE.md`; https://github.com/denoland/deno/tree/v2.9.7 |
| FFmpeg 9.0.2 | static executable built by `build/ffmpeg_lgpl.py` from the unmodified official source release | LGPL-2.1-or-later, with no GPL or non-free components (no `--enable-gpl`, `--enable-nonfree`, `--enable-version3`, x264 or x265); `licenses/ffmpeg-COPYING.LGPLv2.1`. Source: https://ffmpeg.org/releases/ffmpeg-9.0.2.tar.xz (SHA-256 `8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e`, signature `.asc` alongside). Links only Apple system frameworks (VideoToolbox, AudioToolbox, Secure Transport), zlib and dav1d (below). Configure options: `--enable-static --disable-shared --disable-gpl --disable-nonfree --disable-autodetect --disable-doc --disable-debug --disable-ffplay --disable-ffprobe --enable-zlib --enable-securetransport --enable-videotoolbox --enable-audiotoolbox --enable-libdav1d --pkg-config-flags=--static --arch=arm64 --extra-cflags=-mmacosx-version-min=14.0 --extra-ldflags=-mmacosx-version-min=14.0`. The same string is printed by `ffmpeg -version` and written to the bundle's `bin/NOTICE.txt`. The binary is a separate program; replace `Contents/Resources/bin/ffmpeg` with your own build to modify it. |
| dav1d 1.5.4 | statically linked into the FFmpeg binary | BSD-2-Clause; license text `licenses/dav1d-COPYING` (must accompany binaries). Source: https://downloads.videolan.org/pub/videolan/dav1d/1.5.4/dav1d-1.5.4.tar.xz (SHA-256 `686616b7c69eb88d44459391ab25cac13b6647a3b288835c5784e71c1514a5c5`, the publisher's digest), unmodified, built by `build/ffmpeg_lgpl.py` for software AV1 decoding. |
| Rust dependencies | linked libraries / build tools | Pinned by Cargo.lock; 507 registry packages across all platforms, recorded in `licenses/rust-packages.json`. Regenerate with `python3 build/license_inventory.py`. Packaging collects available upstream license files; review gaps before release. |

Helper download URLs, versions and SHA-256 digests are pinned in
`build/tools.lock.json`. Bundles include the license directory, this notice and
the font notices. Upstream helper self-updates can change the installed yt-dlp
version independently of the original bundle.

## Artwork

The photo-derived raven appears in `design/raven-photo-styled.png`, `ui/raven.png`
and the application icons. The owner confirmed permission to redistribute the
photo-derived logo on 2026-10-16. That confirmation is not a grant to relicense
the original photograph or use the Muninn branding to imply endorsement. The
original photograph is not in this repository.

## Sources and complete notices

Upstream release and repository links identify origins; a generic source link is
not being presented as a complete corresponding-source offer for bundled GPL
binaries. Do not publish a binary until the exact source archives, build scripts,
dependency notices and distribution method have been checked and recorded.
