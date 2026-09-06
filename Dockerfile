# NyaTerm web-server image: windowless Tauri runtime + HTTP/WebSocket bridge
# + static web frontend. See docs-site/docs/development/web.md.

# ---- Stage 1: web frontend (Tauri APIs swapped for HTTP/WS shims) ----------
FROM node:22-bookworm-slim AS web-build
WORKDIR /app
RUN corepack enable
COPY package.json pnpm-lock.yaml pnpm-workspace.yaml ./
COPY scripts ./scripts
COPY src ./src
COPY index.html vite.config.ts tsconfig.json tsconfig.node.json components.json biome.json ./
RUN pnpm install --frozen-lockfile
RUN pnpm build:web

# ---- Stage 2: Rust server binary ------------------------------------------
# Edition 2024 requires Rust >= 1.85; the GUI libs are needed even headless
# because the binary links the Tauri/WRY stack (it runs one hidden webview).
FROM rust:1-bookworm AS rust-build
WORKDIR /build
RUN apt-get update && apt-get install -y --no-install-recommends \
        libwebkit2gtk-4.1-dev \
        libgtk-3-dev \
        libayatana-appindicator3-dev \
        librsvg2-dev \
        libsoup-3.0-dev \
        libjavascriptcoregtk-4.1-dev \
        pkg-config \
    && rm -rf /var/lib/apt/lists/*
COPY src-tauri ./src-tauri
# tauri-build validates the externalBin sidecar; build the real one (it is small).
RUN cargo build --release --manifest-path src-tauri/crates/nyaterm-mcp/Cargo.toml
RUN mkdir -p src-tauri/binaries \
    && cp src-tauri/crates/nyaterm-mcp/target/release/nyaterm-mcp \
          src-tauri/binaries/nyaterm-mcp-x86_64-unknown-linux-gnu
RUN cargo build --release --features server --manifest-path src-tauri/Cargo.toml

# ---- Stage 3: runtime -------------------------------------------------------
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
        xvfb \
        libwebkit2gtk-4.1-0 \
        libgtk-3-0 \
        libayatana-appindicator3-1 \
        librsvg2-2 \
        libsoup-3.0-0 \
        libjavascriptcoregtk-4.1-0 \
        libssl3 \
        ca-certificates \
        fonts-dejavu-core \
    && rm -rf /var/lib/apt/lists/*

COPY --from=rust-build /build/src-tauri/target/release/nyaterm /usr/local/bin/nyaterm
COPY --from=web-build /app/dist /opt/nyaterm/web-dist

ENV NYATERM_WEB_PORT=8080 \
    NYATERM_WEB_DIST=/opt/nyaterm/web-dist \
    NYATERM_WEB_SERVER=1 \
    HOME=/data \
    RUST_LOG=info

VOLUME /data
EXPOSE 8080

# xvfb-run gives the Tauri event loop a virtual display on Linux servers.
ENTRYPOINT ["xvfb-run", "-a", "nyaterm", "--server"]
