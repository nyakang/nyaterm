---
name: nyaterm-verify
description: Verify and ship a change to NyaTerm (nyakang/nyaterm) — pick the owning crate, run the focused and CI-equivalent Rust checks, reproduce terminal snapshot/rendering bugs, and land the branch via fork + pull request. Use whenever working in a NyaTerm checkout or investigating its terminal/desktop behavior.
---

# NyaTerm 改动验证手册

NyaTerm 是 Rust + GPUI 的 workspace（`crates/` 下 ~35 个 crate），没有 Web 后端。
本技能解决三件事：改对地方、验证到位、按仓库流程提交。

## 1. 先选对 crate（改错地方 = 白改）

| 改动性质 | 归属 |
| --- | --- |
| 纯模型 / 解析 / 兼容格式 / 策略 | `nyaterm-core` |
| PTY / SSH / SFTP / Telnet / 串口 / 隧道运行时 | `nyaterm-transport` |
| redb、事务、兼容读取 | `nyaterm-store` |
| 终端状态机、快照、控制序列、编码、graphics | `nyaterm-terminal`（不依赖 GPUI） |
| 终端像素布局、输入映射、选区、绘制 | `nyaterm-terminal-gpui` |
| GPUI 状态 / 视图 / 后台任务协调 | `nyaterm-desktop` |
| 共享控件、主题 | `nyaterm-ui` |
| 帧队列、事件泵、worker、录制顺序 | `nyaterm-terminal-runtime` |

终端快照的所有权：`nyaterm-terminal` 生成 UI-neutral snapshot；desktop 只持有呈现缓存
（`scrollback_snapshots`、`retained_rows`、`Surface.snapshot`）。改缓存/新鲜度判定前先读
`crates/nyaterm-terminal-runtime/README.md` 和根目录 `AGENTS.md` 的终端段落。

## 2. 迭代期检查（必做）

```bash
cargo fmt --all                                    # rustfmt 未安装时: rustup component add rustfmt
cargo check -p <crate> --locked
cargo test  -p <crate> --locked
cargo clippy -p <crate> -p <下一个受影响 crate> --lib --locked   # 补装: rustup component add clippy
```

测试放“行为旁边”：`nyaterm-terminal` 的解析/快照测试在 `crates/nyaterm-terminal/src/tests.rs`；
desktop 的缓存/状态测试在对应模块的 `tests.rs`（例如
`features/terminal/terminal_runtime/view_io/tests.rs`）。不要在错误 crate 里造集成测试。

耗时参考（4 vCPU）：`nyaterm-terminal` lib 测试约 10s；`nyaterm-desktop` **首次** test profile
编译约 5–6 分钟（之后跑单测秒级）；`cargo check -p nyaterm-desktop --lib` 首次 10+ 分钟。

## 3. 提 PR 前跑 CI 同款

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked
cargo test --workspace --locked --no-fail-fast      # --no-fail-fast 必须带
```

CI（GitHub Actions）在 Linux 跑 fmt/clippy，Linux x64 / macOS arm64 / Windows x64 各跑一遍
workspace tests；`Lint`、`Test (linux-x64|macos-arm64|windows-x64)`、`Docs`、tmux 矩阵是主要 job。
纯 Rust 改动不会跑文档站构建。

已知既有噪音：`features::session::session_runtime::serial_prompt_tests::*` 在 CI Linux 上偶发
失败，与终端快照改动无关；重跑确认即可，不要顺手改。

## 4. 没本地环境 / 网络慢：用 instabox 盒子

```bash
instabox get mine --json      # 剩余时间 = maxRunMinutes - (now - runStartedAt)
instabox exec --background <box> --cwd /data/workspace/nyaterm -- 'cargo test ... > /tmp/x.log 2>&1'
instabox jobs ls <box>; instabox jobs logs <box> <job> -f
```

坑（都踩过）：

- 前台 `instabox exec` 默认 60s 超时；编译/测试一律 `--background` 写日志再轮询。
- Linux 桌面依赖（GPUI 编译必需）：`sudo apt install libglib2.0-dev libgtk-3-dev
  libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libfontconfig1-dev libssl-dev
  libasound2-dev libvulkan-dev libxcb1-dev libxcb-render0-dev libxcb-shape0-dev
  libxcb-xfixes0-dev libzstd-dev libpulse-dev libudev-dev`。
- 盒子镜像常缺 `rustfmt`/`clippy`：`rustup component add rustfmt clippy --toolchain <channel>`。
  工具链版本见 `rust-toolchain.toml`（当前 1.99.1）。
- `target/` 会涨到 10GB+，磁盘报警时删 `target/`（可重建），不要动 `/data/workspace` 里的源码。
- 传文件用 base64 过 `instabox exec`，或 `instabox pull <box> <remote> <local>`。

## 5. 复现终端快照/渲染类 bug

先在 `nyaterm-terminal` 里写最小用例，而不是猜 UI：

```rust
let mut screen = TerminalScreen::new(20, 6);
screen.advance(b"old\r\nold\r\n\x1b[2J\x1b[Hroot# ");   // 清屏会归档旧屏到 scrollback
let cached = screen.viewport_snapshot(0);                // 快照
screen.advance(b"ls 2\r\n");                             // 原地重绘，history 坐标不变
let live = screen.viewport_snapshot(0);
assert!(!cached.content_agrees_with(&live));             // 内容版本判定
```

关键事实：`ED`/清屏不会清掉 history 坐标，`scroll_count`、`scrollback_len`、`total_rows`
可以完全不变，而行内容已经变了。任何“这份快照还新鲜吗”的判断都必须比较**共享行**的
`revision` + `signature`（见 `TerminalSnapshot::content_agrees_with`），只看
`preserves_coordinates_from` / `covers_display_offset` 会把旧快照当新快照，表现为：
清屏后旧提示符重新出现在新提示符上方、上翻时最新输出缺失。只比较两个快照都覆盖的行；
用“必须覆盖当前视口”当条件会误杀合法的上翻视图快照。

命中测试/选区路径会按绝对行号取快照，所以选中动作容易暴露这类缓存 bug。

## 6. 提交与 PR（fork → 分支 → PR，不要直推上游）

```bash
git checkout -b fix/<topic>            # 或 feat/、docs/
cargo fmt --all -- --check
git commit -m "fix(terminal): <imperative summary>"     # Conventional Commits
git fetch origin main && git rebase origin/main
git push fork HEAD:refs/heads/fix/<topic>               # fork = <you>/nyaterm
```

- PR 目标 `nyakang/nyaterm:main`；body 写“问题 / 根因 / 修改 / 验证 / 兼容性”，验证里如实列出
  没跑的部分（workspace 全量、Windows GUI 手工验收）。
- 无写权限的 token 直推上游会 403；正确路径是 fork 分支 + PR。
- 用户可见修复要在 `CHANGELOG.md` 的 `[Unreleased] → Fixed` 加一条；UI 文案改动同步
  `crates/nyaterm-i18n/locales/{zh-CN,en}.json`；architecture/setup 等文档改动需中英两份。
- 看 CI：`GET /repos/nyakang/nyaterm/actions/runs?event=pull_request`，再取 `runs/<id>/jobs`；
  失败日志 `GET /repos/nyakang/nyaterm/actions/jobs/<job>/logs`。

## 7. 交付前自检清单

- [ ] 改动落在正确 crate，没有把 GPUI 依赖带进 core/terminal/transport。
- [ ] 有相邻测试，且测试名描述行为。
- [ ] `fmt` + 受影响 crate 的 `check/test/clippy` 通过。
- [ ] 说明哪些检查没跑、为什么（CI 平台 / GUI 手工验收）。
- [ ] 用户可见变更进 CHANGELOG / i18n；提交信息符合 Conventional Commits。
- [ ] 分支已 rebase 到最新 `main`，PR 描述含验证矩阵。
