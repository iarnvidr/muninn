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

Only the current preview version receives fixes. There is no response-time guarantee.
Build jobs use GitHub-hosted runners with read-only repository permissions; they
do not run public PR code on the maintainer's infrastructure.
