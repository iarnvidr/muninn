#!/usr/bin/env bash
# Publish only a signed, fully verified draft built from an accepted main tree.
# No public release is exposed before all three required assets match SHA-256.
set -euo pipefail

: "${GH_TOKEN:?the publisher requires a contents-write GitHub token}"
: "${GH_REPO:?the publisher requires a repository}"
: "${VERSION:?the publisher requires the approved version}"
: "${TAG:?the publisher requires the approved tag}"
: "${SOURCE_SHA:?the publisher requires the accepted merge SHA}"
: "${MAPPING_RUN_ID:?the publisher requires the acceptance mapping run}"
: "${RUNNER_TEMP:?the publisher requires runner scratch space}"

test "$GH_REPO" = iarnvidr/muninn
test "$TAG" = "v$VERSION"
python3 build/auto_preview_release.py verify-assets \
  --directory release-assets --version "$VERSION" --tag "$TAG" \
  --sha "$SOURCE_SHA" --mapping-run-id "$MAPPING_RUN_ID"

state="$RUNNER_TEMP/muninn-preview-release"
mkdir -p "$state"

# A slow publish must never publish a stale branch tip. No release source is
# compiled here: the only native binary is the previously accepted PR artifact.
main_sha="$(gh api "repos/$GH_REPO/git/ref/heads/main" --jq .object.sha)"
if [[ "$main_sha" != "$SOURCE_SHA" ]]; then
  if [[ "${RECOVER_EXISTING_DRAFT:-false}" != true ]] ||
     ! git merge-base --is-ancestor "$SOURCE_SHA" HEAD; then
    echo 'Refusing to publish a merge which is no longer main HEAD' >&2
    exit 1
  fi
  # Recovery may finish a previously accepted draft, never start a new
  # release for a superseded commit. The existing tag and marker are checked
  # below; both must still identify that accepted source.
  current_version="$(python3 -c 'import json; print(json.load(open("src-tauri/tauri.conf.json"))["version"])')"
  test "$current_version" = "$VERSION"
fi

# Distinguish a missing tag/release (404) from an authorization/API error.
fetch_or_missing() {
  local path="$1" output="$2" error="$3"
  if gh api "$path" > "$output" 2> "$error"; then
    return 0
  fi
  if grep -q 'HTTP 404' "$error"; then
    return 1
  fi
  sed -n '1,12p' "$error" >&2
  exit 2
}

if fetch_or_missing "repos/$GH_REPO/git/ref/tags/$TAG" "$state/tag.json" "$state/tag.err"; then
  tag_target="$(jq -r '.object.sha' "$state/tag.json")"
  tag_type="$(jq -r '.object.type' "$state/tag.json")"
  if [[ "$tag_type" != commit || "$tag_target" != "$SOURCE_SHA" ]]; then
    echo 'Release tag already identifies a different or non-commit object' >&2
    exit 1
  fi
else
  if [[ "$main_sha" != "$SOURCE_SHA" ]]; then
    echo 'Historic draft recovery requires the pre-existing immutable release tag' >&2
    exit 1
  fi
  gh api --method POST "repos/$GH_REPO/git/refs" \
    -f "ref=refs/tags/$TAG" -f "sha=$SOURCE_SHA" >/dev/null
fi

marker="<!-- muninn-auto-preview source=$SOURCE_SHA mapping=$MAPPING_RUN_ID -->"
# GitHub's by-tag endpoint returns HTTP 404 for *draft* releases. Enumerate
# releases with an authenticated token so interrupted drafts are discoverable.
gh api "repos/$GH_REPO/releases?per_page=100" > "$state/releases.json"
matching="$(jq --arg tag "$TAG" '[.[] | select(.tag_name == $tag)] | length' "$state/releases.json")"
if [[ "$matching" -gt 1 ]]; then
  echo 'More than one release identifies this version' >&2
  exit 1
fi
if [[ "$matching" -eq 1 ]]; then
  jq --arg tag "$TAG" '.[] | select(.tag_name == $tag)' "$state/releases.json" > "$state/release.json"
else
  if [[ "$main_sha" != "$SOURCE_SHA" ]]; then
    echo 'Historic draft recovery cannot create a missing release' >&2
    exit 1
  fi
  notes="Automated macOS preview from an accepted PR merge to main.

Source commit: $SOURCE_SHA
Accepted native mapping: https://github.com/$GH_REPO/actions/runs/$MAPPING_RUN_ID

This preview uses ad-hoc Apple signing and is not notarized. Its Tauri updater
payload is separately signed and verified. Select Stable and previews in Muninn
to receive this version; stable-only installations are unaffected.

$marker"
  # Show authored user-facing notes before the immutable source/CI receipt.
  changelog_file="build/release-notes/$VERSION.md"
  if [[ -f "$changelog_file" ]]; then
    notes="$(cat "$changelog_file")"$'\n\n'"$notes"
  fi
  gh api --method POST "repos/$GH_REPO/releases" \
    -f "tag_name=$TAG" -f "target_commitish=$SOURCE_SHA" \
    -f "name=Muninn $VERSION" -f "body=$notes" \
    -F draft=true -F prerelease=true > "$state/release.json"
fi
release_id="$(jq -r '.id' "$state/release.json")"
[[ "$release_id" =~ ^[1-9][0-9]*$ ]] || { echo 'Invalid GitHub release ID' >&2; exit 1; }

if ! jq -e --arg marker "$marker" \
     '.prerelease == true and ((.body | type) == "string") and (.body | contains($marker))' \
     "$state/release.json" >/dev/null; then
  echo 'Existing release has unexpected provenance or is not a prerelease' >&2
  exit 1
fi

assets=("Muninn_${VERSION}_macos-arm64.dmg" latest.json Muninn.app.tar.gz)

verify_remote_assets() {
  local name count remote expected
  # Do not allow unrelated release assets into the updater discovery contract.
  while IFS= read -r name; do
    case "$name" in
      "${assets[0]}"|latest.json|Muninn.app.tar.gz) ;;
      *) echo "Unexpected release asset: $name" >&2; return 1 ;;
    esac
  done < <(jq -r '.assets[].name' "$state/release.json")
  for name in "${assets[@]}"; do
    count="$(jq --arg name "$name" '[.assets[] | select(.name == $name)] | length' "$state/release.json")"
    if [[ "$count" -gt 1 ]]; then
      echo "Duplicate release asset: $name" >&2; return 1
    fi
    if [[ "$count" -eq 1 ]]; then
      remote="$(jq -r --arg name "$name" '.assets[] | select(.name == $name) | .digest' "$state/release.json")"
      # shasum is available on both macOS test runners and Ubuntu publishers.
      expected="sha256:$(shasum -a 256 "release-assets/$name" | cut -d ' ' -f 1)"
      if [[ "$remote" != "$expected" ]]; then
        echo "Published asset digest mismatch: $name" >&2; return 1
      fi
    elif [[ "$(jq -r '.draft' "$state/release.json")" == false ]]; then
      echo "Published release is missing required asset: $name" >&2; return 1
    fi
  done
}

verify_remote_assets
if [[ "$(jq -r '.draft' "$state/release.json")" == true ]]; then
  for name in "${assets[@]}"; do
    count="$(jq --arg name "$name" '[.assets[] | select(.name == $name)] | length' "$state/release.json")"
    if [[ "$count" == 0 ]]; then
      # Draft releases are not available through the public by-tag lookup, so
      # upload directly to the authenticated numeric release ID.
      curl --fail-with-body --silent --show-error --retry 3 --retry-delay 2 --retry-all-errors \
        -H "Authorization: Bearer $GH_TOKEN" \
        -H 'Accept: application/vnd.github+json' \
        -H 'Content-Type: application/octet-stream' \
        --data-binary "@release-assets/$name" \
        "https://uploads.github.com/repos/$GH_REPO/releases/$release_id/assets?name=$name" \
        > "$state/uploaded-asset.json"
      # GitHub can expose a newly uploaded asset before its digest is filled in.
      # Wait briefly for the server-side hash, then fail closed on a mismatch.
      for attempt in {1..12}; do
        gh api "repos/$GH_REPO/releases/$release_id" > "$state/release.json"
        remote_digest="$(jq -r --arg name "$name" '.assets[] | select(.name == $name) | .digest // empty' "$state/release.json")"
        [[ -n "$remote_digest" ]] && break
        sleep 1
      done
      verify_remote_assets
    fi
  done
  test "$(jq '.assets | length' "$state/release.json")" -eq 3
  gh api --method PATCH "repos/$GH_REPO/releases/$release_id" \
    -F draft=false -F prerelease=true > "$state/release.json"
fi

verify_remote_assets
if ! jq -e '.draft == false and .prerelease == true and (.assets | length == 3)' \
     "$state/release.json" >/dev/null; then
  echo 'Final prerelease is missing verified updater assets' >&2
  exit 1
fi
echo "Published verified preview: https://github.com/$GH_REPO/releases/tag/$TAG"
