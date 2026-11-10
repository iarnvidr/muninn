# Server-browser preview (additive)

The macOS app and its release channel are untouched. The container adds a private
Patchright Chromium profile, rendered browsing canvas and a same-origin gateway
in front of `muninn-web`. The PWA's existing device Blob player remains unchanged.
The server browser blocks upstream audio/video delivery; it is not another player.

`/browser/state`, `/browser/action` and `/browser/frame` require both the edge's
Authelia login and the existing Muninn pairing cookie. Mutations require an exact
Origin plus `X-Muninn-Client: pwa`. No remote CDP, arbitrary JavaScript evaluation,
file upload, external HTTP proxy or public capture endpoint is provided.

Home/search/channel/subscription pages, watch metadata and YouTube API responses
flow through the existing `Core::capture` parser. Open watch tabs use the existing
OpenTab encounter and download rules. Four browser tabs and sixteen queued capture
responses are allowed. Capture responses are bounded to 4 MiB; oversized/dropped
captures are reported rather than silently treated as a complete feed. Response
body reads have a timeout; provider response bodies are never logged.

Google sign-in is performed interactively inside this dedicated server browser.
Its profile persists in the server volume. Only a private, atomic YouTube cookie
snapshot is supplied to Core's per-job downloader session mechanism. Mac cookies
are never imported. Provider login acceptance and live YouTube acquisition still
require real acceptance testing; automated CI uses synthetic provider pages.
The initial browser is **single owner**, not isolated per Authelia identity. The
infrastructure policy restricts this preview to `user:esben`; do not grant others
access to a logged-in browser. Reset removes this server profile and its cookies,
not downloaded videos. Signing out of the PWA revokes the PWA session; it does not
silently erase the owner's server Google session.

## Image and deploy

`ci/dagger/server-preview` builds the image using the existing Linux Cargo seed,
ABI and retained per-crate packages. It deliberately uses the debug-profile server
binary to reuse checked Linux compilation; this is a preview, not the native
release artifact. The Python/browser layer rebuilds independently of Rust.

`Server preview image` tests Rust's new capture adapter and the Python policy,
then runs the exact image with non-root sandboxed Chromium against synthetic
YouTube pages and the real Core. Only a successful trusted push publishes
`ghcr.io/iarnvidr/muninn-server:preview-<40-character-source-sha>` and an immutable
digest receipt. Pull-request/fork runs never publish. The deployment repo owns
selection, source-check verification, OptiPlex configuration, Authelia, DNS,
health and rollback; publishing an image alone is not a deployment.

The image requires `MUNINN_WEB_ORIGIN`, `MUNINN_WEB_TOKEN_FILE` and a dedicated
writable `/var/lib/muninn` volume owned by UID 10001. Only port 8080 is public to
the private Docker network. Rust's ports 47872/47873 are loopback-only inside the
container. Run non-root with init, no-new-privileges, all capabilities dropped,
read-only root and writable /tmp plus the data volume. Use the supplied amd64
seccomp profile: it permits Chromium's user-namespace sandbox, not privileged
containers or SYS_ADMIN. Failure to launch that sandbox is an acceptance failure,
not a reason to add `--no-sandbox`.

Local offline tests: `cd server-preview && python3 -m unittest test_preview -v`.
The image smoke is `/opt/muninn-preview/smoke.py`; it uses no personal credentials
and must run inside the started image. iPhone install/touch input, live Google
login and live feed/download behavior require further device/provider acceptance.
