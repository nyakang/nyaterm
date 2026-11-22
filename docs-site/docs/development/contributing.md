# 贡献指南

感谢你有兴趣为 NyaTerm 做出贡献。

## 开始之前

1. 阅读仓库根目录的 `AGENTS.md` 和 `CONTRIBUTING.md`。
2. 按照 [开发环境搭建](./setup) 配置 Rust 和平台依赖。
3. 查看 [Issues](https://github.com/nyakang/nyaterm/issues)，确认问题和预期行为。

## 选择正确的 crate

- 纯模型、解析、兼容格式和策略放在 `nyaterm-core`。
- 数据库执行和兼容性读取放在 `nyaterm-store`。
- PTY、SSH、SFTP、Telnet、串口、隧道和传输运行时放在 `nyaterm-transport`。
- 终端状态机与快照放在 `nyaterm-terminal`；GPUI 终端绘制放在 `nyaterm-terminal-gpui`。
- GPUI 状态、视图和后台任务协调放在 `nyaterm-desktop`。
- 共享 GPUI 控件和主题集成放在 `nyaterm-ui`。
- RDP/VNC 会话管理、输入模型和 IPC 合约放在 `nyaterm-remote-desktop`；协议解码器只放在 `nyaterm-rdp-helper` 和 `nyaterm-vnc-helper`。

跨 crate 修改应保持 adapter 小而明确，并确保每份状态只有一个权威 owner。

## 连接编辑一致性

连接编辑器与资产批量编辑器必须保持同一套字段表面。新增、删除或重命名连接设置时：

- 在连接编辑器所属分区中加入该字段；
- 为批量编辑器提供同名、同分区、同“新建连接默认值”的行；
- 保持 `BATCH_FIELDS_BY_SECTION` 完备；某个批量字段无法在任何协议分区中渲染，就是一致性缺陷；
- 运行 `nyaterm-desktop` 中的 `editor_surface_parity` 测试，它会校验字段覆盖、分区可达性，以及“恢复默认”写入的值与新建连接一致。

各协议的编辑分区只组合共享的 `editor_surface` 原语，不要自绘控件外壳；也不要为了塞进字段而把某套控件布局复制到批量分区。

## 贡献流程

1. Fork 仓库并从 `main` 创建分支。
2. 在正确的 crate 中实现修改并添加相邻测试。
3. 运行受影响 crate 的检查，再运行相关 workspace 检查。
4. 使用 Conventional Commit 风格提交。
5. 推送分支并创建 Pull Request。

```bash
git checkout -b feat/my-feature
cargo check -p <crate-name>
cargo test -p <crate-name>
```

## 提交规范

提交主题使用：

```text
<type>(<scope>): <imperative summary>
```

示例：

```text
feat(terminal): add search result navigation
fix(transport): handle closed SSH channels
docs: update development setup
```

常用类型包括 `feat`、`fix`、`docs`、`refactor`、`perf`、`test` 和 `chore`。常用 scope 包括 `terminal`、`transport`、`desktop`、`storage`、`ui`、`ai` 和 `sync`。

## 代码与测试

```bash
cargo check --workspace
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets
```

- 使用 Rust 2024 idiom、标准 rustfmt 和显式 import。
- 不要增加 `#[path = "..."]`、`use super::*` 或共享 feature prelude。
- render 路径中不执行数据库、文件系统、网络或其他阻塞操作。
- 存储、凭据、加密、备份和同步修改需要新数据 round trip 与代表性旧数据测试。
- 平台相关窗口、PTY、串口、剪贴板和输入行为需在目标系统验证。

## 国际化与文档

新增或修改应用 UI 文本时同步更新：

- `crates/nyaterm-i18n/locales/zh-CN.json`
- `crates/nyaterm-i18n/locales/en.json`

修改 docs-site 时同步维护 `docs-site/docs/` 中文源文档和 `docs-site/i18n/en/docusaurus-plugin-content-docs/current/` 英文页面，新增页面还要加入 `docs-site/sidebars.ts`，并运行：

```bash
python3 scripts/ci/check_docs_translations.py
pnpm --dir docs-site build
```

## 修改第三方依赖

打了补丁的第三方依赖不在仓库里，而是 [github.com/nyakang](https://github.com/nyakang) 下 fork 的 `nyaterm` 分支上的补丁序列。改动流程见 [开发环境搭建 → 修改第三方依赖](./setup#修改第三方依赖)。

要点：提交到 fork 分支并推送，再在根 `Cargo.toml` 里 bump revision；补丁按关注点拆分；在提交信息和该分支的 `NYATERM.md` 里记录原因和验证方式。PR 描述中需注明改动的 fork 分支和 revision。

**不要改 `temp/`。** 那里是只读副本，不参与编译，改动既不生效也不报错。

## 安全与兼容性

不要提交或记录密码、私钥、OTP、API secret 或未脱敏的终端上下文。持久化修改必须保留现有 table、key、字段名、加密前缀、备份格式和 fallback 行为，除非同时提供经过测试的迁移。

## 许可证

贡献代码遵循项目的 [Apache License 2.0](https://github.com/nyakang/nyaterm/blob/main/LICENSE)。
