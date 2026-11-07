#!/usr/bin/env bash
# Read-only GHCR fallback for a fresh GitHub-hosted Dagger engine.
# GHCR contains compilations, NEVER a substitute for a passing check receipt.
set -euo pipefail

seed="${GITHUB_WORKSPACE:-$PWD}/ci/cache/linux-cargo"
mkdir -p "$seed"

# The Actions cache may have returned a restore-prefix hit. Keep its source
# mirror and target together; never overlay two different Cargo snapshots.
if [[ -e "$seed/worktree" || -e "$seed/target" || -e "$seed/cargo" ]]; then
  echo 'Cargo snapshot already restored from actions/cache; keeping it intact.'
  exit 0
fi

if [[ -n "${GHCR_READ_TOKEN:-}" ]]; then
  printf '%s' "$GHCR_READ_TOKEN" | docker login ghcr.io --username "${GITHUB_ACTOR:-github-actions}" --password-stdin >/dev/null
  trap 'docker logout ghcr.io >/dev/null 2>&1 || true' EXIT
fi

images=()
if [[ "${MUNINN_PR_NUMBER:-}" =~ ^[0-9]+$ ]]; then
  images+=("ghcr.io/iarnvidr/muninn-ci-cache:pr-${MUNINN_PR_NUMBER}")
fi
images+=("ghcr.io/iarnvidr/muninn-ci-cache:main")

for image in "${images[@]}"; do
  if ! docker pull --quiet "$image"; then
    echo "No readable package at $image; trying next source."
    continue
  fi
  source_label="$(docker image inspect --format '{{ index .Config.Labels "org.opencontainers.image.source" }}' "$image")"
  kind_label="$(docker image inspect --format '{{ index .Config.Labels "io.muninn.cache.kind" }}' "$image")"
  if [[ "$source_label" != https://github.com/iarnvidr/muninn || "$kind_label" != cargo-cache-v1 ]]; then
    echo "Refusing unexpected package format: $image" >&2
    exit 1
  fi
  cid="$(docker create --entrypoint /bin/true "$image")"
  if ! docker cp "$cid:/cache/." "$seed/"; then
    docker rm "$cid" >/dev/null
    echo "Cannot restore package $image" >&2
    exit 1
  fi
  docker rm "$cid" >/dev/null
  echo "Restored Cargo source mirror + compiled artifacts from $(docker image inspect --format '{{index .RepoDigests 0}}' "$image")"
  exit 0
done

echo 'No previous GHCR compilation package available; Cargo will build cold.'
