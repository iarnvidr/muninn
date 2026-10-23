# Public release review — 2026-10-22

Recommendation: keep all four feature PRs. Owner Mac verification is still a
binary-release blocker (it does not gate source publication); Linux tests do not
establish native playback correctness. At review time no PR had been merged and
Actions was disabled; the reviewed stack became the initial public commit on
2026-10-23.

## Review and merge order

| Order | PR | Recommendation | Review result |
| --- | --- | --- | --- |
| 1 | #14 | Merge after owner verification | Apache-2.0 and public identity are consistent. Official FFmpeg/dav1d source downloads match their pinned hashes; dav1d build options exist in the pinned release. CI has read-only permissions, pinned actions and hosted runners. Added publication reference checks, made missing FFmpeg capabilities fatal, and corrected the declared Rust minimum to 1.90 to match locked Tauri dependencies. |
| 2 | #8 | Keep / merge | Rebased onto #14. Target parsing rejects unrelated origins, credentials, ambiguous app actions and invalid short links. Cold/warm delivery retains pending requests. Resolved packaging conflicts using shared bundle metadata; replaced the stale URL registration identity in both plist and regression test. Native Launch Services behavior still needs the Mac test. |
| 3 | #9 | Keep / merge | Rebased onto #8. Browse groups copies without changing Manage entries; search matches all query words and runs before the display limit. Representative-copy selection and search have regression coverage. Committed screenshots use synthetic library content. |
| 4 | #10 | Keep / merge | Rebased onto #9. The marker uses backend completion policy and the playback reporter's rounding; Keep promotes the playing entry. Protected copies and keep rules suppress the reminder. Errors leave retry available. |
| 5 | #11 | Keep / merge | Rebased onto #10. Offline policy gates fetches, workers and proxy access while preserving local playback; cancelled jobs are requeued only after exit. Cached channel refreshes retain stale data and bound concurrent work. Account actions require current account-bound subscription evidence and positive acknowledgement. Recorded-start, paused-stall recovery and cancelled-source fixes have regression coverage. Native account switching and playback remain Mac checks. |

#8 targets `public-prep`; #9–#11 retain their existing stacked targets. Resolve
the stack in the order above using merge commits so descendant ancestry remains
intact. If the owner chooses squash merges, rebase each remaining descendant and
retarget its base before proceeding. The overseer will replace this history with
one fresh commit at publication.

All rewritten feature commits use the owner's GitHub noreply identity. Every
final branch tree must pass the publication boundary check. Dependabot PRs are
left open and are not part of this merge order; reassess updates after publication.

## Validation and remaining blockers

The following suites pass on each rebased tree (Rust 1.99.1):

| PR | Core Rust tests | JavaScript tests | Packaging tests |
| --- | ---: | ---: | ---: |
| #14 | 225 | 36 | 26 |
| #8 | 225 | 36 | 27 |
| #9 | 225 | 38 | 27 |
| #10 | 226 | 40 | 27 |
| #11 | 238 | 48 | 27 |

- Publication boundary checks and `git diff --check` pass on every tree.
- Gitleaks 8.30.1 (checksum verified): source tree and Git history report no leaks.
- Official source archives: both SHA-256 values match `tools.lock.json` and the
  dav1d recipe. No Mac build is claimed from this download verification.
- Full workspace compilation with pinned Rust 1.99.1 reaches native dependency
  discovery but this Linux host lacks pkg-config/GTK development packages.
- Owner must run [RELEASE-CHECKLIST.md](RELEASE-CHECKLIST.md) on the final #11
  tree: native workspace tests/build, LGPL helper build, VideoToolbox/HTTPS,
  compatibility HLS playback, URL handoff and feature smoke tests.
- Before a binary release, finish the source-archive and dependency-notice gates
  in [PUBLICATION.md](PUBLICATION.md). Source publication and binary release have
  separate gates; this review does not mark the existing unchecked items done.

The repository was confirmed private with Actions disabled before pushing the
rebased stack. No workflow was enabled or dispatched, no self-hosted runner was
introduced, and no release or repository recreation was performed.

Runner-label reference: GitHub currently lists `macos-15` as a standard arm64
hosted runner in its [runner reference](https://docs.github.com/en/actions/reference/runners/github-hosted-runners).
