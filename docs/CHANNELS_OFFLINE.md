# Channels, favourites and offline mode

Channel video pages persist on disk. Opening a cached page displays it immediately;
after **Settings → Freshness → Channel video lists** (24 hours by default), Muninn
refreshes it in the background. Refresh keeps the previous list visible. Failed
refreshes preserve the cached page. Previously opened pages also work offline;
pages never fetched require a connection. Reopening the channel panel remembers
its source and avoids repeating recent requests.

In **Library → Channels**, select a group and enable **Favourite group**. It
defaults to downloading new uploads using the group's quality and retention rule.
Individual channel overrides still win. The global automatic-download switch
also applies; disabling it does not disable the favourites carousel.

**Settings → Favourites** controls inherited downloads, the Browse carousel,
removal on first playback, and its lifetime (five days by default). New uploads
leave the carousel after actual playback starts or their lifetime elapses,
whichever comes first. Opening or seeking without playing does not count.
Feature placement never protects media from cleanup. The first channel check
considers only the newest uploads up to the group's configured limit.

**Unsubscribe** removes the YouTube subscription through the existing signed-in
browser session, then removes Muninn's channel override and group membership.
Existing library videos remain. Offline, signed-out or unconfirmed requests do
not change local policies. Open YouTube in Browse first if no live signed-in page
exists. An ambiguous response asks you to refresh subscriptions before retrying.
The account action is fixture-tested; live YouTube and native macOS behaviour
still require verification. No account was modified by tests.

## Offline mode

Enable **Settings → Network → Offline mode** to use cached content without
remote browsing, catalogue fetching or new download requests. The status bar
shows **Offline mode · cached content only** and a **Reconnect** button.

Remote browser tabs sleep. Active network downloads stop and return to the queue
after their workers exit; they resume when online. Explicitly cancelled jobs stay
cancelled. Local media and compatibility conversion remain available. Requests
already in flight may finish while workers stop; this setting is an application
policy, not an operating-system firewall. Cached pages, thumbnails and metadata
can be read even when stale. Normal disk limits and cleanup still apply.

Both toggles persist and use the same API as the CLI:

```sh
printf '%s' '{"offline_mode":true}' | muninn settings set -
printf '%s' '{"offline_mode":false}' | muninn settings set -
```

The offline switch does not disconnect the localhost service, diagnostics or CLI.

## YouTube sign-in

| Session | Browsing, groups and downloads | YouTube subscriptions and unsubscribe |
| --- | --- | --- |
| Not checked | Cached content and public channel lists work | Cached subscriptions are labelled; account actions disabled |
| Signed out | Public browsing, favourite groups, carousel and configured public downloads continue | Sign-in action shown; subscription loading and unsubscribe disabled |
| Signed in, identity unavailable | Public features continue | Reload/open YouTube to verify before account actions |
| Signed in, verified identity | Public features plus YouTube's personalized native browsing | Load this account's subscriptions; unsubscribe only a matching account's discovered channel |
| Offline | Cached pages and local playback; downloads wait | No remote account actions; reconnect requires a fresh sign-in observation |

Cached subscriptions never establish sign-in. Account state comes from the native
YouTube page and resets to unknown on app startup, opening sign-in, or entering
offline mode. Subscriptions are associated with a one-way hash of the page's
account context, not cookies or an exported account identifier. Signing out or
switching accounts preserves local groups, media and download policies while
disabling actions on another account's cached subscriptions. A same-document
account switch requires reloading subscriptions so old page contents cannot be
attributed to the new account. Older background-page observations are ignored.

Unsubscribe verifies the expected account again inside the YouTube page before
sending the request and before applying local policy changes. Failed or ambiguous
changes preserve local settings. Subscription lists and unsubscribe markers are
account-bound; the app does not automatically subscribe or unsubscribe channels
when login changes. From 0.2.15, download and playback helpers also receive the
native browser's YouTube session; see [YOUTUBE-SESSION.md](YOUTUBE-SESSION.md).
