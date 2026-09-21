# 安装指南

## 系统要求

NyaTerm 支持以下操作系统：

- **Windows** 10/11（x64 / ARM64）
- **macOS** 12+（Intel / Apple Silicon）
- **Linux**（x64 / ARM64；Ubuntu 20.04+、Fedora 36+、Arch Linux 等）

NyaTerm 用 GPUI 做原生 GPU 渲染，因此对图形环境有要求：

- **Linux**：需要可用的 Vulkan 驱动（例如 `libvulkan1` 和 `mesa-vulkan-drivers`），以及 X11 或 Wayland 会话。缺少 Vulkan 驱动时应用无法启动
- **macOS**：使用 Metal，系统自带
- **Windows**：使用系统图形驱动，通常无需额外安装

NyaTerm 是桌面客户端而不是终端复用器，在纯 SSH 的无头服务器上无法运行。

## 下载安装

### 从发布页面下载

前往 [Releases](https://github.com/nyakang/nyaterm/releases) 页面，按操作系统下载安装包：

| 平台 | 安装包格式 |
|------|-----------|
| Windows | 安装版 `-setup.exe` / 便携版 `.zip` |
| macOS | `.dmg` / `.app.tar.gz` |
| Linux | `.deb` / `.AppImage` / `.rpm` |

Windows 便携版解压后运行 `NyaTerm.exe` 即可，配置数据保存在同目录的 `data/`。

**Help → 检查更新** 只检查 GitHub Releases 并提供页面入口，不会自动下载或替换程序文件。更新便携版时关闭 NyaTerm 后手动替换程序文件，保留 `data/` 目录。

以前安装的 Tauri 安装版可以通过原有签名更新器迁移一次到 GPUI 安装版。旧 Tauri 便携更新器只允许更新单个主程序，无法携带 GPUI 必需的 RDP/VNC helper；请下载完整的 GPUI 便携 ZIP，并将旧目录中的 `data/` 复制到新目录，不能只替换 `NyaTerm.exe`。

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

### Nix / NixOS

你可以使用 Nix Flakes 直接运行或安装 NyaTerm：

```bash
# 直接运行
nix run github:nyakang/nyaterm

# 安装到用户环境 profile
nix profile install github:nyakang/nyaterm
```

若从本地仓库源码构建：

```bash
nix build
```

### 从源码构建

从源码构建见 [开发环境搭建](../development/setup)。

## 迁移旧环境

在其他客户端维护过会话时，安装后可以直接导入 Xshell、MobaXterm、WindTerm、SecureCRT、FinalShell、Termius 或 NyaTerm / Electerm JSON。完整格式清单和导入注意事项见 [SSH 连接管理 → 导入其他客户端的会话](../guide/ssh-connection#导入其他客户端的会话)。

要完整恢复一个 NyaTerm 环境，用 `.nya` 加密配置备份而不是会话导入——它恢复的不只是连接列表。`.nya` 导入 / 导出需要先在 **设置 → 安全** 设置主密码，导入后通常需要重启应用。

## 下一步

装好之后从 [快速开始](./quick-start) 继续，它会带你建立第一个连接、认识工作区，并指出哪些设置值得先过一遍。
