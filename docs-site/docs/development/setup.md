# 开发环境搭建

NyaTerm 应用本身是 Cargo workspace。Node.js 和 pnpm 只用于构建本仓库中的 Docusaurus 文档站点。

## 应用开发前置要求

### Rust 与 Git

安装最新稳定版 Rust：

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Windows 用户可从 [rustup.rs](https://rustup.rs/) 安装。所有平台还需要 Git 和目标平台的原生编译工具链。

### 平台依赖

#### Windows

安装 [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/)，并选择“使用 C++ 的桌面开发”工作负载。

#### macOS

安装 Xcode 和命令行工具：

```bash
xcode-select --install
```

GPUI 在 macOS 上使用 Metal，因此需要可用的 macOS SDK。

#### Linux（Ubuntu / Debian）

安装 Rust crate 编译工具、字体/窗口系统开发库和 Vulkan loader：

```bash
sudo apt update
sudo apt install build-essential clang pkg-config cmake \
  libfontconfig1-dev libfreetype6-dev libssl-dev libudev-dev \
  libwayland-dev libx11-dev libx11-xcb-dev \
  libxcb-cursor-dev libxcb-icccm4-dev libxcb-image0-dev \
  libxcb-keysyms1-dev libxcb-randr0-dev libxcb-render0-dev \
  libxcb-shape0-dev libxcb-xfixes0-dev libxcb-xinerama0-dev \
  libxkbcommon-dev libxkbcommon-x11-dev libzstd-dev \
  libvulkan1 mesa-vulkan-drivers
```

运行桌面应用还需要可用的 Vulkan 驱动，以及 X11 或 Wayland 会话。

## 获取源码

```bash
git clone https://github.com/nyakang/nyaterm.git
cd nyaterm
```

应用依赖全部由 Cargo 管理。仓库里的 Node.js 依赖只属于 `docs-site`。

## 启动应用

```bash
cargo run -p nyaterm-app --bin nyaterm
```

首次编译会构建 GPUI 和全部依赖，耗时明显长于后续增量构建。

### RDP / VNC 需要先构建 helper

RDP 和 VNC 各自运行在独立的 helper 进程里，应用在自己的可执行文件旁边查找它们。上面的 `cargo run -p nyaterm-app` **只构建应用**，因此两个协议都会以 `HelperMissing` 失败。先构建 helper：

```bash
cargo build -p nyaterm-rdp-helper -p nyaterm-vnc-helper
```

不带 `-p` 的 `cargo build` 会构建应用和两个 helper，因为它们是 workspace 的 `default-members`。`cargo check` 也会检查三者，但不会生成可供应用启动的 helper 可执行文件。若使用自定义 `CARGO_TARGET_DIR`、`--target` 或 profile，请确保 helper 与应用位于同一目录。

`NYATERM_RDP_HELPER` 和 `NYATERM_VNC_HELPER` 可以用显式路径覆盖查找结果，便于指向另一个 target 目录里的构建产物。

## 常用检查

迭代时优先运行受影响 crate 的锁定依赖检查：

```bash
cargo check -p <crate-name> --locked
cargo test -p <crate-name> --locked
```

Rust CI 在 Linux 上执行 fmt 和 clippy，并在 Linux x64、macOS arm64、Windows x64 上分别执行 workspace tests。对应的精确命令是：

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked
cargo test --workspace --locked --no-fail-fast
```

Linux 上独立的 Python 打包单测 job 执行：

```bash
python -m unittest scripts.tests.test_check_release_assets scripts.tests.test_package_native scripts.tests.test_verify_native_package scripts.tests.test_generate_release_metadata
```

RDP/VNC helper 的非 ignored lifecycle 集成测试已经由 workspace tests 自动覆盖，包括握手、正常退出和 crash/hang 回收；也可定向运行：

```bash
cargo test -p nyaterm-rdp-helper --test lifecycle --locked
cargo test -p nyaterm-vnc-helper --test lifecycle --locked
```

这些 lifecycle 测试会启动真实 helper 可执行文件，但不连接真实 RDP/VNC 服务器，因此不能替代协议互操作、帧缓冲、剪贴板和输入链路的手工验收。`cargo fmt --all` 会写回格式化结果，仅在准备应用格式变更时运行。

## Release profile 构建

```bash
cargo build -p nyaterm-app --bin nyaterm --release --locked
```

原生二进制位于 `target/release/nyaterm`，Windows 下为 `target/release/nyaterm.exe`。该命令只构建应用二进制，既不构建 helper，也不生成安装包。

发布包由 `scripts/release/package_native.py` 生成。它会以锁定依赖分别构建应用和两个 helper，把 helper 放到应用旁边，并按平台产出安装包和便携包。新增 helper 时必须同时更新该脚本的 `HELPER_BINS` 列表。

### 六个发布目标

| 平台 | Rust target | 产物 |
| --- | --- | --- |
| macOS arm64 | `aarch64-apple-darwin` | `.dmg`、`.app.tar.gz` |
| macOS x64 | `x86_64-apple-darwin` | `.dmg`、`.app.tar.gz` |
| Linux x64 | `x86_64-unknown-linux-gnu` | `.AppImage`、`.deb`、`.rpm` |
| Linux arm64 | `aarch64-unknown-linux-gnu` | `.AppImage`、`.deb`、`.rpm` |
| Windows x64 | `x86_64-pc-windows-msvc` | `_portable.zip`、`-setup.exe` |
| Windows arm64 | `aarch64-pc-windows-msvc` | `_portable.zip`、`-setup.exe` |

Release CI 的每个 matrix leg 都执行：

```bash
python scripts/release/package_native.py "${TARGET}"
python scripts/release/verify_native_package.py --target "${TARGET}" --version "${VERSION}" --dist dist
```

发布前还会对六目标合并后的资产集合执行 `scripts/ci/check_release_assets.py`，拒绝缺失或多余的产物。

`NYATERM_ARTIFACT_VERSION` 只改变产物文件名中的版本段，包内元数据仍使用 workspace 的 SemVer。该接口仅供手动快照构建使用，打包和验包必须传入同一个值：

```bash
NYATERM_ARTIFACT_VERSION=main-snapshot \
  python scripts/release/package_native.py "${TARGET}"
python scripts/release/verify_native_package.py \
  --target "${TARGET}" --version "${VERSION}" \
  --artifact-version main-snapshot --dist dist
```

正式标签在验包后发布 GitHub Release 和 R2 版本目录，再触发 Gitee、AUR 与 Homebrew。官网读取 `downloads.json`；签名的 `latest.json` 只用于让已安装的旧 Tauri 版本迁移到 GPUI。稳定版才覆盖 R2 根目录清单，预发布只保留版本化清单。手动运行 `Main Snapshot` 会覆盖 `main-snapshot` prerelease，不发布到外部分发渠道。

Release workflow 需要 `NYATERM_GITHUB_GIST_CLIENT_ID`、Gitee/R2 Variables，以及 Tauri updater、R2、Gitee、AUR、Homebrew 对应的 Secrets。相关发布步骤缺少配置时会失败，不会生成缺功能或只发布一部分渠道的正式包。

### 原生工具与手工验收边界

原生打包依赖目标平台工具：Windows 使用 NSIS，验证安装包时还需要 7-Zip；macOS 使用 `codesign` 和 `hdiutil`；Linux 使用 `appimagetool`、`dpkg-shlibdeps`、`dpkg-deb`、`rpmbuild`、`rpm`/`rpm2cpio` 等工具。因此在缺少对应工具的平台上，单独运行 Python 打包单测并不等于完成原生打包。

自动验证会检查产物集合、归档路径、应用与 helper 是否齐全、二进制架构、版本及包元数据。它不会证明 GUI 能实际启动，也不会覆盖真实安装/升级/卸载、快捷方式或 `nyaterm:` URL handler 调用、签名/notarization 与 Gatekeeper/SmartScreen 信任、真实 RDP/VNC 会话，以及 GPU、IME、PTY、剪贴板和窗口生命周期。发布候选必须在对应目标操作系统上手工验收这些行为，并如实记录实际执行的平台与结果。

## 文档站开发

编辑 `docs-site` 时另外安装 Node.js 22.13+ 和 [pnpm](https://pnpm.io/)。Docusaurus 本身只要求 Node 18+，但本仓库固定的 pnpm 版本用到了 `node:sqlite`，在更低版本上会直接崩溃。`docs-site/package.json` 的 `packageManager` 字段固定了 pnpm 版本，启用 corepack 时会自动对齐。

```bash
pnpm --dir docs-site install --frozen-lockfile
pnpm --dir docs-site start:zh
```

英文文档开发服务器：

```bash
pnpm --dir docs-site start:en
```

文档 CI 的精确检查命令是：

```bash
python3 scripts/ci/check_docs_translations.py
pnpm --dir docs-site install --frozen-lockfile
pnpm --dir docs-site build
```

构建会检查全部 locale 的页面和 sidebar，Markdown 链接问题按站点配置报告。翻译脚本校验每个页面都有中英两份、两份的标题数一致（用来发现整节漏译）、每个页面都被 `sidebars.ts` 引用，以及没有把撰稿备注留在正文里。CI 的 `Documentation site` job 会先运行该脚本，再安装锁定依赖并构建全部 locale。

注意脚本比对的是标题数量，不校验译文措辞。

## 修改第三方依赖

NyaTerm 打了补丁的第三方依赖**没有 vendor 到仓库里**。每个都是 [github.com/nyakang](https://github.com/nyakang) 下 fork 的 `nyaterm` 分支上的一条补丁序列，由根 `Cargo.toml` 固定 revision 消费：`alacritty`、`gpui-kit`、`IronRDP`、`russh`、`russh-sftp`、`sspi-rs`、`vnc-rs`、`zed`（`gpui`）和 `zmodem2`。

改动流程是：提交到 fork 分支 → 推送 → 在根 `Cargo.toml` 里 bump revision。补丁按关注点拆分而不是压成一个提交，并在提交信息和该分支的 `NYATERM.md` 里记录原因与验证方式。已有序列优先 rebase 到更新的上游 revision，而不是不断累积快照。

`temp/vendor/` 下是这些源码的只读本地副本，仅供阅读。**它不参与编译，改动那里既不会生效也不会报错。**

## 开发约定

- 先阅读根目录 `AGENTS.md` 和 `CONTRIBUTING.md`。
- UI 状态与视图放在 `nyaterm-desktop`，共享控件放在 `nyaterm-ui`。
- transport、terminal 和 core crate 保持独立于 GPUI。
- 新增 UI 文本时同步更新 `crates/nyaterm-desktop/src/i18n/locales/` 下的中英文文件。
- 不要在测试、日志或诊断数据中使用真实凭据。
