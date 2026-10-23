# Player keep point

The timeline marks the position where the current copy reaches the configured
watched threshold. The default is 95%; the percentage and cleanup delay still
come from Library → Playback & cleanup. This point starts the completion cleanup
grace period, rather than deleting the copy at that instant.

Once playback reaches the point, a compact prompt inside the video frame offers
**Keep** and a dismiss button. It does not pause playback, steal keyboard focus or
change the source. Keep protects the copy currently playing through the existing
promotion API; it does not queue a new quality or download. A failed request keeps
the prompt available for retry. Dismiss suppresses the reminder for this viewing
session and leaves the cleanup policy unchanged.

Protected copies, matching keep rules, disabled completion cleanup and unknown
durations have no cleanup marker or prompt. If playback crosses the point in the
background, the prompt can appear when returning to the full player. Native OS
PiP does not display the app's HTML overlay.

The watch-status response supplies the effective completion policy with rules
resolved by the backend. Playback-report responses refresh it as playback
continues. Marker placement uses the same rounded duration and integer position
as the playback reporter. Cleanup, playback and promotion behavior are otherwise
unchanged.

## Verification, 2026-10-17

- All 194 core library tests passed, including current policy values, disabled
  cleanup and a matching keep rule in the watch-status response.
- All 40 JavaScript UI/injection checks passed, including threshold rounding,
  configured percentages and unknown/disabled/keep cases.
- Production-player Chromium checks used an isolated synthetic MP4, API fixture
  and native-shell mock. They verified marker placement, crossing the point,
  in-frame prompt geometry, dismissal, retry after a failed save, and protecting
  the exact playing copy even when another copy was already protected.
- Additional browser checks passed for changing/raising the threshold, disabling
  cleanup, a keep rule, fullscreen, and clicking Keep without pausing playback.
- [Screenshot](../output/playwright/player-keep-point.png) uses synthetic media
  and account-free fixtures. Native macOS behavior was not tested. This feature
  needs a new app build before it appears in an installed copy.
