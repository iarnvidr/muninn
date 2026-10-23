# Muninn 0.2.6 test build

Channels now occupy the page without a nested vertical scroller or enclosing setup
frame. Search, subscription discovery and upload checks share a compact toolbar.
Per-channel download, quality and retention controls remain visible. Channel groups
share policies, including 1080p; explicit channel overrides win. Bulk group assignment
clears overrides. Group changes affect future automatic downloads, leaving existing
copies intact. Until watched remains the default, followed by the completion grace
period (24 hours by default); Keep protects copies.

Each channel expands inline into In Muninn and Channel videos. Library copies are
matched by channel identity; public uploads are listed using bounded yt-dlp metadata
requests, 25 per page, with a 30-minute cache. Play creates a player tab. Download
uses the effective quality and Keep flag; age retention for this manual action uses
normal cache rules. Listing alone does not download media. Shorts and live archives
are not included in the Videos-tab listing.

The tab strip and compact navigation sit directly above the content column. Active
tabs scroll into view. Browse uses browser back/forward/reload; player back returns
to the originating view and refresh restarts playback. Internal pages have contextual
refresh and return-to-tab actions. Native child-view bounds account for page zoom.
The player uses accessible, visible icon controls, with no playback fold-out.

Verification: three new Rust integration tests cover persisted group inheritance,
explicit overrides and group membership validation; a fake feed reaching the real
poller queues 1080p; upload parsing and imported-library channel identity are checked.
Browser checks against the real local service used 116 discovered channels: no
nested vertical scrolling or horizontal document overflow at 1280 pixels; persisted
bulk assignments and channel overrides; real public upload pagination (25 then 50
items); local library expansion; download request quality 1080; Play opening a new
tab, visible SVG player controls and return to Library.

The browser shell uses mock native tabs for these checks. macOS frame alignment,
native history, signed-in YouTube discovery, PiP permissions and the final app launch
still require testing on a Mac. All workspace Rust, JavaScript and packaging checks
run in the Apple Silicon build script before packaging; results are recorded with
the release. The app is ad-hoc signed and not notarized.
