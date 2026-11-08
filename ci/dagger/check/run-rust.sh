#!/usr/bin/env bash
# Cargo's stable fingerprinting uses source mtimes. Preserve the mtimes of
# unchanged bytes across fresh checkouts without trusting a snapshot's mtimes.
# The caller must hold Dagger's LOCKED cache mount for this entire process.
set -euo pipefail

if (( $# < 3 )); then
  printf '%s\n' 'usage: run-rust.sh INPUT_DIR CACHE_ROOT COMMAND [ARG ...]' >&2
  exit 2
fi

input_dir="$1"
cache_root="$2"
shift 2

if [[ ! -d "$input_dir" || -L "$input_dir" ]]; then
  printf '%s\n' 'Rust source input must be a real directory, not a symlink.' >&2
  exit 2
fi
if [[ "$cache_root" != /* || "$cache_root" == / || -L "$cache_root" ]]; then
  printf '%s\n' 'Rust cache root must be an absolute, non-root, non-symlink directory.' >&2
  exit 2
fi

input_dir="$(realpath "$input_dir")"
cache_root="$(realpath -m "$cache_root")"
if [[ "$cache_root" == / || "$cache_root" == "$input_dir" || "$cache_root" == "$input_dir"/* ]]; then
  printf '%s\n' 'Rust cache must be a non-root directory outside the source input.' >&2
  exit 2
fi
worktree="$cache_root/worktree"
target="$(realpath -m "${CARGO_TARGET_DIR:-$cache_root/target/default}")"
case "$target" in
  "$cache_root"/target/*) ;;
  *) printf '%s\n' 'CARGO_TARGET_DIR must be a namespace below CACHE_ROOT/target/.' >&2; exit 2 ;;
esac
if [[ -L "$worktree" || "$input_dir" == "$worktree" || "$input_dir" == "$cache_root"/* ]]; then
  printf '%s\n' 'Rust source must be outside the cache; cached worktree must not be a symlink.' >&2
  exit 2
fi
if [[ -n "$(find "$input_dir" -type l -print -quit)" ]]; then
  printf '%s\n' 'Rust source symlinks are unsupported: provide ordinary files inside the source input.' >&2
  exit 2
fi

mkdir -p "$worktree" "$target"
anchor="$(mktemp "$cache_root/.clock.XXXXXX")"
trap 'rm -f -- "$anchor"' EXIT
if [[ -n "$(find "$target" -type f -newer "$anchor" -print -quit)" ]]; then
  printf '%s\n' 'Restored Cargo target has future timestamps (clock skew). Retry with a fresh Cargo cache; refusing potentially stale output.' >&2
  exit 2
fi

# Deliberately NO --times. --checksum retains an existing file only when its
# bytes match, whatever timestamp an archive or checkout happened to give it.
# Dagger's 13 checks mount DIFFERENT source subsets into the SAME worktree.
# A plain `--delete` on every check destroys the sources imported with other
# published crate packages; Cargo then recompiles those unchanged crates when
# a subsequent check adds their sources back. The host supplies the complete
# checked-out source path manifest to distinguish a scoped omission from a
# real deletion. In local/full-worktree builds, retain the original --delete.
source_manifest="$cache_root/current-source-files.txt"
if [[ -f "$source_manifest" ]]; then
  if ! LC_ALL=C sort --check=quiet --unique "$source_manifest"; then
    printf '%s\n' 'Complete source manifest must be sorted with no duplicates.' >&2
    exit 2
  fi
  rsync --recursive --checksum --links --perms "$input_dir/" "$worktree/"
  stale_list="$(mktemp "$cache_root/.stale-sources.XXXXXX")"
  trap 'rm -f -- "$stale_list"' EXIT
  comm -23 <(find "$worktree" -type f -printf '%P\n' | LC_ALL=C sort) "$source_manifest" > "$stale_list"
  while IFS= read -r stale; do
    case "$stale" in
      ''|/*|../*|*/../*|*/..|.)
        printf 'Unsafe source path in removal list: %s\n' "$stale" >&2
        exit 2 ;;
    esac
    rm -f -- "$worktree/$stale"
  done < "$stale_list"
  rm -f -- "$stale_list"
  trap - EXIT
else
  rsync --recursive --checksum --links --perms --delete "$input_dir/" "$worktree/"
fi
rm -f -- "$anchor"
trap - EXIT
export CARGO_TARGET_DIR="$target"
cd "$worktree"
# An individual Dagger check can request a diagnostic copy of Cargo's complete
# stdout/stderr. Dagger's hosted summary sometimes exposes only exit code 101;
# the log is exported from the same locked volume even when the check fails.
# Restrict the destination to this cache so the diagnostic cannot write to a
# source tree or an arbitrary host-mounted path.
if [[ -n "${MUNINN_CARGO_LOG:-}" ]]; then
  case "$MUNINN_CARGO_LOG" in
    "$cache_root"/check-logs/*.log) ;;
    *) printf '%s\n' 'MUNINN_CARGO_LOG must be under CACHE_ROOT/check-logs/.' >&2; exit 2 ;;
  esac
  mkdir -p "$cache_root/check-logs"
  "$@" 2>&1 | tee -- "$MUNINN_CARGO_LOG"
else
  exec "$@"
fi
