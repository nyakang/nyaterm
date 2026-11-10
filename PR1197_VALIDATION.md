# PR #1197 修复与验证报告

## 交付基线

- 分支：`fix/pr1197-host-tools`
- worktree：`/data/kang/nyaterm-pr1197`
- 基线：`e8bf8d412c04dba70009d3c2e2a0af625d69397b`
- 原工作区：`/data/kang/nyaterm-gpui`，保持原状；构建缓存复用其 `target`。
- 平台：Linux x86_64，Rust 1.99.1。GPUI 测试仅在命令级设置 `RUST_MIN_STACK=16777216`。

## 已实施的修复

1. **平台能力**：remote-ops 增加 `ProcessControlCapabilities` 和类型化 `UnsupportedProcessControl`。Windows 本地仅接受显式 KILL；TERM/HUP/STOP/CONT/renice 在执行前拒绝，保留信号归一化入口。Processes 快照及比较条件包含能力，菜单与优先级行按能力渲染；Ports 遵守相同限制，KILL 保留确认。
2. **执行期限**：`HostTarget` 保留公开接口，本地监督器移入私有 `host/local_exec`。Unix 使用独立进程组与非阻塞三路 I/O；Windows 使用挂起创建、私有 Job Object、恢复线程、PeekNamedPipe 和可取消 stdin 写入线程。主进程退出后仍监督继承管道；执行、输入与输出共享期限。每条输出流限制 16 MiB，超限及超时返回类型化 `HostCommandError`，不返回截断成功。输入缓冲使用 `Zeroizing`，错误不带命令、密码或捕获输出。
3. **任务状态**：主机 epoch 与请求 ID 分开，刷新、操作、日志及详情独立槽位。刷新单次执行，重复手动请求合并；操作期间不启动普通刷新，成功后请求刷新。通过统一结果入口收尾成功、失败、拒绝、取消与 panic；过期回调不能释放新槽位。`JobCancellationHandle` 可克隆，现有 JobTask Drop 行为不变。
4. **主机隔离**：会话变化和具体 SSH 租约变化使旧任务失效；租约使用 Arc 身份比较。日志与详情同时校验 epoch、请求 ID 和条目 key。Services 使用 `(scope, name)` 身份。Ports/Services 的操作、确认及提权重试绑定 epoch 和目标；Processes 的操作/确认也携带会话、epoch 和租约身份。
5. **密码生命周期**：切换主机、隐藏面板、取消提权和关闭授权时调用 `discard_content()`，包括编辑历史。提交先复制到 `SecretString`，立即清空输入。认证失败清除缓存；操作期间禁止切换授权。首次加载失败、空列表仍显示提权入口；隐藏后收尾的失败不会重建授权。
6. **Windows 能力提示**：本地 Docker/GPU/NPU 显示明确未支持提示，轮询与手动入口不提交必然失败的后端请求；保留 SSH 后端。新增六种语言文案；强制结束确认框明确说明强制终止及未保存数据可能丢失。

未改变活动栏 ID、设置/数据库/凭据/备份/同步格式；没有依赖 fork revision 变更，remote-ops 未引入 GPUI 或 desktop 依赖。

## 基线复现

已有 remote-ops 测试 72 项通过，四个复现用例失败：

| 基线复现 | 正式回归覆盖 |
| --- | --- |
| 跨主机保留未提交 sudo 草稿 | `switching_hosts_discards_password_draft_and_cached_authorization` |
| 操作期间手动刷新留下 busy | `refreshing_during_an_action_cannot_orphan_its_busy_state` |
| 慢速轮询重叠 | `slow_refresh_is_single_flight_and_manual_requests_are_coalesced` |
| 父进程退出、后代持有管道导致期限失效 | `inherited_pipes_remain_subject_to_deadline_after_parent_exits` |

复现源与原始日志保存在 `/tmp/nyaterm-pr-1197-review/`。Windows CI 的既有堆栈溢出在 PR 基线也出现：[基线 CI](https://github.com/nyakang/nyaterm/actions/runs/42540477495/job/128914113819)。

## 验证状态

| 检查 | 结果 | 本机日志 |
| --- | --- | --- |
| `cargo test -p nyaterm-remote-ops --locked` | 80 项通过：71 单元测试、2 本地冒烟、5 既有构造器/解析/信号兼容测试、2 SSH 兼容测试 | `/tmp/pr1197-remote-ops-verified.log` |
| desktop `host_tools::` 回归 | 最终代码 29 项通过，包含真实确认框回调和操作期间刷新 | `/tmp/pr1197-final-host-tools.log` |
| Processes 菜单与优先级控件 | 最终代码 1 项通过，覆盖 Windows/Posix 两种快照能力 | `/tmp/pr1197-final-process-menu.log` |
| 六种语言及占位符检查 | 最终代码 17 项通过 | `/tmp/pr1197-final-locales.log` |
| `cargo check --workspace --locked` | 通过 | `/tmp/pr1197-final-check.log` |
| `cargo test --workspace --locked` | 失败：唯一失败是 root 身份下的既有只读文件权限测试；桌面 2752 项通过、15 项既有忽略 | `/tmp/pr1197-workspace-test-confirmed.log` |
| `cargo test --workspace --locked --no-fail-fast` | 完整执行全部测试及文档测试：6028 通过、1 失败、37 既有忽略；失败仍为同一权限用例 | `/tmp/pr1197-workspace-all-tests.log` |
| `cargo fmt --all -- --check` | 通过 | `/tmp/pr1197-delivery-format.log` |
| `cargo clippy --workspace --all-targets --locked` | 通过 | `/tmp/pr1197-final-lint.log` |
| `python3 scripts/ci/check_architecture.py` | 通过 | `/tmp/pr1197-delivery-architecture.log` |

全量测试启动后新增了确认框回归及强制终止文案，最终版本随后按 `host_tools::`、Processes 菜单和 i18n 分别复测通过；上述全量统计保留实际运行数字，未将针对性复测重复计入。

唯一全量失败为 `nyaterm-files::download_path::tests::staged_write_preserves_existing_permissions`：该测试要求向只读文件写入失败，但本机以 root 运行，可以绕过权限限制。使用相同的 `RUST_MIN_STACK=16777216 cargo test --workspace --locked` 在 PR 基线 worktree `/tmp/nyaterm-review-1197` 也复现同一失败，日志为 `/tmp/pr1197-baseline-workspace-test.log`。对修复分支产出的同一 files 测试二进制，以普通用户 `nobody` 执行该用例通过，日志为 `/tmp/pr1197-readonly-unprivileged.log`。本次未修改 files 实现或其测试。

修复源代码分为两个提交，便于分别审查：

- `8fd460238`：本地执行期限、进程树回收、输出限制和平台能力。
- `3987e21b6`：面板任务隔离、主机/交互绑定、密码清理、菜单和文案，以及桌面回归。

## 回归测试位置与覆盖

| 测试位置 | 覆盖行为 |
| --- | --- |
| `crates/nyaterm-remote-ops/src/process_control.rs` | POSIX 信号白名单、Windows 只接受 KILL、优先级能力 |
| `crates/nyaterm-remote-ops/src/host/local_exec/mod.rs` | 父进程退出后继承管道仍超时、进程组回收、堵塞 stdin、连续双路输出、大输入与非零退出、两路独立上限、回收后继续执行 |
| `crates/nyaterm-remote-ops/src/host/local_exec/windows_tests.rs` | Windows 受控父子进程、Job 回收、继承管道、大 stdin 与双路输出、两路上限、不支持信号和 renice 不影响活进程、显式 KILL |
| `crates/nyaterm-remote-ops/tests/ssh_compatibility.rs` | Ports、Services、日志、详情和控制操作遵守 SSH 兼容模式限制 |
| `crates/nyaterm-desktop/src/features/pages/remote/host_tools/jobs.rs` | epoch 与请求 ID 独立、同主机同条目旧请求不能释放替代请求、scope 匹配 |
| `crates/nyaterm-desktop/src/features/pages/remote/host_tools/tests.rs` | 密码草稿及缓存清理、单次刷新与合并、操作期间刷新、过期日志和详情、队列满、取消、panic、失败后恢复、同名服务不同 scope、菜单能力、过期操作与提权重试 |
| `crates/nyaterm-desktop/src/features/pages/remote/panels.rs` | Processes 菜单及优先级控件按快照能力显示、面板快照与布局 |
| `crates/nyaterm-desktop/src/features/remote/remote_runtime/panel_layout.rs` | 现有本地 Ports/Services 冒烟和面板需求测试 |
| `crates/nyaterm-desktop/src/blocking_jobs.rs` | 可克隆取消句柄阻止排队执行，同时仍可等待原 future 的取消结果 |
| `crates/nyaterm-desktop/src/app_shell/session_hub/ssh_connections.rs` | 克隆租约保持相同连接身份、替换租约拥有不同 Arc 身份 |

`confirming_an_old_host_dialog_cannot_submit_ports_or_process_operations` 实际挂载确认框、切换主机并点击确认按钮，验证 Ports 和 Processes 旧回调不能提交操作；同时验证未切换主机的 Ports 确认按钮可以提交受控操作。测试不对真实 PID 发信号。

GPUI 状态回归通过可注入执行器和受控 gate 驱动，不依赖 systemd、特定服务或真实 SSH 主机。已有本地资源冒烟测试与这些确定性回归分别保留。

## 复测命令

在修复 worktree 中执行；本机使用 `CARGO_TARGET_DIR=/data/kang/nyaterm-gpui/target` 复用未跟踪构建缓存：

```sh
cargo test -p nyaterm-remote-ops --locked
RUST_MIN_STACK=16777216 cargo test -p nyaterm-desktop --locked host_tools:: -- --test-threads=1
RUST_MIN_STACK=16777216 cargo test -p nyaterm-desktop --locked process_menu_and_priority_controls_follow_the_captured_capabilities
cargo check --workspace --locked
RUST_MIN_STACK=16777216 cargo test --workspace --locked
RUST_MIN_STACK=16777216 cargo test --workspace --locked --no-fail-fast
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked
python3 scripts/ci/check_architecture.py
```

Windows runner 上运行 `cargo test -p nyaterm-remote-ops --locked` 会自动包含 `windows_tests`；桌面测试同样仅在命令级设置 `RUST_MIN_STACK`。macOS runner 上运行 remote-ops 与桌面相关用例，并验证 launchd 不同 domain 的同名条目和原生交互。

## 平台验证边界

- Linux：本地执行器、面板状态、GPUI 菜单和 workspace 检查在本机运行。
- Windows：交付受控子进程测试，覆盖不支持信号拒绝后子进程仍存活、显式 KILL、Job 进程树回收、继承管道、堵塞 stdin、双路大量输出、非零退出与两路输出上限。完整 remote-ops Windows GNU 交叉检查在原生 C 依赖阶段因缺少 `x86_64-w64-mingw32-gcc` 中断；本地监督器源码的独立 Windows 编译检查通过。该检查仅验证编译，**不是 Windows 行为验证**；Windows 实机/runner 测试待执行。
- macOS：共享 Unix 执行器和 launchd scope 身份规则经代码检查；macOS 运行与原生交互验证待 runner/实机执行。
- 已运行的读取使用 SSH 原有期限或修复后的本地期限结束；取消句柄用于阻止排队读取与丢弃过期结果，不承诺即时终止已运行的 SSH 调用。已开始的控制操作正常收尾。
