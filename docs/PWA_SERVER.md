# Muninn server and PWA

Muninn can also run as a standalone server with a browser app at `/app/`. This is
an **additional, experimental client** for a server-owned library. The macOS app,
its native YouTube tabs and its existing installation remain available.

The initial web client lets you browse the server library and saved feed
snapshots, search for videos, open a YouTube video URL, queue downloads, play media
from the server, keep a copy and copy source links. It can be installed on a phone
or desktop as a Progressive Web App (PWA).

This is a source-build preview. The macOS DMG does not install or start this server.
There is no hosted Muninn account service, automatic desktop-to-server sync or
multi-user separation. Every paired browser has access to the same library,
downloads and watch progress.

## Build and run locally

Use a checkout containing the server/PWA changes. Install Git, rustup and a C
compiler; the repository's `rust-toolchain.toml` selects the Rust toolchain and
`Cargo.lock` pins dependencies. On macOS, install Xcode Command Line Tools. On
Linux, install your distribution's compiler/build tools and `pkg-config`.
Building these packages does not require the desktop WebKit/GTK shell or a
frontend npm installation.

From the repository root:

```sh
cargo build --locked --release -p muninn-server --bin muninn-service
cargo build --locked --release -p muninn-cli --bin muninn-cli
```

Install **FFmpeg and Deno** on the server and make them available to the account
running Muninn. Install a current **yt-dlp**, or allow Muninn's existing helper
bootstrap to obtain it. For example, with Homebrew on macOS:

```sh
brew install ffmpeg deno yt-dlp
```

On Linux, use your distribution's FFmpeg package and the official
[Deno installation instructions](https://docs.deno.com/runtime/getting_started/installation/)
and [yt-dlp installation instructions](https://github.com/yt-dlp/yt-dlp/wiki/Installation).
Check the actual service account can run the tools, not just your interactive
shell:

```sh
ffmpeg -version
deno --version
yt-dlp --version
```

When no usable yt-dlp is found, Muninn downloads the official standalone helper
and verifies it against the release checksums. A copy owned by Muninn under
`<data-dir>/bin` can update in the background, at most once a day. A working
system installation is left alone. This bootstrap does **not** install FFmpeg
or Deno. Helper downloads and YouTube requests require outbound network access;
playing an already cached copy only requires a connection to this server.

Start a separate library on the same machine as your browser:

```sh
./target/release/muninn-service --server \
  --bind 127.0.0.1:47872 \
  --data-dir "$HOME/.local/share/muninn-server"
```

In another terminal, read the token file printed by the server:

```sh
cat "$HOME/.local/share/muninn-server/token"
```

Open **http://127.0.0.1:47872/app/**, paste that token into the connection form and
connect. Use this exact hostname: `localhost` and `127.0.0.1` are different
origins. The token is submitted in an authorization header and exchanged for an
HttpOnly session cookie; it is not a URL parameter or a saved browser setting.
Do not include it in links or screenshots.

The server prints the token's **file path**, not its value. The token is replaced
on every restart, and browser sessions expire after seven days or a restart,
whichever comes first. **Sign out** revokes the current browser's session.
Restarting revokes all browser sessions. Stop a foreground server with Ctrl+C.

### Server options

| Option | Meaning |
| --- | --- |
| `--server` | Explicitly enable the additional browser server. Without it, the existing localhost service behavior remains. |
| `--bind ADDRESS:PORT` | Numeric listening address; defaults to `127.0.0.1:47872`. Use this instead of the legacy `--port` flag. |
| `--public-origin ORIGIN` | Exact browser origin, such as `https://muninn.example.com`, including a port if nonstandard. No path, query or credentials. |
| `--data-dir DIR` | Persistent server library, databases, helpers and token. Use an absolute path. |
| `MUNINN_SERVER_DATA_DIR` | Server data directory when `--data-dir` is omitted. It is separate from the native/CLI `MUNINN_DATA_DIR` setting. |

Without either data-directory override, server mode uses
`$XDG_DATA_HOME/muninn-server`, or `~/.local/share/muninn-server` when
`XDG_DATA_HOME` is unset, including on macOS. This intentionally differs from the
native app's directory. An instance lock refuses to start a second service using
the same directory.

## Connect a phone or another computer

Use a dedicated **HTTPS origin** and put a reverse proxy in front of the server.
The simplest arrangement keeps Muninn on loopback and terminates HTTPS on the
same host. Configure DNS and a certificate for the hostname you actually use.

```sh
./target/release/muninn-service --server \
  --bind 127.0.0.1:47872 \
  --public-origin https://muninn.example.com \
  --data-dir "$HOME/.local/share/muninn-server"
```

The matching [Caddy example](../build/server/Caddyfile.example) is:

```caddyfile
muninn.example.com {
    reverse_proxy 127.0.0.1:47872
}
```

Replace `muninn.example.com` in **both** places. The browser URL is then
`https://muninn.example.com/app/`. Use the token from this server's data directory
to pair each device. HTTPS sessions use the `Secure` cookie attribute.

Caddy's HTTP upstream preserves the incoming `Host` header by default; keep
that behavior. Muninn validates `Host` and browser `Origin` against the configured
public origin and does not infer trust from `X-Forwarded-Host`. See
[Caddy's reverse-proxy header documentation](https://caddyserver.com/docs/caddyfile/directives/reverse_proxy#headers).
Proxy the **whole origin**, including `/api/`, `/media/`, `/stream/` and `/thumb/`;
serving only `/app/` or rewriting it under another path is unsupported. Do not
add a shared response cache for authenticated routes or log authorization/cookie
headers.

The same origin rule applies to a private VPN or tailnet: use a hostname with
HTTPS trusted by the device and preserve the browser's Host and Origin through
the proxy. A phone visiting plain `http://192.168…` is not equivalent to local
development. Browsers require a secure context for PWA installation and service
workers, with a loopback exception for development
([MDN installability requirements](https://developer.mozilla.org/en-US/docs/Web/Progressive_web_apps/Guides/Making_PWAs_installable)).

If the reverse proxy runs elsewhere, `--bind 0.0.0.0:47872` or a specific network
address requires an explicit HTTPS `--public-origin`. Restrict that port to the
proxy on a trusted network. Muninn does not terminate TLS itself. The separate
internal localhost listener recorded in `service.json` is for the local CLI and
media helpers; never expose or reverse-proxy that listener.

### Linux service example

[build/server/muninn-server.service](../build/server/muninn-server.service) runs
the binary as a dedicated `muninn` account and keeps persistent data in
`/var/lib/muninn-server`, mode `0700`. It expects helper executables in system
locations such as `/usr/local/bin` or `/usr/bin`, rather than another user's home.

After building, on a systemd host:

```sh
sudo useradd --system --user-group --home-dir /var/lib/muninn-server \
  --shell /usr/sbin/nologin muninn
sudo install -m 0755 target/release/muninn-service /usr/local/bin/muninn-service
sudo install -m 0755 target/release/muninn-cli /usr/local/bin/muninn-cli
sudo install -m 0644 build/server/muninn-server.service \
  /etc/systemd/system/muninn-server.service
sudo systemctl edit --full muninn-server.service
```

In the service, replace `https://muninn.example.com` with your origin before
starting it. If the account already exists, reuse it instead of running
`useradd` again. Install and configure your HTTPS proxy separately.

```sh
sudo systemctl daemon-reload
sudo systemctl enable --now muninn-server.service
sudo systemctl status muninn-server.service
sudo -u muninn cat /var/lib/muninn-server/token
```

Read operational logs with `journalctl -u muninn-server.service`. Check helper
availability as `muninn` with `sudo -u muninn ffmpeg -version` and
`sudo -u muninn deno --version`. Stop with
`sudo systemctl stop muninn-server.service`; the binary handles SIGTERM as well
as Ctrl+C. These are deployment examples, not an automatically installed service
or a published container image.

## Install the web app

First open the HTTPS `/app/` URL and connect. You can keep using Muninn in an
ordinary browser tab; installation adds an app icon and a standalone window.

- **iPhone/iPad:** in Safari, open the Share menu (under More in some layouts),
  choose **Add to Home Screen**, enable **Open as Web App** if shown, and tap
  **Add**. See [Apple's instructions](https://support.apple.com/guide/iphone/iphea86e5236/ios).
- **Android:** in a supporting browser such as Chrome, use its menu's
  **Install app** or **Add to Home screen** action. The wording and install prompt
  vary with the browser.
- **Desktop:** use the browser's install button or app-install menu. Muninn's
  **Install app** button opens a native install prompt where supported and
  otherwise shows platform instructions.

The installed app may have a separate session from the original browser tab;
pair it again if asked. Browser support and install UI vary; see
[MDN's installation guide](https://developer.mozilla.org/en-US/docs/Web/Progressive_web_apps/Guides/Installing).

### Server cache and device offline use

**Downloaded videos live on the server.** Closing the PWA does not stop an
already queued server download. **Keep** protects a server copy from automatic
cache eviction; it does not download a second copy to your phone.

The service worker caches the app shell only. It does not cache session
responses, library/feed data, thumbnails or video/audio files. The installed app
can open its shell without a network connection, but browsing and playback need
a reachable, running Muninn server. There is no device-offline media download in
this initial version. Normal server retention and cleanup policies still apply
to unprotected copies.

## Where the library and feeds come from

A new server begins with an empty library. Use **Find video** to search YouTube
or open a video URL, then queue a download. Completed media is shared by all
paired devices. The **Downloads** page shows work running on the server.

The **Feeds** page reads snapshots already stored in this server's content
database and shows their source context. It does not capture other browser tabs
or connect to the native app's Google sign-in. Personalized Home and Subscription
capture still belongs to the native app. Server-side public channel polling can
be configured through the existing local CLI; it is not a Google account sync.

The server also keeps a private localhost API for that CLI. On the server host,
use the same account and data directory:

```sh
./target/release/muninn-cli --data-dir "$HOME/.local/share/muninn-server" library
```

For the systemd example:

```sh
sudo -u muninn muninn-cli --data-dir /var/lib/muninn-server library
```

See the [CLI reference](CLI.md) for importing links and existing media, channel
subscriptions, retention rules and cleanup. Local administrative capabilities
are not exposed by the PWA's restricted API.

## Data, migration and updates

The macOS app and standalone server are independent installations. There is no
automatic migration, shared login store or bidirectional sync in this version.
Use the local CLI's `import links` or `import media` commands to populate the
server deliberately; `import media --dry-run` previews a media import and the
default import copies its source file. The PWA does not upload local files.

Do not point both processes at one live data directory or copy a live SQLite
database between them. Existing media rows can contain absolute file paths, so
moving only the database to another machine is not a library migration.

For a consistent server backup, stop the service and back up its **whole data
directory**, including `db/`, `library/` and `cache/`. Treat the backup as private
library data. Restore to the same absolute path and service account unless you
are explicitly reimporting media into a new location.

To update this initial source installation, build the desired revision, stop the
service, back up the data directory, replace `muninn-service` (and the matching
CLI), and start it again. Keep the data-directory path and public origin stable.
PWA assets are embedded in the binary; reload the app to pick up the update and
pair again using the newly generated token. Neither uninstalling the PWA nor
clearing a browser's site data deletes the server library.

## First-run checks and limitations

Confirm these on your own host/device before relying on it:

1. The exact `/app/` origin loads and a wrong token cannot connect.
2. The correct token connects; signing out makes the library inaccessible.
3. A search or a pasted video URL can queue a download with the installed helpers.
4. A completed copy plays and seeks in the device's browser; codec and HLS support
   differ across browsers. Installing the PWA does not change browser media support.
5. Restarting the service preserves the library and requires pairing again.
6. Taking the device offline shows the connection message instead of promising
   offline video playback.

This first version does not include native YouTube webviews, interception/capture,
desktop tab management, a feed graph, user accounts, per-device permissions or a
remote administration console. It reuses the existing downloader and media
pipeline, so upstream restrictions and helper failures can still affect online
search and acquisition. Automated checks do not establish a live YouTube or
physical iOS/Android playback result. See [SECURITY.md](../SECURITY.md) for the
server trust boundary and private vulnerability reporting.

### Developer checks

The existing server and UI checks include the new API boundary, session,
feed-source and service-worker regressions:

```sh
cargo test --locked -p muninn-server -p muninn-ui-assets
node --test ui/test/pwa.test.js
```

`e2e/pwa-server.test.js` additionally starts the real server in an isolated
temporary directory and drives Chromium through pairing, MP4 and HLS playback,
seeking, saved-position restoration, source links, downloads, search, sign-out
and an offline reload. It generates media with FFmpeg and uses a synthetic
yt-dlp executable, so it does not depend on live YouTube or personal data.
With Playwright and its Chromium browser installed, plus FFmpeg and Python 3:

```sh
cargo build --locked -p muninn-server --bin muninn-service
node --test e2e/pwa-server.test.js
```

Set `CHROMIUM_PATH` to use an existing Chromium executable, `MUNINN_SERVICE`
to test a different server binary, or `PWA_SCREENSHOTS` to capture the tested
views. The browser smoke test is an explicit local check; the standard CI jobs
run the Rust and JavaScript regressions without downloading a browser.
