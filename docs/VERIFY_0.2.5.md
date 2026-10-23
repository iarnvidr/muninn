# Muninn 0.2.5 test build

Sponsored browse/search cards are hidden using structural ad containers, without
matching translated label text or video titles. CSS applies to newly inserted
cards during SPA navigation and infinite scrolling. Newly captured feeds skip ad
subtrees before extracting or deduplicating video IDs. Creator sponsorship segments
inside videos are outside this change.

The photo-based red raven is now the sidebar logo in every theme, the favicon,
and the app icon in PNG, ICO and ICNS formats. The Mac ICNS contains PNG entries
from 16 through 1024 pixels. All sizes derive from the same committed artwork;
no old logo is used in the build. The export script is reproducible with ImageMagick.

Browser fixture checks execute the actual page injection script: organic cards
remain visible; Danish-labelled sponsored cards, search ads and ads inserted after
load are hidden. The real service serves the new PNG and the sidebar loads it.
Core regression tests cover an advertised video appearing before its organic copy,
plus an ordinary video whose title mentions sponsorship.

The full Rust, JavaScript and packaging checks run before the Apple Silicon build.
The bundle is ad-hoc signed and verified on Linux; it has not been launched here
on macOS. Live authenticated YouTube markup can change and still needs Mac testing.
The app is not notarized.

The user's 0.2.4 diagnostic export showed successful discovery of 116 channels.
The channel picker is replaced with a visible searchable list and Follow actions;
discovery status is separate from download-poll feedback. Checking uploads with no
followed channels explains the required action instead of claiming work is running.

Local MKV copies now prepare H.264/AAC HLS automatically. Other local files can
use the same compatibility path after a decoder/format error or through Playback
options. Originals stay unchanged. Preparation is cancelled on leaving playback,
uses two encoder threads, checks minimum free space and limits temporary output to
one quarter of the disk budget. Finished older compatibility streams are removed
when another is prepared; the normal HLS startup cleanup also clears these files.
Decode errors are recorded as ui.playback.error for future diagnostic exports.
Playback stalls also trigger the compatibility path after 10 seconds without
progress. Media response status/ranges and browser playback state are logged.
MOV, AAC and WAV responses now have explicit media content types.

A real ffmpeg check imports an MKV containing MPEG-4 video, prepares/decodes its
H.264 HLS, checks the watch status and verifies both source and imported original
are byte-for-byte unchanged. Native Mac playback still requires user confirmation.

Diagnostics schema 2 includes up to 200 recently accessed entries with container,
file existence/actual bytes, height and duration; per-format counts; channel count
and discovery state; and bounded local ffmpeg probes of the three most recent
complete copies. Probes list container and stream codecs without exporting file
paths or media, time out after three seconds each, and only inspect managed files.
The real-media regression checks these fields and that paths are excluded.

Structural selector reference:
https://github.com/uBlockOrigin/uAssets/issues/19350
