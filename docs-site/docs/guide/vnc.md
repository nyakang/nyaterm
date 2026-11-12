# VNC 远程桌面

VNC 会话连接提供 RFB 服务的虚拟机控制台、实验环境或轻量图形桌面。它和 RDP 共用远程桌面 pane，并共享已保存连接、最近使用、标签页和分屏。

VNC 的协议解码运行在独立的 helper 进程中。从源码运行时需要先构建 helper，否则连接会以 `HelperMissing` 失败，见 [开发环境搭建](../development/setup#rdp--vnc-需要先构建-helper)。

## 连接配置

### 基本信息

| 字段 | 说明 |
|------|------|
| 主机 / 端口 | RFB 服务地址，默认端口 `5900` |
| VNC 密码 | classic VNC Authentication 使用的密码 |

### 安全模式

| 模式 | 行为 |
|------|------|
| 自动 | 按服务器声明的安全类型协商（默认） |
| 无 | 不做认证 |
| VNC 密码 | classic VNC Authentication |

classic VNC Authentication 只使用密码的**前 8 字节**。超过 8 字节的密码会被拒绝。

### 显示

**缩放** 可选 **适应**（默认）、**实际大小** 或 **拉伸**。

### 剪贴板

**剪贴板** 默认开启，同步 Latin-1 文本。限制来自 RFB 协议本身：

- 只支持 Latin-1 文本，非 Latin-1 字符会被拒绝
- 单次不超过 1 MiB

这条限制是刻意保留的，避免把二进制或超大内容塞进 VNC 协议路径。

### 会话行为

- **共享会话** — 允许其他查看器保持连接，而不是把它们踢掉
- **仅查看** — 禁用键盘和指针输入

这两个开关在 helper 进程里强制执行，而不只是在界面上隐藏输入。

### 重连

**重连** 默认开启，对临时传输故障做有限次数重试，默认 5 次。连接超时为 15 秒，握手超时为 30 秒。

## 传输与编码支持

当前实现的边界：

- 传输只支持 direct TCP，**没有 TLS / VeNCrypt**，也不支持代理或 SSH 隧道承载
- 画面编码按 `DesktopSizePseudo`、ZRLE、Tight、Raw 的顺序声明；Tight JPEG 在 helper 里解码成统一的 RGBA framebuffer，Raw 保留为稳定 fallback
- 不支持 CopyRect、cursor pseudo-encoding 和远程 resize

## 互通状态

| 场景 | 安全模式 | 编码 | 状态 |
| --- | --- | --- | --- |
| Scripted RFB 3.8 fixture | 无 | ZRLE / Tight / Tight JPEG → RGBA | 已通过自动测试 |
| Scripted RFB 3.8 fixture | VNC 密码 | ZRLE / Tight / Tight JPEG → RGBA | 已通过自动测试 |
| TigerVNC | 无 / VNC 密码 | Raw / ZRLE / Tight / JPEG | 真实服务器待测 |
| TightVNC | 无 / VNC 密码 | Raw / Tight / JPEG | 真实服务器待测 |
| x11vnc / LibVNCServer | 无 / VNC 密码 | Raw / ZRLE / Tight / JPEG | 真实服务器待测 |
| QEMU / KVM VNC | 无 / VNC 密码 | Raw / ZRLE / Tight / JPEG | 真实服务器待测 |

自动测试覆盖的是脚本化的 RFB 3.8 fixture。真实服务器的互通范围仍在验证中，遇到问题请附上服务器实现和版本反馈。

## 能力边界

和 RDP 一样，VNC 会话不提供终端命令历史、SFTP 文件浏览器、SSH 代理 / 跳板机或远程主机监控。

## 错误分类

| 类型 | 常见原因 |
|------|----------|
| `Authentication` | 密码错误；选择 VNC 密码模式但未填写密码 |
| `Protocol` | 服务器 RFB 版本或安全类型不受支持 |
| `Encoding` | 收到无法解码的编码，例如未协商的 pseudo-encoding |
| `Transport` | 端口不可达、连接被拒或链路中断 |
| `Clipboard` | 剪贴板内容超出 Latin-1 或 1 MiB 限制 |
| `HelperMissing` | helper 可执行文件不在应用旁边 |
| `HelperCrashed` | helper 进程在会话期间退出 |
