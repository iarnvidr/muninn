#!/usr/bin/env bash
# Cross-compiles the macOS app on Linux, assembles "Muninn.app" by hand (Tauri's
# own bundler only produces .app/.dmg on macOS), bundles yt-dlp + ffmpeg + deno,
# ad-hoc signs the bundle with rcodesign and zips it.
#
#   build/build-macos.sh [OUT [VARIANTS]]
#
# VARIANTS defaults to "arm64": one zip each in OUT, named
# Muninn_<version>_macos-<variant>.zip, with .sha256 files and the integrity report
# verify-macos.txt. CI uses this path on the self-hosted Linux runners.
# It cannot run the app: it tests, compiles, signs, packages and verifies the
# package structure and signature hashes.
set -euo pipefail

app_dir="$(cd "$(dirname "$0")/.." && pwd)"
out="${1:-$app_dir/dist}"
variants="${2:-arm64}"
mkdir -p "$out"

# A caller can reuse an explicitly selected local compiler image (prefer its
# immutable sha256 ID). CI builds the checked-in Dockerfiles by default.
image="${MUNINN_MACOS_BUILDER_IMAGE:-muninn-builder-macos}"
if [[ -n "${MUNINN_MACOS_BUILDER_IMAGE:-}" ]]; then
  docker image inspect "$image" >/dev/null
else
  docker build -t muninn-builder "$app_dir/build"
  docker build -f "$app_dir/build/Dockerfile.macos" -t "$image" "$app_dir/build"
fi

# The Docker daemon may be remote (bind mounts of this checkout would not exist
# there), so source and results travel with docker cp instead of -v.
cid="$(docker create \
  --memory 6g --memory-swap 8g --cpus 2 \
  --user "$(id -u):$(id -g)" \
  -e HOME=/tmp/home \
  -e CARGO_TARGET_DIR=/cache/target-macos \
  -e CARGO_BUILD_JOBS=2 \
  -e VARIANTS="$variants" \
  ${FFMPEG_LGPL_BINARY:+-e FFMPEG_LGPL_BINARY=/cache/ffmpeg-lgpl} \
  -v muninn-cargo-registry:/opt/cargo/registry \
  -v "muninn-unit-tests-$(id -u)":/cache/target \
  -v muninn-target-macos:/cache/target-macos \
  -v muninn-tools-cache:/cache/tools \
  "$image" bash -euxo pipefail -c '
    mkdir -p "$HOME" /cache/target-macos /cache/out
    tar -C /cache/source --exclude=node_modules --exclude=target --exclude=dist -cf - . | tar -C /build --no-overwrite-dir -xf -
    cd /build

    # --- tests: Rust core + shell, page script / UI logic, build scripts -----------
    CARGO_TARGET_DIR=/cache/target cargo test --locked --workspace
    node --test src-tauri/test/*.test.js ui/test/*.test.js
    python3 -m unittest discover -s build -p "test_*.py"

    # --- helper binaries: pinned, checksum-verified -----------------------------------
    tool_args=()
    case "$VARIANTS" in
      arm64|x86_64) tool_args=(--arch "$VARIANTS") ;;
    esac
    python3 build/fetch_tools.py /tmp/tools --cache /cache/tools "${tool_args[@]}"

    # --- the app, once per CPU architecture --------------------------------------------
    version="$(python3 -c "import json; print(json.load(open(\"src-tauri/tauri.conf.json\"))[\"version\"])")"
    for arch in aarch64 x86_64; do
      case " $VARIANTS " in
        *" universal "*) need=1 ;;
        *" ${arch/aarch64/arm64} "*) need=1 ;;
        *) need=0 ;;
      esac
      [ "$need" = 1 ] || continue
      # rust-toolchain.toml can select a version different from a reused image.
      rustup target add "$arch-apple-darwin"
      cargo zigbuild --release --locked -p muninn --features custom-protocol --target "$arch-apple-darwin"
    done
    bin_of() { echo "$CARGO_TARGET_DIR/$1-apple-darwin/release/muninn"; }

    assemble() {  # variant, archs...
      local variant="$1"; shift
      local archs=("$@")
      local app="/tmp/stage-$variant/Muninn.app"
      rm -rf "/tmp/stage-$variant"; mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources/bin"
      if [ "${#archs[@]}" -gt 1 ]; then
        llvm-lipo-14 -create "$(bin_of aarch64)" "$(bin_of x86_64)" -output "$app/Contents/MacOS/muninn"
      else
        cp "$(bin_of "${archs[0]}")" "$app/Contents/MacOS/muninn"
      fi
      chmod 755 "$app/Contents/MacOS/muninn"
      cp src-tauri/icons/icon.icns "$app/Contents/Resources/icon.icns"
      cp -p /tmp/tools/"$variant"/* "$app/Contents/Resources/bin/"
      cp /tmp/tools/NOTICE.txt "$app/Contents/Resources/bin/NOTICE.txt"
      cp -R licenses "$app/Contents/Resources/licenses"
      cp LICENSE "$app/Contents/Resources/licenses/Muninn-Apache-2.0.txt"
      cp ui/fonts/*-OFL.txt "$app/Contents/Resources/licenses/"
      python3 build/license_inventory.py --collect "$app/Contents/Resources/licenses/rust" > "$app/Contents/Resources/licenses/rust-packages.json"
      cp THIRD_PARTY_NOTICES.md "$app/Contents/Resources/THIRD_PARTY_NOTICES.md"
      python3 build/native_info.py "$app"
      rcodesign sign "$app"
      python3 build/verify_macos.py "$app" "${archs[@]}" | tee -a /cache/out/verify-macos.txt
      local zip_name="Muninn_${version}_macos-${variant}.zip"
      (cd "/tmp/stage-$variant" && zip -qry "/cache/out/$zip_name" "Muninn.app")
      zip -T "/cache/out/$zip_name"
      (cd /cache/out && sha256sum "$zip_name" > "$zip_name.sha256")
      file "$app/Contents/MacOS/muninn" || true
    }
    : > /cache/out/verify-macos.txt
    for variant in $VARIANTS; do
      case "$variant" in
        universal) assemble universal aarch64 x86_64 ;;
        arm64) assemble arm64 aarch64 ;;
        x86_64) assemble x86_64 x86_64 ;;
        *) echo "unknown variant $variant" >&2; exit 2 ;;
      esac
    done
    ls -l /cache/out
  ')"

trap 'docker rm -f "$cid" >/dev/null 2>&1 || true' EXIT
docker cp "$app_dir/." "$cid":/cache/source
# ffmpeg is compiled on an Apple Silicon Mac (build/ffmpeg_lgpl.py), not in this container.
# Pass that binary with FFMPEG_LGPL_BINARY=/path/to/ffmpeg; it is re-verified as LGPL-only.
if [[ -n "${FFMPEG_LGPL_BINARY:-}" ]]; then
  docker cp "$FFMPEG_LGPL_BINARY" "$cid":/cache/ffmpeg-lgpl
fi
docker start -a "$cid"
docker cp "$cid":/cache/out/. "$out"
ls -l "$out"
