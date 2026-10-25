# Signed-in downloads

From 0.2.16, the desktop app reads its persistent native YouTube cookie store at
the start of each yt-dlp operation: downloads, playback resolution, playlists and
channel lookups. The reader is registered before restored jobs start. Version
0.2.15 attempted this through live page account observations, leaving a startup
gap before Browse reported an account ID. Earlier versions had no handoff.

The existing browser login is available before opening Browse or subscription
discovery. After updating, retry downloads that previously failed with a sign-in message.
The account still needs permission to watch the video, including any age
verification required by YouTube.

The app reads secure and HttpOnly cookies natively from its existing browser
store. Only YouTube domains are accepted. Cookie values never pass through page
IPC or enter app settings, channel discovery, diagnostics or command arguments.
Each helper receives a separate private temporary Netscape cookie file, removed
when the helper finishes. A process crash can leave a private temporary file;
the app does not claim crash-proof secret erasure. Every new helper reads the
current store again, including cookie rotations, account changes and sign-out;
it does not reuse a page snapshot. Already running helpers finish with their starting session. The standalone
headless service has no browser session and continues to use anonymous requests.

Hosted checks use synthetic cookies only. The macOS smoke reads a synthetic
secure/HttpOnly cookie through a separate blank WKWebView in the shared persistent
store before any account observation, then verifies rotation and deletion are
visible on subsequent reads. Core tests check fresh per-helper reads, domain
filtering, private files, cleanup and download/playback handoff. A failed native
read fails the operation instead of silently downloading anonymously. Safe
`youtube.session` events record the reader source and presence booleans only.
They do not assert access to a real user's restricted videos.
