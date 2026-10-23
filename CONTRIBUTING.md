# Contributing

Muninn is an experimental macOS app. Read [the architecture](README.md#architecture),
[development setup](docs/DEVELOPMENT.md) and [known issues](docs/KNOWN_ISSUES.md).

Keep changes focused and describe the user-visible result. Include relevant
verification and the platform used. A Linux WebKitGTK result does not establish
that WKWebView or native HLS works on macOS. Do not claim a fix without evidence.

Run the Rust, JavaScript and packaging checks in DEVELOPMENT.md before proposing
a change. Media lifecycle changes should also exercise the real production player.
Use synthetic media and isolated application data for regression fixtures.

Do not commit diagnostics, databases, browser sessions, credentials, downloaded
videos or personal screenshots. Review logs before attaching them to an issue.
Report vulnerabilities privately as described in SECURITY.md.

Muninn's own code is licensed under the Apache License, Version 2.0; contributions are accepted under the same terms.
Third-party assets keep their own terms; see THIRD_PARTY_NOTICES.md. The license
does not grant trademark rights or permission to imply endorsement with the logo.
