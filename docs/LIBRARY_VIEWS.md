# Library: Browse and Manage

Library → Videos has two modes. Manage remains the initial default; switching
views is remembered on this device. Both modes use the same local library,
collections, tags, filters and existing actions.

## Browse

- A responsive thumbnail grid with video titles, channels, duration and watch
  progress. Link-only items remain visible without starting downloads.
- One card per video. Multiple copies are grouped; the card prefers a completed,
  kept copy, then a completed copy, then an active download. Manage still lists
  every copy separately.
- Enter a search and press Enter or the search button to get horizontal video
  results. Every query word must match the title, channel or identifier. Relevance
  favors title matches; other existing sort orders remain available.
- Search covers the full local library, including videos beyond the currently
  displayed cards. It does not perform a YouTube network search or fetch missing
  metadata for imported links.
- Initially displays 36 videos; Show more adds another 36. New searches and filter
  changes reset that limit.
- Clicking a thumbnail or title opens a new player tab through the existing
  playback action. The three-dot menu exposes existing copy actions and collection
  and tag editing. Keep and Remove apply to the represented copy.

## Manage

The original storage meters, per-copy table, download status, retention, actions
and collection/tag editing remain available. Filtering updates as you type.

## Verification, 2026-10-17

- All 38 JavaScript UI/injection checks passed. New checks cover multiword
  title/channel search, identifier matching, relevance and grouping copies without
  changing the original entry list.
- Headless Chromium exercised the production UI against a disposable synthetic
  API fixture and mocked native shell: Browse/Manage, search ordering and empty
  results, collections, tags, Keep, persisted view selection, a 950px viewport,
  and dispatching Play to a new player tab. No script errors were observed.
- A separate 1,406-video fixture verified initial rendering of 36 cards, 72 after
  Show more, searching an item beyond those cards, and resetting the display limit
  when clearing the search.
- Appearance is recorded in [Browse](../output/playwright/library-browse.png) and
  [search results](../output/playwright/library-search.png), using synthetic data.
- No media pipeline, cache policy or native-shell changes were made. Native
  macOS layout and playback were not tested by these browser checks. These source
  changes require a new app build before they are available in an installed app.
