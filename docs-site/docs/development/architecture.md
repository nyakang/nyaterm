---
sidebar_position: 1
---

# 架构说明

Dragonfly 是一个基于 **Tauri 2** 的桌面应用：前端在 `src/`，后端在 `src-tauri/src/`，两者通过 Tauri command 与事件通信。

## 整体架构

```text
┌─────────────────────────────────────────────────────────┐
│ Frontend (React / TypeScript)                          │
│  ├─ Main window: AppProvider + App.tsx                 │
│  ├─ Child windows: ChildAppProvider + ChildWindowRouter│
│  ├─ Terminal workspace, side panels, dialogs           │
│  └─ invoke wrapper + Tauri event listeners             │
├─────────────────────────────────────────────────────────┤
│ Tauri IPC bridge                                       │
├─────────────────────────────────────────────────────────┤
│ Backend (Rust)                                         │
│  ├─ SessionManager / TunnelManager / RecordingManager  │
│  ├─ PendingAuthManager                                 │
│  ├─ SSH / SFTP / watcher / importer / stats            │
│  └─ JSON config + encrypted credential storage         │
└─────────────────────────────────────────────────────────┘
```

## 前端窗口模型

前端入口由 `src/main.tsx` 决定：

- **主窗口**：加载 `AppProvider` 与 `App.tsx`
- **子窗口**：加载 `ChildAppProvider` 与 `ChildWindowRouter`

当前子窗口流程包括：

- 设置
- 新建连接
- 快捷命令编辑
- 自动上传提示

相关实现位置：

- `src/main.tsx`
- `src/ChildWindowRouter.tsx`
- `src/lib/windowManager.ts`

其中 `windowManager.ts` 还负责 modal child 与主窗口之间的焦点 / 可交互状态协调。

## 前端状态模型

### AppContext

`src/context/AppContext.tsx` 是主窗口的核心状态容器，负责：

- 工作区标签与窗格树
- 应用设置与 UI 设置
- 已保存连接 / 分组刷新
- 启动时恢复 `ui.open_tabs`

### ChildAppProvider

`src/context/ChildAppProvider.tsx` 是子窗口专用的轻量 Provider：

- 只加载 / 保存设置
- 通过事件与主窗口同步
- 不管理完整的工作区标签与会话状态

### TransferContext

`src/context/TransferContext.tsx` 单独管理文件传输队列，消费后端 `transfer-event` 事件，并驱动暂停、继续、取消、重试等前端行为。

## 工作区模型

Dragonfly 的终端工作区有两个容易混淆、但职责不同的层次：

### 逻辑标签 / 窗格树

`src/lib/workspaceTabs.ts` 负责：

- 创建标签页与会话 pane
- 标签页内横向 / 纵向分屏
- 持久化 `ui.open_tabs`
- 启动时恢复可序列化的工作区结构

### 运行时窗口布局

`src/lib/tabWindows.ts` 负责：

- 不同标签当前分布在哪个 window leaf 中
- 每个 leaf 的活动标签
- 运行时窗口 split ratio

可以简单理解为：

- `workspaceTabs.ts` = “会保存下来的逻辑工作区”
- `tabWindows.ts` = “当前运行时终端区域怎么摆”

## 终端集成

`src/components/terminal/XTerminal.tsx` 是 xterm.js 集成中心，负责：

- Fit/Search/WebLinks 等 addon
- 命令历史建议与 shell integration
- 行号 / 时间戳 gutter
- 动作链接与关键词高亮
- 大输出保护与恢复提示
- 与 session 事件的绑定和重连处理

## 后端运行时模型

`src-tauri/src/lib.rs` 是后端入口，负责构建并注入共享状态：

- `SessionManager`
- `TunnelManager`
- `RecordingManager`
- `PendingAuthManager`
- `CloudSyncManager`

同时也在这里集中注册所有 Tauri commands，例如：

- session 创建 / 关闭 / 写入 / 录制 / OTP
- SFTP 文件与传输操作
- 连接 / 密钥 / 密码 / OTP / 设置读写
- cloud sync / backup 状态、推送、拉取、恢复、冲突处理
- watcher、翻译、导入、stats、tunnel、proxy

## SessionManager 与事件流

`src-tauri/src/core/session.rs` 中的 `SessionManager` 是活动会话注册中心，负责：

- 管理所有活动会话
- 向具体 session I/O loop 路由命令
- 维护命令历史与模糊搜索存储
- 发出 `sessions-changed`、`command-history-changed` 等事件

后端还会向前端发送这些典型事件：

| 事件 | 说明 |
|------|------|
| `terminal-output-{id}` | 终端输出 |
| `cwd-changed-{id}` | 工作目录变化 |
| `session-closed-{id}` | 会话关闭 |
| `sessions-changed` | 会话列表变化 |
| `connections-changed` | 已保存连接变化 |
| `transfer-event` | 传输队列进度变化 |
| `otp-request` | 触发 OTP / keyboard-interactive 认证 |
| `cloud-sync-status-changed` | 云同步 / 备份状态变化 |
| `cloud-sync-history-changed` | 同步 / 备份历史变化 |
| `cloud-sync-conflict` | 云同步冲突预览与处理入口 |

## SSH / SFTP / watcher / 导入

核心后端能力主要分布在这些模块：

- `src-tauri/src/core/ssh/` — SSH 连接、认证、OSC/CWD、SFTP、隧道
- `src-tauri/src/core/pty.rs` — 本地终端
- `src-tauri/src/core/telnet.rs` — Telnet
- `src-tauri/src/core/serial.rs` — 串口
- `src-tauri/src/core/watcher.rs` — 本地文件监听与自动上传流程
- `src-tauri/src/core/importer.rs` — Xshell / MobaXterm / WindTerm 导入
- `src-tauri/src/core/recording.rs` — 会话录制
- `src-tauri/src/core/cloud_sync.rs` — 云同步、远程备份、状态事件、冲突处理
- `src-tauri/src/core/portable_snapshot.rs` — 可移植快照构建 / 应用与同步范围控制

## 配置与持久化

应用配置保存在 `~/.dragonfly/` 下，典型文件包括：

- `settings.json`
- `sessions.json`
- `keys.json`
- `passwords.json`
- `otp.json`
- `quick-command.json`
- `tunnels.json`
- `proxies.json`
- `history.json`
- `known_hosts`
- `cloud_sync_state.json`

其中敏感值会先加密再写盘，因此前端管理的是可复用凭据条目，而不是明文配置。

云同步功能本身还有两层额外模型：

- `src-tauri/src/config/cloud_sync.rs` 负责 provider 配置、运行状态和敏感字段加密 / mask / merge
- `src-tauri/src/core/portable_snapshot.rs` 定义哪些配置会进入可移植快照，哪些设备本地 UI 状态会保留在本机
