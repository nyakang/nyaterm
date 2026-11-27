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

Windows 便携版解压后运行 `NyaTerm.exe`，数据保存在旁边的 `data/`。GPUI 的 **Help → 检查更新** 使用签名更新清单，支持适用安装方式的下载和安装；Homebrew、deb/rpm 用户通过包管理器更新。

**从 1.x 迁移：Windows 便携版和旧 MSI 必须手动升级。** 先退出并备份完整数据及密钥；便携版完整覆盖程序文件并保留 `data/`，MSI 备份后卸载旧版再安装 NSIS。不能只复制主程序，也不承诺所有旧安装方式都能自动升级。详见 [2.0 迁移指南](./migration-v2)。

### 按需安装原生组件

主安装包仅包含应用和 MCP helper。SPICE、RDP Builtin、VNC、Android 通过现有插件面板按需安装；Android 组件包含 ADB、投屏 helper 和 FFmpeg。首次连接会先询问是否下载，安装完成后继续仍然有效的连接。Windows Native RDP 使用系统组件，无需下载 RDP Builtin。

插件面板支持查看下载进度、取消、启用/禁用、卸载，以及导入官方签名组件 ZIP。离线使用时，在包路径输入框中填写 ZIP 路径，再点击对应组件的导入按钮。组件必须兼容当前应用版本和目标平台。兼容的已安装版本会在应用升级后保留；组件更新由用户手动执行。

旧版便携包首次迁移到精简包时，请先退出应用并备份完整 `data/`，将精简包解压到新目录，再把完整 `data/` 复制过去；不要复制旧 helper 和运行库。首次使用时重新安装组件。后续精简包更新会保留组件和用户数据。

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

### 从源码构建

从源码构建见 [开发环境搭建](../development/setup)。

## 迁移旧环境

在其他客户端维护过会话时，安装后可以直接导入 Xshell、MobaXterm、WindTerm、SecureCRT、FinalShell、Termius 或 NyaTerm / Electerm JSON。完整格式清单和导入注意事项见 [SSH 连接管理 → 导入其他客户端的会话](../guide/ssh-connection#导入其他客户端的会话)。

要完整恢复一个 NyaTerm 环境，用 `.nya` 加密配置备份而不是会话导入——它恢复的不只是连接列表。`.nya` 导入 / 导出需要先在 **设置 → 安全** 设置主密码，导入后通常需要重启应用。

## 下一步

装好之后从 [快速开始](./quick-start) 继续，它会带你建立第一个连接、认识工作区，并指出哪些设置值得先过一遍。
