---
sidebar_position: 2
---

# 开发环境搭建

## 前置要求

### Node.js

安装 Node.js v18 或更新版本：

- 推荐使用 [nvm](https://github.com/nvm-sh/nvm)（Linux/macOS）或 [nvm-windows](https://github.com/coreybutler/nvm-windows)（Windows）管理版本
- 推荐使用 [pnpm](https://pnpm.io/) 作为包管理器

### Rust

安装最新稳定版 Rust：

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Windows 用户请访问 [rustup.rs](https://rustup.rs/) 下载安装。

### 平台特定依赖

#### Windows

- 安装 [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/)，勾选 "C++ 桌面开发"

#### macOS

```bash
xcode-select --install
```

#### Linux (Ubuntu/Debian)

```bash
sudo apt update
sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget file \
  libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev
```

## 获取源码

```bash
git clone https://git.coderkang.top/Tauri/dragonfly.git
cd dragonfly
```

## 安装依赖

```bash
pnpm install
```

## 启动开发

```bash
pnpm tauri dev
```

这将同时启动：
- Vite 开发服务器（端口 1420，HMR 端口 1421）
- Tauri 应用窗口

修改前端代码会热更新，修改 Rust 代码会自动重新编译。

## 构建发布

```bash
pnpm tauri build
```

构建产物位于 `src-tauri/target/release/bundle/`。

## 开发脚本

| 命令 | 说明 |
|------|------|
| `pnpm dev` | 仅启动 Vite 开发服务器 |
| `pnpm build` | TypeScript 检查 + Vite 构建 |
| `pnpm tauri dev` | 启动 Tauri 开发模式 |
| `pnpm tauri build` | 构建生产版本 |
| `pnpm lint` | 运行 Biome 代码检查 |
| `pnpm format` | 运行 Biome 代码格式化 |
| `pnpm version-sync` | 同步各文件中的版本号 |
| `pnpm --dir docs-site start` | 启动文档站点（所有语言） |
| `pnpm --dir docs-site start:zh` | 启动中文文档开发服务器 |
| `pnpm --dir docs-site start:en` | 启动英文文档开发服务器 |
| `pnpm --dir docs-site build` | 构建文档站点 |

## 文档开发提示

如果你在修改 README 或 `docs-site/docs/` / `docs-site/i18n/en/` 下的文档，建议同时执行文档站点构建，确认：

- 中英文两套文档都能通过构建
- 新增页面已经进入导航
- 相对链接没有失效

## 代码规范

项目使用 [Biome](https://biomejs.dev/) 进行代码检查和格式化：

```bash
# 检查代码
pnpm lint

# 自动格式化
pnpm format
```
