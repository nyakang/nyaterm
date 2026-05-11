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
git clone https://github.com/nyakang/nyaterm.git
cd nyaterm
```

## 安装依赖

```bash
pnpm install
```

## 启动开发

### 启动完整桌面应用

```bash
pnpm tauri dev
```

这将同时启动：

- Vite 开发服务器（端口 1420，HMR 端口 1421）
- Tauri 应用窗口

修改前端代码会热更新，修改 Rust 代码会自动重新编译。

### 只启动前端

```bash
pnpm dev
```

适合只改界面或排查前端布局问题。

### 启动文档站点

```bash
pnpm --dir docs-site start
```

如果只想针对某个语言编辑：

```bash
pnpm --dir docs-site start:zh
pnpm --dir docs-site start:en
```

## 常用检查与构建命令

### 前端 / 根项目

| 命令 | 说明 |
|------|------|
| `pnpm build` | TypeScript 检查 + Vite 构建 |
| `pnpm lint` | 运行 Biome 代码检查 |
| `pnpm format` | 运行 Biome 代码格式化 |
| `pnpm format:check` | 检查 Biome 格式，不写回 |
| `pnpm i18n:check` | 检查 locale JSON 格式 |
| `pnpm i18n:fix` | 修复 locale JSON 格式 |
| `pnpm version-sync` | 同步各文件中的版本号 |
| `pnpm release` | 版本同步 + 前端构建 + Tauri 构建 |

### Rust / Tauri 后端

| 命令 | 说明 |
|------|------|
| `cargo fmt --manifest-path src-tauri/Cargo.toml` | 格式化 Rust 代码 |
| `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets` | 运行 Rust lint |
| `cargo test --manifest-path src-tauri/Cargo.toml` | 运行后端测试 |
| `cargo test --manifest-path src-tauri/crates/otp/Cargo.toml` | 运行 OTP crate 测试 |

### 文档站点

| 命令 | 说明 |
|------|------|
| `pnpm --dir docs-site start` | 启动文档站点（所有语言） |
| `pnpm --dir docs-site start:zh` | 启动中文文档开发服务器 |
| `pnpm --dir docs-site start:en` | 启动英文文档开发服务器 |
| `pnpm --dir docs-site build` | 构建文档站点 |

## 构建发布

```bash
pnpm tauri build
```

构建产物位于 `src-tauri/target/release/bundle/`。

## 文档开发提示

如果你在修改 README 或 `docs-site/docs/` / `docs-site/i18n/en/` 下的文档，建议至少执行：

```bash
pnpm --dir docs-site build
```

这样可以尽早发现：

- 中英文文档是否都能通过构建
- 新增页面是否进入导航
- 相对链接是否失效
- Markdown 或 frontmatter 是否有语法问题

## 代码规范

### 前端

- 使用 TypeScript 严格模式
- 优先复用 `src/components/ui/` 中的共享组件
- 新增或修改 UI 文本时，同时更新 `src/i18n/locales/zh-CN.json` 和 `src/i18n/locales/en.json`

### 后端

- 遵循 Rust 标准编码风格
- 新增 Tauri command 时，同时检查 `src-tauri/src/lib.rs` 的注册项
- 变更设置或持久化结构时，同时确认前端默认值和 Rust 配置迁移逻辑
