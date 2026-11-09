# Desktop 职责拆分实施计划

目标：将 `nyaterm-desktop` 中可独立验证的业务算法、原生服务和解码实现
迁到职责归属 crate，并收窄 desktop 内部状态与视图模块的边界。
desktop 继续拥有 GPUI 组合、任务生命周期、窗口与会话协调、应用级持久化适配。

## 实施阶段

- [x] 1. 文件操作：将复制、移动、目录验证、文件修改执行及行为测试迁到
  `nyaterm-files::operations`。以文件层请求/结果取代 UI 结果；desktop
  保留会话标识、剪贴板、队列、重试、进度和视图刷新。
- [x] 2. 终端分层：协议状态/编码、选择与快照搜索算法迁到
  `nyaterm-terminal`；字体测量、绘制装饰和几何逻辑迁到
  `nyaterm-terminal-gpui`。desktop 的帧处理器按队列、事件、搜索和处理循环
  拆成模块，保留线程、录制与 GPUI 协调。
  将 TRZSZ 上传/下载执行迁到 transport 的协议 runtime；desktop 保留协议
  触发、路径提示与任务展示，公共取消/背压适配不引入业务 crate 依赖。
- [x] 3. 预览解码：建立不依赖 GPUI 的 `nyaterm-preview`，接收字节或文本，
  返回像素、尺寸、PDF 元数据及结构化内容。迁移 JSON、CSV/TSV、图片、PDF、
  旋转与预算检查；desktop 保留 `RenderImage` 转换及分页缓存/加载生命周期。
- [x] 4. 翻译客户端：建立 `nyaterm-translation-client`，迁移提供方 HTTP
  请求和错误适配，添加本地 HTTP 夹具验证。desktop 保留任务、密钥状态、
  设置保存和对话框。
- [x] 5. 纯输入与导入逻辑：将临时连接解析、action-link 匹配与动作规划、
  快捷命令转义/导入解析/验证/合并迁到 core 的权威模块。I/O 与 UI 选择
  适配保持独立；保留命令原始空白、转义、未知字段处理和支持的导入格式。
- [x] 6. 工作区算法：将窗格/标签树的分割、合并、裁剪和恢复转换迁到
  core；desktop 保留指针位置、Pixels、拖拽、焦点和滚动状态。
- [x] 7. desktop 内部模块：按现有子状态拆分 session/settings 状态实现，
  沿用既有协议表单模块，进一步拆分连接编辑器的凭据、分组和选择器视图，
  将相关测试随职责迁移；通用 resize handle 视觉迁到 `nyaterm-ui`。
- [x] 8. 更新架构约束、CI crate 登记、开发文档；完成全工作区验证与审计。

## 每阶段验收

1. 权威实现只有一份；下层 crate 不依赖 desktop、store 或 GPUI（终端 GPUI
   适配除外），没有新增依赖环或跨业务 crate 依赖。
2. 保留原入口需要的窄适配，行为测试位于逻辑归属 crate；UI 状态测试继续
   在 desktop。验证失败、取消、冲突、源变化、编码/位置和兼容格式。
3. 不改变数据库表/键、设置与备份序列化、凭据加密、同步协议、已知主机或
   会话恢复格式；秘密不进入 Debug/日志。
4. 执行相关 package 测试与 `cargo check -p nyaterm-app`，记录实际结果。

## 最终验证

- `python3 scripts/ci/check_architecture.py`
- 相关 Python 架构与 CI 选择测试
- `cargo check --workspace`
- `cargo test --workspace`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets`

本机平台为 Linux。涉及其它系统的验证应明确记录可验证范围，不能以 Linux
通过声称 Windows/macOS 的原生行为已经验证。

## 实施记录

- 2026-11-09：核验当前工作树干净；重新读取实现和仓库约束，开始阶段 1。
- 已完成文件操作、预览、翻译、纯解析与工作区算法迁移。
  action-link 本地化保持分层：core 规划动作，desktop 翻译标签。
- 终端帧处理按命令队列、事件队列、搜索、worker、生命周期入口拆分；布局和
  装饰缓存及其行为测试迁到 terminal-gpui。滚动的 UI 状态适配保留在 desktop。
- 会话与设置按原有状态所有者拆分模块。核验会话原有 390 个方法和设置原有
  215 个方法均保留；未复制可变状态。连接编辑器分离凭据、分组、选择器和测试。
- Python 架构与 CI 选择测试：39 个通过。
- `cargo check -p nyaterm-desktop --all-targets` 已通过，包含迁移后的测试编译。
- desktop Rust 源码（含测试）由 389,922 行降至 380,427 行，净减少 9,495 行。
  文件拆小本身不减少职责；上述减少来自实现和测试迁到其权威 crate。
- 八阶段实施与 Linux 验收已完成；下文记录保留边界、后续候选及验证条件。

## 保留在 desktop 的边界与理由

- `NyaTermApp`、`AppShell`、窗口、焦点、输入实体、叠层和 GPUI 通知需要应用
  上下文，继续由 desktop 组合。迁到 core 会引入 UI 依赖；另设协调 crate
  则需暴露大量跨功能接口，容易造成反向依赖和重复状态。
- 会话、传输、翻译和预览的任务生命周期及事件路由需要协调 UI、服务和
  持久化。协议执行、解码和 HTTP 已下移，desktop 只保留应用协调及适配。
- 设置与会话目录的可变状态保留原有单一所有者；数据库执行和兼容读取仍
  在 store，格式与纯模型仍在 core。本轮没有改动持久化字段或加密协议。
- 通用控件放在 ui；依赖具体功能状态的连接表单、设置面板和远程监控视图
  继续放在 desktop，避免 ui 反向依赖业务。较大的视图和事件处理文件仍可
  在 crate 内按领域继续拆分，不以文件行数作为新增 crate 的唯一依据。

## 后续可继续拆分的内部模块

- `features/pages/settings/panel.rs`（3,082 行）：按设置类别拆视图，复用
  当前 settings 状态所有者与输入实体。
- `features/pages/remote/panels.rs`（2,670 行）：按系统指标、进程及容器等
  面板拆分 GPUI 构造，服务执行继续归 remote-ops。
- `features/terminal/terminal_surface_entity/mod.rs`（3,495 行）和
  `terminal_runtime/buffer/mod.rs`（3,318 行）：可进一步分离事件适配、
  输入和缓冲生命周期，仍需保留实体与工作线程的单一协调入口。
- 大型行为测试文件可按对应领域随实现一起拆分；它们的行数不能直接
  说明生产模块承担了更多业务职责。

## 验证记录

- Linux：架构检查通过；Python 架构与 CI 选择测试 39 个通过。
- `cargo check -p nyaterm-desktop --all-targets`、格式检查和 diff 空白检查通过。
- `cargo check --workspace`、`cargo check -p nyaterm-app` 和
  `cargo clippy --workspace --all-targets` 均通过；最终 Clippy 无警告。
- 最终测试中 desktop 使用 1 个测试线程，其余 crate 使用 8 个线程。
  desktop 的同步侧栏测试与其它 UI 测试共享进程级同步锁；8 线程运行时
  曾超出原有 60 秒等待期限，单独运行 6.91 秒通过。串行执行保留其完整
  行为断言，同时避免跨测试锁等待影响本轮验收。
  desktop 主测试进程通过 `--test-threads=1` 串行执行，环境变量仍为
  `RUST_TEST_THREADS=8`，避免把串行设置传给使用本测试二进制模拟服务的
  子进程夹具；相关 AI 夹具分组 6 个测试在该设置下通过。
  工具进程以 root 运行，会绕过只读文件权限；因此测试二进制使用 UID 1
  执行器、重置用户环境、UTF-8 locale 和独立可写缓存目录。这同时保留了
  文件权限、真实 PTY 中文输入及 helper 剪贴板的行为断言。
- 修正 environment 测试夹具的临时脚本启动竞争：仅对 `ETXTBSY` 按
  10 ms 间隔重试，最长 1 秒。该错误表示脚本尚未执行，重试不会重复
  副作用；生产代码和测试断言均未改动。相关 environment 测试 33 个通过。
- 最终完整套件通过，使用
  `env CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUNNER=/tmp/nyaterm-split-test-runner.sh cargo test --workspace --no-fail-fast`。
  此本机执行器仅为 desktop 测试二进制添加 `--test-threads=1`，再执行
  `setpriv --reset-env --reuid=1 --regid=1 --clear-groups env SHELL=/bin/bash LANG=C.UTF-8 LC_ALL=C.UTF-8 XDG_CACHE_HOME=/tmp/nyaterm-split-cache-2k7b7pmx RUST_TEST_THREADS=8 <测试二进制及参数>`。
  73 个测试目标成功结束，合计 5,953 个测试通过、0 个失败、37 个原有
  忽略项；包含单元测试、集成测试和文档测试。desktop 为 2,715 个通过、
  15 个原有忽略项。完整命令退出码为 0。
  本机完整日志：`/tmp/nyaterm-split-workspace-tests-isolated-ui.log`。
- Windows/macOS 原生运行行为尚未在对应系统上验证。
