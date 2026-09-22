# 架构说明

NyaTerm 是一个基于 **GPUI** 构建的原生 Rust 桌面应用。应用界面、终端模拟、连接传输和持久化均位于同一个 Cargo workspace 中，不依赖浏览器运行时或 IPC 桥接层。

## 整体分层

```text
nyaterm-app
  └─ 启动 GPUI、注册资源、创建根窗口
       └─ nyaterm-desktop
            ├─ AppShell / NyaTermApp / feature state / views
            ├─ nyaterm-ui                共享 GPUI 控件和主题
            ├─ nyaterm-terminal-gpui     终端布局、输入和绘制
            ├─ nyaterm-terminal          终端状态机、解析和快照
            ├─ nyaterm-transport         PTY、SSH、SFTP 和其他协议
            ├─ nyaterm-remote-desktop    RDP/VNC 会话管理与 helper IPC 合约
            ├─ nyaterm-store             redb、事务和兼容性读取
            └─ nyaterm-core              纯模型、格式和策略

  独立进程（通过 IPC 通信，不被应用链接）
       ├─ nyaterm-rdp-helper            IronRDP 解码
       └─ nyaterm-vnc-helper            vnc-rs 解码
```

各 crate 的主要职责如下：

| Crate | 职责 |
|------|------|
| `nyaterm-app` | 可执行入口、日志、嵌入资源和根窗口创建 |
| `nyaterm-desktop` | GPUI 应用组合、状态、视图、平台适配和后台任务协调 |
| `nyaterm-ui` | 共享控件、主题 token 和 `gpui-kit` 集成边界 |
| `nyaterm-terminal` | 与 UI 无关的终端状态机、控制序列、编码和图形协议 |
| `nyaterm-terminal-gpui` | GPUI 终端输入、布局、选区、高亮、图片和绘制 |
| `nyaterm-transport` | PTY、SSH、Telnet、串口、SFTP、隧道、远程操作和传输协议 |
| `nyaterm-store` | redb 数据库、事务、加密适配和兼容性读取 |
| `nyaterm-core` | 领域模型、兼容格式、解析、策略及纯逻辑 |
| `nyaterm-remote-desktop` | 与 UI 无关的 RDP/VNC 会话管理、framebuffer 与输入模型、证书策略、剪贴板状态和 helper IPC 合约 |
| `nyaterm-rdp-helper` | 隔离的 IronRDP helper 进程 |
| `nyaterm-vnc-helper` | 隔离的 VNC helper 进程，持有 fork 的 `vnc-rs` 解码器、重连梯度和服务端策略门控 |
| `nyaterm-otp` | HOTP/TOTP 兼容实现 |

## 启动流程

`crates/nyaterm-app/src/main.rs` 是应用入口：

1. 解析运行目录并初始化日志。
2. 向 GPUI 注册嵌入资源和共享组件。
3. 创建原生根窗口和 `AppShell` Entity。
4. `AppShell` 启动 `StoreRuntime`，异步加载启动快照。
5. 数据验证成功后创建 `NyaTermApp`，再恢复窗口布局和会话。

`AppShell` 还负责加载、恢复、退出前 flush 等应用级生命周期。数据库启动失败时会进入恢复界面，而不是用未验证的数据继续创建主应用状态。

## 状态所有权

`NyaTermApp` 是 GPUI 组合中心，但主要 UI 域由 focused feature state 管理，例如连接、会话、终端、传输、设置、安全、AI、同步和远程操作。

每份状态只有一个可写 owner。当前独立 Entity store 只拥有 `NyaTermApp` 不拥有的状态：

- `StartupRestoreStore`：启动恢复队列
- `OverlayStore`：快速切换 overlay

视图直接读取权威状态构造 GPUI element。不要建立同帧 publish/read-back 的只读镜像，也不要在 feature state 和 Entity 中同时保存可变副本。

## 后台任务与事件

文件系统、数据库、网络、SSH、SFTP、子进程和图片解码等阻塞工作不在 render 路径中执行。

后台任务通过有类型的结果或事件返回 GPUI 状态层。例如会话运行时使用 `nyaterm_transport::SessionEvent` 传递输出、工作目录变化、命令确认、退出和错误；桌面层在窗口运行时 pump 中消费事件、更新 feature state 并通知 GPUI 重绘。

## 终端数据流

```text
PTY / SSH / Telnet / Serial
        │
        ▼
nyaterm-transport typed events
        │
        ▼
nyaterm-desktop event drain and session state
        │
        ▼
nyaterm-terminal state machine and snapshots
        │
        ▼
nyaterm-terminal-gpui layout, input and painting
```

`nyaterm-terminal` 使用 Alacritty 的终端组件维护网格和控制序列状态，同时负责搜索、编码、Kitty graphics 和 Sixel 等与 UI 无关的逻辑。GPUI 尺寸计算、键盘适配、选区、高亮、图片及逐帧绘制均留在 `nyaterm-terminal-gpui`。

## 持久化与兼容性

`nyaterm-store` 通过专用 `StoreRuntime` 执行数据库工作，桌面层使用 UI client 或 blocking client 提交有类型的请求。GPUI 视图不直接访问 redb。

配置模型、备份格式、云同步文档和加密策略等 schema-neutral 合约位于 `nyaterm-core`。数据库实现与旧数据读取位于 `nyaterm-store`。现有 table 名、key、字段名、加密前缀、`.nya` 备份和 Dragonfly fallback 都属于兼容性边界。

## RDP / VNC 的进程隔离

RDP 和 VNC 的协议解码器解析的是**服务器控制的字节流**，因此它们不在应用链接的任何 crate 里，而是各自跑在独立的 helper 进程中，通过 `nyaterm-remote-desktop` 里的类型化 IPC 协议与应用通信。

这条边界的含义：

- 解码器崩溃不会带走应用。两个 helper 都必须把解码器 panic 转换成一个致命 IPC 错误上报，而不是静默退出
- `nyaterm-remote-desktop` 自己不含任何解码器。它只负责会话管理、framebuffer 与输入模型、证书策略和剪贴板状态
- VNC 的服务端策略门控（`view_only`、`shared`、剪贴板启用）必须在 helper 里强制执行，而不是只在应用侧判断
- 应用在自己的可执行文件旁边解析 helper 路径，`NYATERM_RDP_HELPER` / `NYATERM_VNC_HELPER` 可覆盖

两个 helper crate 各自带一个 `tests/lifecycle.rs`，覆盖握手、正常断开和崩溃 / 挂起回收。IPC 合约变更时必须同步更新两边。

## 依赖规则

- `nyaterm-core`、`nyaterm-terminal`、`nyaterm-transport` 和 `nyaterm-remote-desktop` 不依赖 GPUI。
- 解析服务器控制字节的协议解码器只放在 helper crate 里，不放进应用链接的 crate。
- 桌面功能通过 `nyaterm-ui` 使用普通输入、选择、菜单、开关和对话框。
- 模块使用正常 Rust module tree 和显式 import。
- 新功能优先放入已有 focused feature state；只有需要独立生命周期时才新增权威 Entity。

更具体的界面层规则见 [GPUI 桌面开发](./frontend)，运行时与持久化规则见 [运行时、传输与存储开发](./backend)。
