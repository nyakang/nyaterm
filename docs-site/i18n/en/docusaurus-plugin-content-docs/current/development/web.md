# Web Deployment

NyaTerm can run headless as a web service: the same binary started with `--server` exposes the exact same capabilities as the desktop app over HTTP/WebSocket, and the same frontend runs in a browser — SSH / local shells / Telnet / Serial, SFTP, connection & credential management, settings, quick commands, tunnels and proxies.

## How it works

```
Browser (React frontend, web build)
   │  fetch  POST /api/rpc   { cmd, args }
   │  ws     GET  /api/ws     event stream (terminal-output-*, transfer-event, auth dialogs…)
   ▼
nyaterm --server  (real Tauri runtime, windowless, one hidden bridge webview)
   │  reuses every existing #[tauri::command] and event broadcast
   ▼
core/ (SSH/PTY/Telnet/Serial/SFTP/…) + storage/ (~/.nyaterm/nyaterm.redb)
```

- **Zero changes to desktop logic**: server mode reuses the real Tauri runtime and all managed state (`SessionManager` etc.); every command runs through the same code path the desktop webview uses.
- **One frontend codebase**: when building with `NYATERM_WEB_BUILD=1`, Vite aliases `@tauri-apps/api/*` and the dialog/opener plugins to `src/lib/web/shims/*` (invoke → HTTP RPC, listen/emit → multiplexed WebSocket). Desktop builds are unaffected.
- **Child windows become modals**: desktop child windows (settings, new-session, quick-command…) render as in-document modals hosted by `src/lib/web/modalHost.tsx`, reusing the real page components and ready-handshake protocol.

## Quick start (Docker)

```bash
docker run -d --name nyaterm-web \
  -p 8080:8080 \
  -v nyaterm-data:/data \
  ghcr.io/nyaterm/nyaterm-web:latest
```

A first start generates an access token at `/data/web-token` and prints it:

```bash
docker logs nyaterm-web | grep -i token
# NyaTerm web server listening on http://0.0.0.0:8080
```

Open `http://<host>:8080` in a browser and paste the token.

## Quick start (bare binary)

```bash
# Build (Rust 1.85+ and the Tauri Linux dependencies required)
cargo build --release --features server --manifest-path src-tauri/Cargo.toml
pnpm build:web

# Run (headless Linux needs xvfb for a virtual display)
xvfb-run -a ./src-tauri/target/release/nyaterm --server \
  --dist dist NYATERM_WEB_PORT=8080
```

On Windows/macOS `--server` needs no virtual display; on Linux servers the Tauri event loop requires X11 (provided by xvfb).

## Configuration

| Environment variable | Description | Default |
| --- | --- | --- |
| `NYATERM_WEB_SERVER` | `1` enables server mode (same as `--server`) | off |
| `NYATERM_WEB_PORT` | HTTP/WS listen port | `8080` |
| `NYATERM_WEB_TOKEN` | Access token; auto-generated into `<data dir>/web-token` on first start when unset | auto |
| `NYATERM_WEB_DIST` | Directory with the web frontend build | embedded assets (desktop build) |
| `NYATERM_WEB_ORIGIN` | (reserved) CORS allowlist | same-origin |

Data (connections, credentials, settings) lives in `~/.nyaterm/nyaterm.redb` (`/data` in the container) — the same format as the desktop app, so it can be migrated directly.

## Security

- **Auth**: every API call requires `Authorization: Bearer <token>`; the WebSocket uses a `?token=` query parameter (browsers cannot set WS headers). Token comparison is constant-time.
- **Transport**: the server speaks plain HTTP. Always terminate TLS at a reverse proxy in production:

```nginx
server {
  listen 443 ssl;
  server_name nyaterm.example.com;
  ssl_certificate     /etc/ssl/certs/nyaterm.pem;
  ssl_certificate_key /etc/ssl/private/nyaterm.key;

  location / {
    proxy_pass http://127.0.0.1:8080;
    proxy_http_version 1.1;
    proxy_set_header Upgrade $http_upgrade;      # WebSocket
    proxy_set_header Connection "upgrade";
    proxy_set_header Host $host;
    proxy_read_timeout 3600s;                    # long-lived terminal connections
  }
}
```

- **Exposure**: the web service equals terminal access on your network — guard the token and prefer intranet/VPN deployments.

## v1 known limitations

- **Not exposed**: RDP / VNC (frame channels use Tauri IPC `Channel`), AI assistant, recording playback, tray & auto-update, ZMODEM picker dialogs, directory transfer dialogs (browsers cannot pick server-side directories).
- **File dialogs**: browser-selected files are staged to the server via `POST /api/sftp/staging` and then flow through the regular upload/download commands; downloads materialize server-side and stream to the browser via `GET /api/sftp/temp/<name>`, with transfer progress events intact.
- **Drag & drop**: bridged to HTML5 events with identical behavior, but large files are staged in full first.
- **Single user**: one deployment shares one connection library; multi-user accounts are not implemented yet.

## Developer notes

- Server code lives entirely in `src-tauri/src/server/`, gated by the cargo feature `server` (`dep:axum`, `dep:tower-http`); desktop default builds exclude axum.
- The command registry is in `src-tauri/src/server/rpc.rs`: every handler calls the corresponding `#[tauri::command]` function directly (managers via `app.state()`), with arguments parsed using the same camelCase convention as Tauri. Unregistered commands return an explicit error.
- Frontend shims live in `src/lib/web/shims/`; `vite.config.ts` injects the aliases when `NYATERM_WEB_BUILD=1`; `pnpm build:web` (`scripts/build-web.mjs`) produces `dist/`.
- The hidden bridge window (`public/blank.html`) exists only to satisfy the `WebviewWindow` parameter of a handful of commands (they only read `window.label()`).
