# Base toolchain (Node, Rust) for build-macos.sh; Dockerfile.macos adds the
# macOS cross-compilation pieces on top.
FROM ubuntu:22.04

ENV DEBIAN_FRONTEND=noninteractive \
    RUSTUP_HOME=/opt/rustup \
    CARGO_HOME=/opt/cargo \
    PATH=/opt/cargo/bin:/opt/node/bin:$PATH

RUN apt-get update && apt-get install -y --no-install-recommends \
      build-essential ca-certificates clang curl dpkg-dev file git libayatana-appindicator3-dev \
      librsvg2-dev libssl-dev libwebkit2gtk-4.1-dev lld llvm patchelf pkg-config python3 unzip xz-utils \
    && rm -rf /var/lib/apt/lists/*

ARG NODE_VERSION=22.23.3
RUN mkdir /opt/node \
    && curl -fsSL "https://nodejs.org/dist/v${NODE_VERSION}/node-v${NODE_VERSION}-linux-x64.tar.xz" \
       | tar -xJ --strip-components=1 -C /opt/node

ARG RUST_VERSION=1.99.1
RUN curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain "$RUST_VERSION" \
    && rm -rf /opt/cargo/registry

# The directories are created here because named volumes copy their ownership
# from the image on first use.
RUN mkdir -p /opt/cargo/registry /build /cache/target \
    && chmod -R a+rwX /opt/rustup /opt/cargo /build /cache
