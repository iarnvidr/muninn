# Development

Install Git, Python 3, Node 22 and rustup. `rust-toolchain.toml` selects the tested
Rust version automatically. Cargo.lock pins Rust dependencies. No frontend npm
installation is needed: the app embeds plain HTML, CSS and JavaScript.

## macOS (Apple Silicon)

Install Xcode Command Line Tools (`xcode-select --install`) and the helper build tools (`brew install meson ninja pkgconf`; they compile the bundled LGPL-only FFmpeg and dav1d). From the repo root:

```sh
cargo test --locked --workspace
node --test src-tauri/test/*.test.js ui/test/*.test.js
python3 -m unittest discover -s build -p 'test_*.py' -v
bash build/build-native-macos.sh "$PWD/dist"
```

The packaging script builds arm64 only, fetches checksum-pinned helpers, assembles
the existing application identity and ad-hoc signs/verifies the bundle. It does not
notarize, upload a release, or change repository visibility. CI uses this same path
on a GitHub-hosted Apple Silicon runner. A successful build is not a playback test.

## Linux

On Ubuntu 24.04 install build-essential, pkg-config, libssl-dev,
libwebkit2gtk-4.1-dev, libayatana-appindicator3-dev, librsvg2-dev and patchelf.
Install ffmpeg for media integration tests. Run the same three check commands above.
`cargo build --locked -p muninn` builds the development shell.

The existing Linux-to-Mac cross-build remains available with Docker:

```sh
bash build/build-macos.sh "$PWD/dist" arm64
```

Native macOS CI is preferred because it does not need a separately downloaded SDK.

Maintainer release orchestration can also live in a separate private infrastructure
repository. That pipeline should fetch explicitly approved source commits, keep
runner configuration and signing/publishing credentials private, and publish only
the intended artifacts. Public pull-request code must stay on disposable hosted
runners; hiding an orchestration file does not make untrusted code safe to run on
private infrastructure. No private runner configuration belongs in this repository.

## Production-player regression

The E2E launcher **resets its test HOME**. Use an isolated disposable container,
never a user's application data. The scripts expect sources at `/build`, a built
app at `/cache/target/debug/muninn`, Xvfb, dbus, xdotool, ffmpeg and WebKitGTK.
See [VERIFY_0.2.8.md](VERIFY_0.2.8.md) for the local-player commands and assertions.
The live YouTube scenario needs internet access and can change with the website.

## Sidebar and pane layout check

`e2e/sidebar-layout.test.js` drives the UI in its `?mock=1` mode with Playwright (themes,
window sizes, collapsed/expanded sidebar, mini-player on/off; bounding-box and keyboard
assertions, optional screenshots via `SHOTS=dir`). It is not part of CI; it skips when
Playwright is missing. The native YouTube view is only a drawn stand-in there, so macOS
WKWebView layering still needs a manual check. The pane-to-native rectangle maths is
`src-tauri/src/geometry.rs` (pure, unit-tested with `cargo test`).

## Privacy and installation continuity

Keep `io.github.iarnvidr.muninn` stable; the application-data location follows it.
Changing it moves every data directory. A repository move must not create an empty
replacement library or discard login state.
Use a separate test home for experimental builds. Never commit generated local data.
