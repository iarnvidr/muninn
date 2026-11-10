# Security reports

Muninn is experimental and has not undergone an independent security audit.
Report vulnerabilities using [GitHub's private vulnerability reporting](https://github.com/iarnvidr/muninn/security/advisories/new)
(**Security → Report a vulnerability**). If that option is unavailable, contact
the maintainer through their GitHub profile to arrange a private channel. Do not
disclose exploit details in a public issue.

Include the affected version, platform, reproduction steps and impact. Do not send
account cookies, authorization tokens, CA private keys, browser profiles or full
databases. Sanitized diagnostics may still reveal video IDs, channel names and
error details: review them first.

## Local security model

The app uses an authenticated localhost service and a per-install capture proxy.
The proxy decrypts supported YouTube browsing traffic to cache content. Google
account authentication traffic is excluded. Certificates and browser login data
remain local. The CA trust handling, native webview isolation and localhost API
are security-sensitive components; changes need focused review and regression checks.

## Optional standalone server

`muninn-service --server` enables a separate browser-facing API and PWA. It is
single-user: every paired browser can access the same server library, queue
downloads and change shared watch progress. Pair only devices trusted with that
library. The server does not provide user accounts or per-device permissions.

The default listener is loopback and the default data directory is separate from
the native app. Remote access requires an explicit HTTPS public origin and a
reverse proxy that preserves Host and Origin. The application validates that
origin directly, without trusting forwarded-host headers. Keep the internal
localhost API private; only the restricted browser router belongs behind the
HTTPS proxy. Server mode does not start the native capture proxy or request CA
trust.

A locally readable token file pairs a browser through an authorization header.
Browser sessions use opaque HttpOnly, SameSite=Strict cookies, with Secure set for
HTTPS. Tokens are not placed in URLs or browser local storage. Sessions expire
after seven days or a server restart; signing out revokes the current session.
Restarting rotates the token and revokes all sessions. Treat the token file,
session cookies and the server data directory as credentials/private data. Do
not log authorization or cookie headers at a reverse proxy.

The PWA service worker caches static app assets only, not authentication, library
responses or media. Do not add a shared proxy cache for authenticated responses.
The server's existing helper bootstrap may download and update its own yt-dlp;
FFmpeg and the JavaScript runtime must be supplied by the operator. See
[server setup](docs/PWA_SERVER.md) for deployment, persistence and initial limits.

## Maintenance

Only the current preview version receives fixes. There is no response-time guarantee.
Build jobs use GitHub-hosted runners with read-only repository permissions; they
do not run public PR code on the maintainer's infrastructure.
