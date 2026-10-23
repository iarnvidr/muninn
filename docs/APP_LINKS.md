# Open YouTube links from outside Muninn

External applications and scripts can open a YouTube URL in Muninn. On macOS:

```sh
open -a Muninn 'https://www.youtube.com/watch?v=dQw4w9WgXcQ'
```

Muninn also registers its own URL scheme, suitable for clickable links in another
application:

```text
muninn://open?url=https%3A%2F%2Fwww.youtube.com%2Fwatch%3Fv%3DdQw4w9WgXcQ
```

```sh
open 'muninn://open?url=https%3A%2F%2Fwww.youtube.com%2Fwatch%3Fv%3DdQw4w9WgXcQ'
```

The app starts if needed, opens a new active tab and brings its window forward.
An already-running app receives the link through macOS opened-URL events or the
single-instance argument handler. Library or Settings yields to the requested tab;
the request survives a chrome page that has not loaded yet. Closing the Mac window
hides the app, so another outside-open action restores it.

Videos use Muninn's own player and normal playback/cache rules. Channel, feed,
playlist and search pages open in a browsing tab. This is an individual open
action. For large collections of links, use `--cli import links` to populate the
library without opening tabs or downloading videos. `--cli open` remains the
separate operation that creates sleeping tabs in an already-running app.

## Constructing app links

Use `URL` and `searchParams.set("url", youtubeUrl)` in JavaScript:

```js
const appLink = new URL('muninn://open');
appLink.searchParams.set('url', youtubeUrl);
```

Encode the entire target URL, including its query string. The parser preserves
the target parameters; subsequent tab/player navigation uses its existing behavior.
This feature does not add timestamp seeking or playlist playback to that behavior.

Only HTTP(S) YouTube hosts and valid `youtu.be` video links are accepted. App links
accept one `url` parameter and the `open` action. Additional or duplicate action
parameters, embedded credentials, nonstandard ports and other schemes are ignored.
Rejected inputs never invoke a shell or open a fallback browser. The handler does
not register itself as the system's default HTTP(S) application or change another
application's routing settings.

## Packaging and verification

`src-tauri/Info.plist` contains the `muninn` scheme registration. Tauri, native
packaging and the Linux cross-build consume the same file. Install the built
`Muninn.app` in `/Applications` before testing Launch Services.

The parser and packaged registration have automated regression tests. The real
Linux Tauri regression can exercise cold argv delivery, a second executable
launch, a hidden window and leaving an internal page:

```sh
# Disposable E2E container only: this launcher resets its test HOME.
cd /build/e2e
./run-app.sh /cache/target/debug/muninn /usr/bin \
  'muninn://open?url=https%3A%2F%2Fwww.youtube.com%2Fresults%3Fsearch_query%3Dmuninn%2Bhandoff%2Bcold'
python3 app-links.py
```

Linux results do not verify macOS Launch Services. On a Mac, verify the installed
bundle with these actions before claiming native delivery works:

1. Quit Muninn; use each `open` command above and confirm the expected active tab.
2. Leave Muninn running on Library or Settings; repeat and confirm it shows the tab.
3. Close the window; repeat and confirm the window returns with the requested tab.
4. Try a non-YouTube target and confirm it creates no tab or fallback browser.

Native Mac runtime verification is still required when no Mac run is recorded.
