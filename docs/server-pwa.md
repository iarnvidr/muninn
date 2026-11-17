# Muninn server + PWA (initial additive edition)

This is **another entry point**, not a replacement for the macOS app. `muninn-web`
uses the existing Rust `Core`, download workers, stores and HTTP media implementation.
It serves a separate installable client at `/app/`. Tauri, `/ui/`, `Service::start`,
`muninn-service`, desktop playback, native tabs, and the desktop release channel
are unchanged. There is no migration or automatic cutover.

## Included

- Single-owner server login with an access-token file exchanged for a revocable,
  24-hour HTTP-only session cookie; no token in URLs or browser storage.
- Server library, video-URL download requests, queue status, retry/cancel and feeds.
- Explicit **Save to device** transfers for completed server files. The metadata
  and full media Blob are committed atomically in IndexedDB. **Play local copy**
  uses only that persisted Blob, never an upstream or server streaming fallback.
- Installable, responsive shell, offline device library, local playback position,
  copy-URL buttons and visible feed filters (no kind dropdown).
- Per-file device-copy limit of 512 MiB, cancellation, size validation, quota
  checks and user-requested persistent storage. Server downloads are not limited
  by this browser-copy limit.
- Docker/Compose example and tests in the existing Dagger `serverTests` and
  `javascript` checks. Source assets live in `ui/pwa/`, already included in
  source-scoped Dagger inputs and retained crate fingerprints.

## Local first run

From the repository checkout:

```sh
cargo build --locked -p muninn-server --bin muninn-web
mkdir -p "$HOME/.config/muninn-server"
target/debug/muninn-web --init-token "$HOME/.config/muninn-server/access-token"
target/debug/muninn-web \
  --token-file "$HOME/.config/muninn-server/access-token" \
  --data-dir "$HOME/.local/share/muninn-server" \
  --bind 127.0.0.1:47872
```

Open `http://127.0.0.1:47872/app/`. Enter the token stored in the file (use a local
editor to read it). The token generator creates a new file with Unix mode `0600`
and refuses to overwrite an existing file; the server never logs its value.
The core discovers/bootstrap-installs yt-dlp and related tools using its existing
logic. Initial requests target public YouTube videos; authenticated desktop
YouTube sessions are **not** copied to this server.

Use a **separate data directory**. Never run the desktop and server against the
same SQLite stores or actively written cache files. This initial version does
not synchronize a running macOS library, device metadata, credentials or tabs.

## Remote access and installation

Use an HTTPS reverse proxy or a private HTTPS network endpoint. Set the exact
public origin explicitly, for example:

```sh
muninn-web --bind 127.0.0.1:47872 \
  --origin https://muninn.example \
  --token-file /etc/muninn/access-token \
  --data-dir /var/lib/muninn
```

The proxy must preserve `Host: muninn.example`, terminate TLS and keep the backend
port private. Forwarded headers are not trusted. Mount at the origin root, **not**
a subpath: the client uses `/app/`, `/api/` and `/media/`. Only loopback development
origins allow HTTP; non-loopback listeners require a configured HTTPS origin.
The server is HTTP behind that trusted proxy; declaring an HTTPS origin does not
itself encrypt the backend socket.

Install from the browser's app/install menu. On iPhone, open the HTTPS URL in
Safari and use Share → Add to Home Screen. The shell service worker controls only
`/app/`; it cannot take control of the desktop's `/ui/` pages.

Browser storage is not a backup. The browser/OS can remove device copies or deny
persistent storage. Large transfers require free memory as well as storage in
this first implementation. Device transfers are **foreground-only**, have no
resume across page closure, and do not pretend to be reliable iOS background jobs.
A transfer is not playable until its complete file is durably stored. Unsupported
codecs produce an explicit error; no live stream is substituted.

## Container example (not automatically deployed)

```sh
mkdir -m 700 -p deploy/secrets
(umask 077; openssl rand -hex 32 > deploy/secrets/muninn_web_token)
# The 0700 directory protects the host path; the read-only secret mount must
# be readable by the non-root container user, UID 10001.
chmod 0444 deploy/secrets/muninn_web_token
export MUNINN_WEB_ORIGIN=https://muninn.example
docker compose -f deploy/compose.server.yml up --build -d
```

Compose publishes only `127.0.0.1:47872`. Put your HTTPS reverse proxy on that host
in front of it. The named volume is initialized for UID 10001 and survives restarts.
Use a secured secret provider instead of a file for a production deployment.
The container installs Debian's FFmpeg/Python packages, with their distribution
licenses under `/usr/share/doc`; this is separate from the macOS LGPL packaging
pipeline. This PR does not publish a container or modify desktop releases.

## Access boundary

The browser-facing adapter exposes only library/feed reads, cache requests,
retry/cancel/keep, completed file reads, and thumbnails. It dispatches these into
the existing localhost router **in-process after authentication**, without opening
an internal TCP server or capture proxy. The core's original Host/token checks
are left intact. The gateway deliberately does not expose local-path imports,
settings mutation, tool installation, native open hooks, captured browser cookies,
proxy/CA state, or stream/HLS endpoints.

Login attempts and session counts are bounded. Exact Host and Origin checks plus
`X-Muninn-Client: pwa` protect mutations, including login/logout. No wildcard CORS.
Production cookies use `__Host-`, `Secure`, `HttpOnly` and `SameSite=Strict`.
Restarting the server invalidates sessions; replacing the access-token file and
restarting rotates credentials. Sign out clears both the current server session
and this browser's device copies. Cached shell assets contain no user data; API,
media, thumbnails and session responses are never put in the shell cache.

Offline device copies intentionally remain accessible on the device even when a
server session expires. Secure the device itself. Sign out while online to revoke
the session and erase local copies. This initial single-owner adapter has no
accounts, roles, sharing permissions, per-device quotas or multi-user isolation.

## Tests

```sh
cargo test --locked -p muninn-server --bin muninn-web
node --test ui/test/pwa.test.js
```

The existing Dagger checks automatically run both (server package tests include
its binaries; JavaScript tests include `ui/test/*.test.js`). Security tests cover
Host/Origin/CSRF, public shell vs private API, cookie revocation, login throttling,
URL-token rejection, route isolation and incomplete-media refusal. Client tests
cover safe URLs, manifest/scope, bounded complete copies and service-worker cache
exclusions. Native iPhone installation and Safari codec compatibility still need
hardware acceptance before treating this as a production release.

### Deferred

Remote YouTube browser/interception, subscription/login capture on the server,
full desktop UI/graph parity, automatic desktop-server synchronization, resumable
chunk transfers, device-independent playback history and notification/background
workers. The current native app continues to provide its existing behavior.
