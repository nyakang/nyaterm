# Web 部署

NyaTerm 支持以"无头 Web 服务"方式运行：同一个程序在服务器上启动 `--server` 模式，通过 HTTP/WebSocket 暴露与桌面端完全一致的能力，浏览器打开同一套前端即可使用 SSH / 本地终端 / Telnet / Serial、SFTP、连接与凭据管理、设置、快捷命令、隧道与代理等功能。

## 工作原理

```
浏览器 (React 前端, web 构建)
   │  fetch  POST /api/rpc   { cmd, args }
   │  ws     GET  /api/ws     事件流（terminal-output-*、transfer-event、认证弹窗事件…）
   ▼
nyaterm --server  （真实 Tauri 运行时，无窗口，仅一个隐藏 bridge webview）
   │  复用全部既有 #[tauri::command] 与事件广播
   ▼
core/（SSH/PTY/Telnet/Serial/SFTP/…） + storage/（~/.nyaterm/nyaterm.redb）
```

- **桌面端零改动**：服务模式复用真实的 Tauri 运行时与全部托管状态（`SessionManager` 等），所有命令走与桌面 webview 完全相同的代码路径。
- **前端一套代码**：`NYATERM_WEB_BUILD=1` 构建时，Vite 将 `@tauri-apps/api/*` 与对话框/打开器插件别名到 `src/lib/web/shims/*`（invoke → HTTP RPC，listen/emit → 多路复用 WebSocket）；桌面构建完全不受影响。
- **子窗口变模态**：桌面上的子窗口（设置、新建会话、快捷命令等）在 Web 中渲染为文档内模态，由 `src/lib/web/modalHost.tsx` 承载，页面组件与就绪握手协议完全复用。

## 快速开始（Docker）

```bash
docker run -d --name nyaterm-web \
  -p 8080:8080 \
  -v nyaterm-data:/data \
  ghcr.io/nyaterm/nyaterm-web:latest
```

首次启动会在 `/data/web-token` 生成访问令牌并打印到日志：

```bash
docker logs nyaterm-web | grep -i token
# NyaTerm web server listening on http://0.0.0.0:8080
```

浏览器打开 `http://<host>:8080`，粘贴令牌登录。

## 快速开始（裸二进制）

```bash
# 构建（需要 Rust 1.85+ 与 Tauri Linux 依赖）
cargo build --release --features server --manifest-path src-tauri/Cargo.toml
pnpm build:web

# 运行（Linux 无头环境需要 xvfb 提供虚拟显示）
xvfb-run -a ./src-tauri/target/release/nyaterm --server \
  --dist dist NYATERM_WEB_PORT=8080
```

Windows / macOS 上 `--server` 模式无需虚拟显示；Linux 服务器上 Tauri 事件循环依赖 X11（由 xvfb 提供）。

## 配置

| 环境变量 | 说明 | 默认 |
| --- | --- | --- |
| `NYATERM_WEB_SERVER` | `1` 启用服务模式（等价 `--server` 参数） | 关闭 |
| `NYATERM_WEB_PORT` | HTTP/WS 监听端口 | `8080` |
| `NYATERM_WEB_TOKEN` | 访问令牌；未设置时首启生成到 `<数据目录>/web-token` | 自动生成 |
| `NYATERM_WEB_DIST` | Web 前端静态目录 | 内嵌资源（桌面构建） |
| `NYATERM_WEB_ORIGIN` | （保留）CORS 白名单 | 同源 |

数据（连接、凭据、设置）存放在 `~/.nyaterm/nyaterm.redb`（容器内 `/data`），与桌面端格式一致，可直接迁移。

## 安全

- **认证**：所有 API 需 `Authorization: Bearer <token>`；WebSocket 因浏览器限制改用 `?token=` 查询参数。令牌比较为常量时间实现。
- **传输**：服务本身仅提供 HTTP。生产环境务必置于 TLS 反代之后：

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
    proxy_read_timeout 3600s;                    # 终端长连接
  }
}
```

- **暴露面**：Web 服务等于把终端能力暴露给网络，令牌务必妥善保管；建议仅在内网/VPN 中部署。

## v1 已知限制

- **未开放**：RDP / VNC（帧通道走 Tauri IPC Channel）、AI 助手、录屏回放、托盘与自动更新、ZMODEM 选择对话框、目录级上传/下载对话框（浏览器无法选择服务器目录）。
- **文件对话框**：浏览器选择文件先经 `POST /api/sftp/staging` 暂存到服务器，再走原有上传/下载命令流；下载则由服务器落盘后经 `GET /api/sftp/temp/<name>` 流式送达浏览器，进度事件照常显示。
- **拖放**：终端/文件管理器的拖放在 Web 中通过 HTML5 事件桥接，行为与桌面一致，但大文件会先整份暂存。
- **单用户**：一个部署共享一套连接库；多用户账户体系尚未实现。

## 开发者说明

- 服务端代码全部位于 `src-tauri/src/server/`，cargo feature `server`（`dep:axum`、`dep:tower-http`）门控；桌面默认构建不包含 axum。
- 命令注册表在 `src-tauri/src/server/rpc.rs`：每个 handler 直接调用对应 `#[tauri::command]` 函数（Manager 通过 `app.state()` 获取），参数按 Tauri 同款 camelCase 约定解析。未注册的命令返回明确错误。
- 前端垫片在 `src/lib/web/shims/`，`vite.config.ts` 按 `NYATERM_WEB_BUILD=1` 注入别名；`pnpm build:web`（`scripts/build-web.mjs`）产出 `dist/`。
- 隐藏 bridge 窗口（`public/blank.html`）仅为满足个别命令的 `WebviewWindow` 参数（只读 `window.label()`）。
