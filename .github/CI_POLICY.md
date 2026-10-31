# CI action policy

The Linux check uses the official Dagger GitHub Action at the reviewed immutable revision
`dagger/dagger-for-github@496f1b3d8b0d823834c13e67cf8a8e08ca3b9602` and pins Dagger CLI
version `0.21.11` in `.github/workflows/check.yml`.

The repository Actions allow-list must include that exact action revision. If the action revision
changes, update the allow-list to the new reviewed full commit SHA before relying on the hosted
check. A denied action appears as a startup failure before any job steps run; it is not evidence
that the Linux checks passed or failed.
