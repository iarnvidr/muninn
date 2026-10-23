# Muninn 0.2.3 test build

Changes: a unified Library workspace, channel download controls and signed-in
channel discovery, retain-until-watched defaults with 95% completion / one-day
cleanup, optional Classified Terminal themes and selectable accents. Includes a
standalone interactive logo review page; no candidate has been adopted.

Core checks cover completion at the threshold, a stable 24-hour deadline, keeping
protected media, upgrading unchanged old defaults without resetting custom rules,
channel discovery without opt-in, upload-feed polling and correctly serving bundled
theme fonts. 192 unit tests and 15 integration tests pass, along with 33 JavaScript
tests and 14 packaging checks.

Browser checks use the real service/API and a simulated native shell. Verified:
one Library navigation owns collections/tags/tab groups/channels/cleanup; channel
quality, retention, check interval and limit save to the real API; theme and accent
save; all three bundled fonts load. The channel list fixture represents captured
subscription data; a real authenticated Mac session is not available here.

The default migration clears age expiries only for matching copies of unchanged
old shipped rules. Positive custom age limits remain. Disk pressure can evict
temporary media before playback; kept media is protected. Downloads/polls run only
while Muninn is open. Discovery adds candidates, not automatic download consent.

This Mac build is cross-compiled on Linux and verified for arm64 helpers and ad-hoc
signature integrity. Native WKWebView PiP, signed-in channel discovery and the actual
Mac appearance need user testing. The app is not notarized.
