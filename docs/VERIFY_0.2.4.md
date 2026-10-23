# Muninn 0.2.4 test build

Library Play opens a new player tab. Browse returns to a YouTube tab and Player
returns to the current watch view. Settings save immediately, with visible status
and retry on failure. Changing PiP mode applies to an already-open video.

The sidebar mini-player has a preview, seek, elapsed time and playback controls;
collapsed mode keeps compact controls. The existing video element stays alive
when navigating away. Playback options and saved-copy management use disclosure
menus instead of duplicate button rows. Classified theme filter contrast is fixed.

Drag tabs to group labels or before another tab in the top strip; Library → Tab
groups supports the same group moves and ordering, plus dropdown alternatives.
The organization API persists the ordered IDs for app and CLI access.

Subscribed-channel discovery now reads named channel candidates directly from
the authenticated channels page and scrolls for additional results. Progress and
terminal states are persisted and shown in the UI. Loading subscriptions does
not enable automatic downloads without channel opt-in.

Verification uses the real Rust service/API with a simulated native shell in
Playwright: Library Play adds a watch tab; Browse preserves it and returns to a
YouTube tab; manual-to-automatic PiP changes an already-open video; settings persist
without Save; top-strip group moves/reordering and Library group moves persist;
selected filters have a filled accent background; collapsed controls are present.
Native macOS PiP and authenticated discovery still require testing on the Mac.

The build runs the Rust workspace, JavaScript and packaging suites, cross-compiles
for Apple Silicon, verifies bundle structure/helper architectures/ad-hoc signatures,
and checks the ZIP. The application is not notarized.

The photo-based red raven PNG and its standalone HTML preview are design review
assets. The Dock icon has not yet been replaced.
