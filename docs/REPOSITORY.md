# Repository maintenance

The repository is public; development and previews use GitHub-hosted runners.
Source visibility and successful builds do not establish native playback quality.
Binary publication still follows [the publication checklist](PUBLICATION.md).

## Main branch

The active [Protect main](https://github.com/iarnvidr/muninn/rules/25743788) ruleset
applies to the default branch:

- All changes go through a pull request.
- The `test` and `package` checks must pass, sourced from GitHub Actions.
- The PR must be tested against the current base and all review threads resolved.
- Force pushes, branch deletion and merge commits are blocked.
- No actor has a ruleset bypass. The owner can change the ruleset in an emergency.
- No additional approving review is required for a solo-maintained project.
  CODEOWNERS requests the maintainer's review without blocking their own PRs.

Squash is the only enabled merge method; its commit uses the PR title and body.
Auto-merge is disabled and merged branches are deleted automatically. Agents must
still obtain the owner's authorization before merging.

If workflow job names change, update the ruleset in the same maintenance window
so that required checks do not wait for names that no longer exist. Retain
GitHub Actions as the required check source.

## Security and community

Private vulnerability reporting, dependency alerts, automatic security update PRs,
secret scanning and push protection are enabled. Weekly Dependabot configuration
groups routine Cargo patch/minor changes and GitHub Actions updates to limit noise.
Major Cargo upgrades remain separate. Review updates through the usual PR checks.

Actions use read-only default tokens, cannot approve PRs and allow only selected
GitHub-owned actions. Workflow actions are pinned to commit IDs. Runners are
GitHub-hosted. Do not add private infrastructure or credentials to public PR jobs.

Issues have bug and feature forms; Discussions is for questions. Keep the security
report link, [security policy](../SECURITY.md), [contribution guide](../CONTRIBUTING.md)
and [Code of Conduct](../CODE_OF_CONDUCT.md) current. The repository's description
and topics should describe supported functionality without promising stable playback.

## Branding

The README reuses the app's existing red raven in `ui/raven.png`. The social preview
is `docs/assets/social-preview.png`, rendered from `design/social-preview.html` at
1280 × 640. Keep it under 1 MB. The logo is not a claim of endorsement by YouTube
or Google; the repository does not change the organization's avatar.

To update GitHub's social card, open **Settings → General → Social preview → Edit →
Upload an image** and select that PNG. This setting is separate from the README
and is managed through GitHub's web interface.
