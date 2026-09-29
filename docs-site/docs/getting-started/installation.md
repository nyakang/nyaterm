---
sidebar_position: 1
---

# 安装指南

## 系统要求

NyaTerm 支持以下操作系统：

- **Windows** 10 版本 1809（Build 17763）或更高版本 / Windows 11 (64-bit)
- **macOS** 12+ (Intel & Apple Silicon)
- **Linux**（Ubuntu 20.04+、Fedora 36+、Arch Linux 等）

Windows 本地终端优先使用随应用分发的 Microsoft ConPTY；如果加载或创建失败，则使用系统 ConPTY。系统仍须提供 ConPTY API，因此 Windows 10 1809（Build 17763）是最低支持版本。可在“关于”中的支持信息查看实际使用的版本。

## 下载安装

### 从发布页面下载

前往 [Releases](https://github.com/nyakang/nyaterm/releases) 页面，下载适合你操作系统的安装包：

| 平台 | 安装包格式 |
|------|-----------|
| Windows | `.msi` / `.exe` / 便携版 `.zip` |
| macOS | `.dmg` |
| Linux | `.deb` / `.AppImage` |

Windows 便携版解压后运行 `NyaTerm.exe` 即可。**Help → 检查更新** 与安装版共用 Cloudflare R2 更新清单和下载源，暂存更新前会强制验证 Tauri updater 签名。重启时 NyaTerm 会自动替换程序文件，并完整保留 `data/` 目录。

首次升级到包含 bundled ConPTY 的便携版时，旧版更新器无法处理新增的 `conpty/` 目录。请手动下载新版便携包，替换程序文件并保留原有 `data/` 目录；之后可继续使用应用内更新。

Windows 便携版直接下载：

- [NyaTerm_1.1.18_windows_x64_portable.zip](https://downloads.nyaterm.app/releases/v1.1.18/NyaTerm_1.1.18_windows_x64_portable.zip)（x64）
- [NyaTerm_1.1.18_windows_arm64_portable.zip](https://downloads.nyaterm.app/releases/v1.1.18/NyaTerm_1.1.18_windows_arm64_portable.zip)（ARM64）

### macOS

macOS 用户可以通过 Homebrew 安装 NyaTerm：

```bash
brew install nyakang/nyaterm/nyaterm
```

该命令会使用 [`nyakang/homebrew-nyaterm`](https://github.com/nyakang/homebrew-nyaterm) tap，并安装 `nyaterm` cask。也可以从 [nyaterm.app](https://nyaterm.app) 或 [Releases](https://github.com/nyakang/nyaterm/releases) 下载 `.dmg` 安装包，然后将 NyaTerm 拖入 `/Applications`。

NyaTerm 目前还没有使用 Apple Developer 证书签名。安装后如果 macOS 提示应用已损坏或无法打开，可以移除 quarantine 属性后再打开：

```bash
sudo xattr -cr /Applications/NyaTerm.app
```

### Linux (Nix / NixOS)

如果你使用 Nix 包管理器或 NixOS，可以通过仓库提供的 Flake 直接运行或安装：

```bash
# 免安装直接运行
nix run github:nyakang/nyaterm

# 安装到用户环境
nix profile install github:nyakang/nyaterm
```

说明：
- Nix 打包版本已自动屏蔽应用内置的自更新检查，版本升级由 Nix 统一管理。
- 完整包含应用本体与 `nyaterm-mcp` sidecar，并已配置桌面图标及 `nyaterm://`、`ssh://`、`telnet://` 协议关联。
- 运行时已自动封装 WebKitGTK、GTK3、xdg-utils 等依赖，并通过 `SSL_CERT_FILE` 默认提供 Nixpkgs CA 证书库（若宿主环境自定义了 `SSL_CERT_FILE` 则优先遵循宿主配置）。
- 如需使用 Termius 等凭据导入功能，建议桌面环境中运行有 Secret Service 提供者（如 gnome-keyring 或 keepassxc）。
- WebDAV 与 S3 云同步开箱即用；默认 Flake 构建未内置 GitHub OAuth Client ID。若需使用 GitHub Gist 云同步授权，可在 NixOS / Home Manager 中覆盖传入：

```nix
# NixOS（configuration.nix）
environment.systemPackages = [
  (inputs.nyaterm.packages.${pkgs.system}.nyaterm.override {
    githubGistClientId = "your_client_id";
  })
];

# 或导入 Flake 导出的 overlay 后在 NixOS 中使用：
nixpkgs.overlays = [ inputs.nyaterm.overlays.default ];
environment.systemPackages = [
  (pkgs.nyaterm.override {
    githubGistClientId = "your_client_id";
  })
];

# Home Manager（home.nix）
home.packages = [
  (inputs.nyaterm.packages.${pkgs.system}.nyaterm.override {
    githubGistClientId = "your_client_id";
  })
];

# 或在 Home Manager 中配合 overlay 使用：
nixpkgs.overlays = [ inputs.nyaterm.overlays.default ];
home.packages = [
  (pkgs.nyaterm.override {
    githubGistClientId = "your_client_id";
  })
];
```

### 从源码构建

如果你想从源码构建，请参考 [开发环境搭建](../development/setup) 章节。

## 首次启动后会看到什么

安装完成后启动 NyaTerm，主窗口通常会由这些区域组成：

- **顶部菜单与窗口栏** — File / View / Help、窗口控制与应用级入口
- **中央工作区** — 终端标签页，以及标签内横向 / 纵向分屏
- **左侧活动栏与面板** — 文件浏览器、网络、Security/Auth、云同步、设置等能力入口
- **右侧活动栏与面板** — 已保存连接、AI Assistant、活动会话、命令历史、资源监控等运行态信息
- **底部辅助区** — 快捷命令、串口发送、录制、锁屏等辅助操作

某些流程会打开独立子窗口，而不是打断主工作区，例如：

- 设置
- 新建连接
- 快捷命令编辑
- 自动上传提示

## 安装后建议先检查的设置

建议在正式投入使用前，先快速浏览以下几项：

- **设置 → 常规**：启动恢复、关闭时最小化到托盘、关闭确认
- **设置 → 常规**：日志级别、日志保留时间、打开日志目录、导出诊断包
- **设置 → 交互**：命令建议开关、历史命令长度范围、复制与右键粘贴、macOS IME 兼容
- **设置 → 终端**：回滚缓冲区、Keep-Alive、动作链接、行号 / 时间戳、关键词高亮、资源监控、工作区间距、字体粗细、图片路径粘贴
- **设置 → 传输**：默认下载目录、默认编辑器、录制保存路径、并发、重试与重复目标策略
- **设置 → 安全**：主密码、锁屏、空闲自动锁定、主机密钥策略
- **设置 → AI**：provider、模型、风险控制、历史记录与上下文限制

如果你经常在后台保留会话或同步任务，**关闭时最小化到托盘** 值得优先确认。

## 如果你要迁移旧环境

如果你之前已经在其他工具中维护会话，安装后可以直接导入：

- **Xshell**（`.xts`）
- **MobaXterm**（`.mxtsessions`）
- **WindTerm**（`.sessions`）
- **NyaTerm** 的 `.nya` 加密配置备份

如果你的目标是完整恢复 NyaTerm 配置，优先使用 `.nya`；它会恢复的不只是连接，还包括更多本地配置数据。

需要注意：

- `.nya` 导入 / 导出需要先设置主密码
- 导入 `.nya` 后通常需要重启应用
- 普通会话导入更适合迁移连接列表，不会替代完整配置恢复

## 第一次体验建议

如果你是第一次使用，建议按这个顺序体验：

1. 打开 [快速开始](./quick-start)
2. 创建一个 **SSH** 连接，或先导入已有会话
3. 再创建一个 **本地终端**，体验混合工作区
4. 在 SSH 会话里打开文件浏览器和传输面板
5. 试试命令历史、快捷命令、AI 助手、录制与终端搜索
6. 如果你在 Windows 上，可额外尝试把本地文件或文件夹直接拖到文件浏览器中上传
