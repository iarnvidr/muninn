# Muninn CLI and automation

Offline mode and favourites use `settings set` too; see
[channel caching, favourites and offline mode](CHANNELS_OFFLINE.md).

The CLI is part of the same app executable. Start Muninn normally, then use:

```bash
muninn() { /Applications/Muninn.app/Contents/MacOS/muninn --cli "$@"; }
muninn help
muninn status
muninn logs
muninn entries
muninn tabs
```

All successful results are JSON. Errors go to stderr and exit nonzero. No pairing
step or token arguments: the client finds the running app through its private data
directory. `--data-dir DIR` or `MUNINN_DATA_DIR` selects a different instance.
The CLI never starts another service or edits a running app's SQLite databases.
On Linux, `cargo run -p muninn-core --bin muninn-cli -- …` runs the same client.

## Cache, keep and cleanup

```bash
muninn cache dQw4w9WgXcQ 480
muninn keep dQw4w9WgXcQ.480
muninn release dQw4w9WgXcQ.480 7
muninn cleanup preview
muninn cleanup run
muninn delete dQw4w9WgXcQ.480
muninn retry dQw4w9WgXcQ.480
muninn cancel dQw4w9WgXcQ.480
```

By default, copies remain until playback reaches 95% of the duration, then their
media and temporary HLS files are removed after 24 hours. The threshold and grace
period are editable in **Library → Playback & cleanup**, or as `completion_percent`
and `cleanup_delay_seconds` through the automation API/CLI. A real ended event also
counts. Seeking near the end counts when player progress is reported; this is not
a measurement that every second was watched. Later progress reports above the
threshold preserve the deadline. Reopening or replaying below the threshold resets
completion; active playback/downloads defer deletion. Metadata and history remain.

Rule/channel `retention_days: 0` means until watched, with no age expiry. Positive
days add a maximum age. The old unchanged shipped rules and their matching cached
copies upgrade to the new default; custom policies stay intact. Temporary copies
can still be evicted to satisfy disk limits. Kept copies are protected.

**Keep video** in the watch view and `keep` in the CLI protect the media from all
automatic cleanup, including age-based eviction. Local media imports are protected
by default. `release ID DAYS` returns a kept file to the temporary cache and sets
its expiry; it can then be cleaned by the completion policies. `delete` refuses protected entries; deliberate deletion of a kept copy
is available through the existing Library UI or `api POST /api/entries/ID/evict?force=1`.

Export and change policies using files or stdin:

```bash
muninn automation get > policies.json
muninn automation set policies.json
muninn rules get > download-rules.json
muninn rules set download-rules.json
muninn settings get > settings.json
muninn settings set settings.json
```

`automation set` replaces the completion configuration. `rules set` replaces the
ordered download rules; `settings set` patches global settings. The app's Rules
page edits the same configuration. Example completion policy:

```json
{
  "cleanup_completed": true,
  "cleanup_delay_seconds": 300,
  "completion_rules": [
    {
      "name": "Keep music from this channel",
      "match": {
        "channels_only": true,
        "channels": ["UCxxxxxxxxxxxxxxxxxxxxxx"]
      },
      "action": "keep"
    },
    {
      "name": "Remove completed news",
      "match": {"title_keywords": ["news", "daily briefing"]},
      "action": "delete"
    }
  ]
}
```

First matching completion rule wins; the global `cleanup_completed` setting is
the fallback. Match fields also include `min_duration_seconds` and
`max_duration_seconds`. Unknown duration cannot satisfy duration bounds. A
channel list only restricts matching when `channels_only` is true. These rules
are declarative JSON; arbitrary code is not executed inside the app. Use shell,
Python or other scripts with the CLI for logic beyond the built-in matcher.

## Automatic channel downloads

```bash
muninn subscriptions get > channels.json
muninn subscriptions set channels.json
muninn subscriptions poll
```

Example `channels.json` (replace the placeholder with a real YouTube channel ID):

```json
[
  {
    "channel_id": "UCxxxxxxxxxxxxxxxxxxxxxx",
    "quality": "480",
    "retention_days": 0,
    "keep": false,
    "enabled": true,
    "poll_minutes": 60,
    "max_per_poll": 3
  }
]
```

**Library → Channels** has per-channel quality, polling interval, download limit,
retention and enable controls. **Load subscribed channels** loads YouTube's channel
list through the existing signed-in desktop session. Discovered channels are
candidates; only the channels you explicitly add get automatic downloads. A manual
channel ID/URL works too. `muninn channels` lists discovered channels and check status;
`muninn feeds refresh channels` requests discovery from the signed-in session.

The app checks public upload feeds while running, so polling itself does not need
Google sign-in. `subscriptions poll` checks enabled channels immediately, regardless
of their normal interval. Global `auto_prefetch` must be enabled.
The first check queues only the newest `max_per_poll` items; older uploads are
marked seen so the app does not gradually download the entire channel history.
The public feed exposes a limited recent-upload window, so this is not a complete
archive/backfill service. Quality is `audio`, `360`, `480`, `720` or `1080`. Set `keep` to
true to protect channel downloads; otherwise completion and expiry policies apply.

### Channel groups and inline video lists

Library → Channels is a full-page table. Select channels and apply a group, then
edit that group's shared quality, retention, polling interval and upload limit.
Each channel belongs to at most one group. A per-channel override takes precedence
over the group's complete policy; reset the override to inherit future group edits.
Applying a group through the UI clears overrides for the selected channels. Existing
downloaded copies are not upgraded or removed when group policies change.

```bash
muninn channel-groups get > channel-groups.json
muninn channel-groups set channel-groups.json
muninn subscriptions effective
muninn channel-videos UCsBjURrPoezykLs9EqgamOA library
muninn channel-videos UCsBjURrPoezykLs9EqgamOA uploads
muninn api GET '/api/channels/UCsBjURrPoezykLs9EqgamOA/videos?source=uploads&offset=25'
```

`channel-groups set` replaces all groups. Each object has `name`, `channel_ids`
(array), `quality`, `retention_days`, `keep`, `enabled`, `poll_minutes` and
`max_per_poll`, using the same policy values as subscriptions. Example:

```json
[{"name":"Learning","channel_ids":["UCsBjURrPoezykLs9EqgamOA"],"quality":"1080","retention_days":0,"keep":false,"enabled":true,"poll_minutes":60,"max_per_poll":3}]
```

Expand a channel to switch between **In Muninn** and **Channel videos**. Public
uploads load 25 at a time with Load more, Play and Download actions. Listing fetches
metadata only; Download uses the displayed channel quality and Keep flag. These
manual downloads use normal cache rules for age retention. Automatic downloads use
the full effective channel policy. Upload listings are cached for 30 minutes;
Refresh bypasses that cache. The listing uses the creator's Videos tab, not Shorts
or live archives. Library matching uses captured channel IDs, falling back to channel
names for older imports; imports without either identity cannot be matched.

## Import existing videos

### Import a large browser video collection without opening tabs

```bash
muninn import links youtube-links.txt --collection 'Imported links' --dry-run
muninn import links youtube-links.txt --collection 'Imported links'
muninn library
muninn library remove dQw4w9WgXcQ
```

The input is one YouTube video URL per line, a JSON array of URLs, or an array
of objects such as `{"url":"https://youtu.be/dQw4w9WgXcQ","title":"Saved title"}`.
Video IDs are deduplicated across URL forms. Existing links can join additional
collections. Import validates the entire batch before saving; `--dry-run` changes
nothing. The response includes `tabs_opened: 0` and `downloads_queued: 0`.

These are durable library references, independent of the expiring content cache.
They appear under Library → Videos → Links and the selected collection. There are
no tabs, renderers, download jobs, rule evaluations or video/metadata fetches during
import. Thumbnails may load when viewing Library. Supplied titles work offline;
captured metadata improves presentation when available. Play opens one player tab
and starts normal playback/caching. Download 480p queues a copy explicitly; the CLI
`cache VIDEO_ID QUALITY` supports other qualities. Remove deletes the reference and
its collection/tag memberships, leaving existing media untouched.

Use `import tabs` or `open` only when tabs are actually wanted: those commands
create unloaded tabs that still occupy the tab bar. They do not import into Library.

### Import media files

```bash
muninn import media '/Volumes/Videos/Example.mp4' --video-id dQw4w9WgXcQ --dry-run
muninn import media '/Volumes/Videos/Example.mp4' --video-id dQw4w9WgXcQ
muninn import media '/Volumes/Videos/Example.mp4' --metadata '/Volumes/Videos/Example.info.json' --enrich
muninn import media '/Volumes/Videos/Example.mp4' --video-id dQw4w9WgXcQ --move
```

Files are copied into the protected library without re-downloading media. If a
matching `.info.json` exists beside the video, its YouTube ID, title, channel,
description, duration, height and thumbnail URL populate the presentation. An
explicit ID overrides the sidecar ID. `--enrich` requests current watch-page data
and comments in the background; failures do not undo a successful media import.
It requires network access and is subject to YouTube availability.

`--dry-run` validates and reports the target without copying or deleting.
`--move` removes the source only after the new file is synced and registered;
`source_removed` reports whether removal succeeded. The sidecar is left alone.
Duplicate video/quality entries and existing target files are refused, never
overwritten. Batch imports by invoking this command from your own script.
No transcoding is performed; the container and codecs must be playable by the
Mac web engine. In particular, importing an MKV does not make it Safari-compatible.

## Import and open tabs

```bash
muninn open 'https://www.youtube.com/watch?v=dQw4w9WgXcQ'
muninn import tabs urls.txt
muninn import tabs urls.json
printf '%s\n' 'https://www.youtube.com/watch?v=dQw4w9WgXcQ' | muninn import tabs -
```

Input is one YouTube URL per line or a JSON array of URL strings. Existing open
URLs and duplicates are skipped. Imports appear as sleeping tabs within three
seconds, without creating one renderer or download per imported URL. A restart
also restores them. Other collection formats can be converted by your script.
For a tab-import dry run, send `{"urls":["…"],"dry_run":true}` through the API.

## Scripting the API and logs

```bash
muninn api GET /api/entries
muninn api GET /api/cleanup
muninn api POST /api/import/tabs batch.json
muninn api PUT /api/automation policies.json
muninn api GET '/api/logs?limit=1000' > diagnostic-logs.json
```

Logs persist in `~/Library/Application Support/io.github.iarnvidr.muninn/logs/`.
Two rotating JSONL files hold roughly 4 MiB total. Events include app lifecycle,
watch opens, completion, download outcomes, imports, cleanup, channel checks and
HTTP failures. Entries contain IDs and counts; request bodies, query strings,
cookies and API tokens are not logged. With Developer mode on in Settings, Diagnostics includes a log viewer.
Proxy capture diagnostics remain separate. These are application events, not
a full WebKit/network trace or native crash reporter.

## Tab groups, collections and tags (0.2.2)

**Library → Collections & tags / Tab groups** creates, renames and removes named tab groups, library
collections and video tags. Assign individual tabs or select several to move
together. Group labels in the tab strip collapse background tabs while keeping
the active tab accessible. Closing a group closes its current tabs; removing a
group only removes its grouping. The organization survives app restarts.

Library adds collection/tag filters, title/channel/size/progress sorting, and
per-video collection/tag assignment controls. Videos can belong to several
collections and carry several tags. These labels apply to the video across all
its cached qualities. Removing a tag or collection never deletes media. Labels
remain attached to the video ID if its temporary media cache is removed.

```bash
muninn group create Research
muninn group assign Research 1 2 3
muninn group collapse Research
muninn group expand Research
muninn group rename Research 'Weekend research'
muninn group remove 'Weekend research' 2
muninn collection create 'Watch later'
muninn collection add 'Watch later' dQw4w9WgXcQ
muninn tag create Music
muninn tag add Music dQw4w9WgXcQ
muninn tag rename Music 'Music videos'
muninn tag remove 'Music videos' dQw4w9WgXcQ
muninn organization get > organization.json
muninn organization set organization.json
```

`collection` and `tag` accept multiple video IDs with `add`/`remove`; `group`
accepts multiple tab IDs with `assign`/`remove`. Each also supports `create`,
`rename` and `delete`. Get tab IDs with `muninn tabs`. The shared authenticated
API is `GET`/`PUT`/`POST /api/organization`. POST applies one atomic change, e.g.
`{"kind":"tag","action":"add","name":"Music","ids":["dQw4w9WgXcQ"]}`.
PUT replaces the entire configuration; POST is preferable for concurrent scripts.

## Watch controls and saved-copy management (0.2.2)

The watch view adds ten-second seek buttons, speeds from 0.25× to 4×, fit/fill,
loop, theatre layout and picture-in-picture where supported by the web engine.
The source selector plays a completed copy from disk or queues another quality.
It does not seamlessly switch an active upstream stream. Looping prevents the
normal player end event, so looping video is not marked completed automatically.

Settings → Playback controls the floating-player lifecycle:

- `pip_mode`: `manual` (click to open, the default), `automatic` (request PiP when
  leaving the watch view), or `disabled`.
- `background_playback`: continue audio while browsing even without PiP. This
  also controls the fallback if an automatic PiP request is rejected. Default false.
- `pip_return_to_inline`: close PiP when returning to the video's watch view.
  Default true.
- `pause_when_pip_closes`: pause background playback when the floating window is
  closed. Set false to continue audio. Default true.

The same video element and source stay alive in the sidebar mini-player while
browsing. It has a preview, seek, pause/play, PiP and return controls, including
compact controls with the sidebar collapsed. Automatic PiP failure follows the
background playback fallback above. Settings save automatically; changing PiP
mode also applies to a video opened before the change.
Opening another video replaces the previous source;
closing the tab that owns playback stops it. Automatic PiP is requested inside
navigation clicks where possible. Some web engines reject automatic requests
without user activation (for example navigation originating outside the page);
the configured background fallback then applies, with a visible message.

```bash
printf '%s' '{"pip_mode":"automatic","background_playback":true,"pip_return_to_inline":true,"pause_when_pip_closes":true}' | muninn settings set -
```

Each completed copy has **Play this copy**, retention, **Create another quality**
and **Delete copy** controls. A lower-quality manual request creates the selected
quality even if a sharper copy exists. Creating another quality preserves the
old copy; delete it after the new one finishes. If the original is kept, the new
copy is kept too. Resolution labels show actual stored height, with the requested
height limit separately when they differ. Temporary retention still allows the
normal completion cleanup policy; choose indefinite Keep to prevent it.

Long comments expand with Show more. Reply counts are interactive, with their
own cached pagination separate from top-level comments. Refresh comments restores
an expired/missing continuation; loading more merges and deduplicates pages.
Replies require a currently available YouTube reply link, and unavailable or
expired links report an error rather than silently doing nothing.

## Verification and limits

The implementation has automated core/API/CLI tests for completion cleanup,
protection, import safety, tab deduplication, channel polling, and JSON CLI output
with nonzero errors. Google sign-in is now reported working on the user's Mac.
Native view bounds, PiP behavior and authenticated subscription capture still
need confirmation on a Mac.

## Tab ordering and channel discovery (0.2.4)

Library Play opens a new player tab. Browse and Player have separate sidebar
destinations. Dragging in the top strip or Library → Tab groups persists through
the shared organization API. Scripts can perform the same atomic move:

```sh
printf '%s' '{"kind":"tab","action":"move","name":"Research","ids":["3"],"before_id":"1"}' | muninn api POST /api/organization -
```

Use an existing group name, or an empty name to ungroup. Omit `before_id` to
append. `GET /api/organization` includes `tab_order` and ordered group `tab_ids`.

Loading subscribed channels reads channel IDs and names directly from the signed-in
YouTube channels page as well as passive captures. `GET /api/channels/status`
reports `phase`, `count` and `updated_at`; the interface reports discovery progress,
signed-out, empty or timeout states. The headless service cannot access that
desktop login. No cookies or credentials are copied into the channel list.

## Compatible local playback (0.2.5)

Imported MKV containers prepare a temporary H.264/AAC HLS stream automatically.
Other copies fall back after decoder/format errors. Playback options also has
**Use compatible local playback**. It uses the bundled ffmpeg on the local file (H.264 through Apple's VideoToolbox, so it needs a Mac that provides it),
keeps the original untouched and starts playback as segments become available.
Seeking is limited to the portion prepared so far. Preparing stops when playback
is closed or another video replaces it. Temporary output obeys a separate cap of
one quarter of the configured disk budget and the minimum free-space setting.

Scripts use the same authenticated API:

```sh
muninn api POST /api/entries/VIDEO_ID.720/playback
muninn api GET /api/entries/VIDEO_ID.720/playback
muninn api POST /api/entries/VIDEO_ID.720/playback/stop
```

Player format/decode errors are included in rotating logs and diagnostic exports
under `ui.playback.error`; local conversion completion/failure uses `playback.local`.
Ten seconds without playback progress records `ui.playback.stall` and tries the
same local compatibility path for file playback. `media.served` records original
file response status and ranges. The schema 2 diagnostic report includes recent
copy containers, file existence/size, channel-discovery state and codec probes
of the three most recent complete copies, each limited to three seconds. It does
not include media or file paths.

## Diagnostics for local Codex

In the app, turn on Developer mode in Settings and use **Export diagnostics** to save the same ZIP
(default name `muninn-diagnostics-<date>.zip`) through a save dialog. The CLI export below is unchanged.

`--cli feeds refresh subscriptions` (or `home`) requests a refresh through the
desktop app's persistent signed-in YouTube session. It creates or reuses a
background tab and leaves the current player/page active. This command needs
the desktop app; the standalone headless service cannot access browser cookies.
The Feeds screen refreshes automatically when Subscriptions has no snapshots,
and also has an explicit refresh button. Google sign-in has been reported
working on the user's Mac; authenticated capture still needs verification there.

Collect a package even when the app is closed or has crashed:

```sh
/Applications/Muninn.app/Contents/MacOS/muninn --cli diagnostics export "$HOME/Desktop/muninn-support.zip"
```

Unzip it, start Codex in that folder, and ask it to read `AGENTS.md` and diagnose
the report. The package includes the current app version, platform, settings,
download state counts, 50 recent download failures and up to 4 MiB of rotating
structured logs. The command does not overwrite an existing package. It opens
the video index read-only and also works if that database is unavailable.

It excludes media, raw databases, browser cookies/storage, pairing tokens and
certificate keys. Known credentials, credential fields, URL queries and home
paths are redacted. Error messages and video IDs may still reveal viewing
activity: review the package before sharing it. Nothing is uploaded automatically.
The included agent instructions cover CLI inspection and require asking before
changing settings or deleting data. For live troubleshooting, `--cli logs`,
`--cli jobs` and `--cli status` return JSON while Muninn is running.
