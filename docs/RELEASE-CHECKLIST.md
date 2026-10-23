# Owner Mac release gate

Run on an Apple Silicon Mac, macOS 14+, from the commit a binary release is
cut from. The source repository is public and its hosted Checks and macOS
workflows run on every push. Record the commit (`git rev-parse HEAD`), macOS
version and results below before a binary is published. Any failed step blocks
that binary release; this checklist does not gate source visibility.

## Build and package

```sh
uname -sm
sw_vers -productVersion
xcode-select -p
brew install meson ninja pkgconf node python
rustup show active-toolchain
python3 build/check_publication.py
bash build/build-native-macos.sh "$PWD/dist/mac-review"
```

Expected: `Darwin arm64`, macOS 14 or newer, a valid developer directory, the
toolchain from `rust-toolchain.toml`, `Standalone boundary checks passed`, all
Rust/JavaScript/Python tests pass (one synthetic binary test is skipped on macOS),
and signing reports `valid on disk` / `satisfies its Designated Requirement`.
The last line names `Muninn_<version>_macos-arm64.zip`. This script runs
`cargo test --locked --workspace`, compiles the native shell and builds the
SHA-256-pinned FFmpeg and dav1d sources. No Actions run is needed.

```sh
cd dist/mac-review
shasum -a 256 -c Muninn_*.zip.sha256
mkdir -p unpacked
ditto -x -k Muninn_*.zip unpacked
cd ../..
app="$PWD/dist/mac-review/unpacked/Muninn.app"
ffmpeg="$app/Contents/Resources/bin/ffmpeg"
python3 build/ffmpeg_lgpl.py verify "$ffmpeg"
otool -L "$ffmpeg"
```

Expected: archive `OK`; verifier exits zero, prints configure flags including
`--disable-gpl`, `--disable-nonfree`, `--enable-libdav1d`,
`--enable-videotoolbox`, `--enable-securetransport`, with no missing capabilities.
`otool` must show only macOS system libraries/frameworks, with no Homebrew or
temporary build paths. Bundled notices and license files must be present.

## VideoToolbox and HTTPS

```sh
probe="$(mktemp -d)"
"$ffmpeg" -hide_banner -y -f lavfi -i testsrc2=size=640x360:rate=30 \
  -f lavfi -i sine=frequency=440 -t 12 -c:v h264_videotoolbox -q:v 65 \
  -profile:v high -allow_sw 1 -pix_fmt yuv420p -c:a aac "$probe/vt.mp4"
"$ffmpeg" -v error -i "$probe/vt.mp4" -f null -
open "$probe/vt.mp4"
"$ffmpeg" -hide_banner -v error \
  -i https://storage.googleapis.com/gtv-videos-bucket/sample/BigBuckBunny.mp4 \
  -t 5 -f null -
```

Expected: every command exits zero; the test clip plays moving video with a tone
in QuickTime. HTTPS reads and decodes five seconds with no TLS/protocol errors.
If the public sample becomes unavailable, record a replacement HTTPS media URL
and rerun; a listed `https` protocol alone is not a playback test.

## App smoke test

```sh
open "$app"
open -a "$app" 'https://www.youtube.com/watch?v=dQw4w9WgXcQ'
open 'muninn://open?url=https%3A%2F%2Fwww.youtube.com%2Fresults%3Fsearch_query%3Dmuninn'
```

Expected: video opens in Muninn's player; the app link opens a search tab.
Repeat both after quitting and after hiding the window: one app instance opens
and receives each request. Use a permitted test video if the example is unavailable.

- Download a video requiring compatibility conversion (AV1/WebM is suitable).
  Play the local copy, seek, and resume: moving video and audio, no VideoToolbox
  error. This exercises Muninn's actual fragmented-MP4 HLS path.
- Library: Browse/Manage switch persists; search finds a title beyond the first
  displayed page; filters and Keep still work.
- Player: reach the watched threshold; Keep protects the playing copy; dismissal
  leaves the cleanup policy intact. A protected copy has no reminder.
- Channels: discover and favourite a channel, refresh its uploads, switch offline,
  restart, browse cached uploads and play a downloaded copy. Offline must not
  start network downloads. Reconnect and verify queued work resumes.
- Recorded/live playback: start, seek within the recording, stop and replay the
  saved result. Verify sign-out/account switching disables account actions.

## Owner sign-off

- [ ] Commit / macOS / hardware recorded; build and helper checks passed.
- [ ] QuickTime, HTTPS and in-app compatibility playback passed.
- [ ] Feature smoke tests passed, including cold/warm URL delivery.
- [x] Owner approved the reviewed stack (#14 → #8 → #9 → #10 → #11); it was
      published as the repository's initial public commit on 2026-10-23.

GitHub-hosted runners only; no self-hosted runners. Do not publish a
binary until the source, helper build recipe and bundled notices are available
as described in [PUBLICATION.md](PUBLICATION.md).
