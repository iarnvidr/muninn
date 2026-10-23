# Contributing

Muninn is an experimental macOS app. Read [the architecture](README.md#architecture),
[development setup](docs/DEVELOPMENT.md) and [known issues](docs/KNOWN_ISSUES.md).

Use the [issue forms](https://github.com/iarnvidr/muninn/issues/new/choose) for bugs
and feature requests, and [Discussions](https://github.com/iarnvidr/muninn/discussions)
for usage questions. Check existing reports before opening another one. Include
the app version and build run or commit: different preview branches contain
different work. Follow the [Code of Conduct](CODE_OF_CONDUCT.md).

Keep changes focused and describe the user-visible result. Include relevant
verification and the platform used. A Linux WebKitGTK result does not establish
that WKWebView or native HLS works on macOS. Do not claim a fix without evidence.

Run the Rust, JavaScript and packaging checks in DEVELOPMENT.md before proposing
a change. Media lifecycle changes should also exercise the real production player.
Use synthetic media and isolated application data for regression fixtures.

## Pull requests and merges

Start a focused branch from `main` and open a pull request. Describe the problem,
the resulting behavior and the checks you ran. Draft PRs are welcome for work in
progress. The maintainer decides whether and when a change is merged.

`main` requires a PR, resolved review conversations, and passing **Checks / test**
and **Apple Silicon build / package** jobs from GitHub Actions on the current base.
Direct pushes, force pushes and branch deletion are blocked. Merges use squash
commits, and merged branches are deleted automatically. No additional approving
review is required while the project has a single maintainer; the maintainer can
merge their own PR after the required checks pass. Auto-merge is disabled.

Repository settings are documented in [repository maintenance](docs/REPOSITORY.md).

Do not commit diagnostics, databases, browser sessions, credentials, downloaded
videos or personal screenshots. Review logs before attaching them to an issue.
Report vulnerabilities privately as described in SECURITY.md.

Muninn's own code is licensed under the Apache License, Version 2.0; contributions are accepted under the same terms.
Third-party assets keep their own terms; see THIRD_PARTY_NOTICES.md. The license
does not grant trademark rights or permission to imply endorsement with the logo.
