# Signed-in downloads

From 0.2.15, the desktop app passes the YouTube session observed in Browse to
yt-dlp for downloads, playback resolution, playlists and channel lookups. Earlier
versions could show Browse as signed in while downloading anonymously.

Open YouTube in Browse after launching Muninn so the current account is observed.
After updating, retry downloads that previously failed with a sign-in message.
The account still needs permission to watch the video, including any age
verification required by YouTube.

The app reads secure and HttpOnly cookies natively from its existing browser
store. Only YouTube domains are accepted. Cookie values never pass through page
IPC or enter app settings, channel discovery, diagnostics or command arguments.
Each helper receives a separate private temporary Netscape cookie file, removed
when the helper finishes. A process crash can leave a private temporary file;
the app does not claim crash-proof secret erasure. Signing out, changing accounts
or invalidating the account clears the in-memory session for future operations.
Already running helpers finish with their starting session. The standalone
headless service has no browser session and continues to use anonymous requests.

Hosted checks use synthetic cookies only. The macOS smoke reads a synthetic
secure/HttpOnly cookie through the persistent YouTube WKWebView store; core tests
check domain filtering, private files, cleanup and download/playback handoff.
They do not assert access to a real user's restricted videos.
