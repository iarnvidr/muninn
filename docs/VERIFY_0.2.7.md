# Muninn 0.2.7 test build

The 0.2.6 Mac diagnostics show WebM files stalling at the start and during seeking,
then playing successfully through local H.264/AAC HLS preparation. MKV and WebM
now use that path automatically on opening, without the ten-second direct-play
failure first. Other containers still use direct playback, with automatic recovery
on decoding/format errors or stalls. The user-facing compatibility control is removed.
This may use temporary disk space and CPU while preparing a WebM file; originals
are preserved and no media is downloaded from YouTube for the local preparation.

Theatre mode previously only hid the related-video column, which is already below
the player at narrower widths. It now visibly expands the player to the content
edges, increases its allowed height and removes the normal width cap at all window
sizes. The control toggles back to the normal layout and reports its pressed state.
Toggle events are included in diagnostics.

Large URL collections can be imported as durable library links, not browser tabs.
The CLI accepts URL lists or JSON URL/title records, supports named collections and
dry runs, deduplicates by video ID, and validates before committing links and
memberships in one transaction. Import runs no media or metadata requests and
queues no downloads. The Library Links filter provides Play, Download and Remove.
Existing cache entries and saved links for the same video show one media row.

Targeted tests cover offline persistence, deduplication, dry runs, rejected imports
leaving no partial state and local media preservation. Real ffmpeg checks cover
automatic MKV and AV1 WebM preparation and HLS decoding. Browser checks exercise
theatre on/off at narrow and wide widths, visible controls without a compatibility
button, and saved-link presentation and actions. Full regression and packaging
checks run before the Apple Silicon build. Native Mac launch/PiP still need testing.
