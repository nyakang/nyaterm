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

## 第二轮复审修复

本轮基线为已推送的 `790caa55a`，继续使用同一独立 worktree。本轮不修改数据格式、依赖 revision 或无关 CI 问题。

1. **Linux 信号命令**：Processes 改用 dash 接受的 `kill -SIGNAL PID`；拒绝 PID 0，保留信号归一化和 Windows 能力检查。Processes/Ports 测试只控制自己创建的进程，验证 HUP、STOP、CONT、TERM 和 KILL。
2. **后台服务生命周期**：修正第一轮执行器引入的回归。父进程、stdin 和两条输出流全部结束后解除清理，包括正常返回的非零退出码；Unix 的 Drop 不再杀组，Windows 清除 Job 的 `KILL_ON_JOB_CLOSE` 后解除清理。清除失败仍返回错误并回收进程树。私有 Supervisor 在异常和 panic 时先回收进程树，再取消/回收输入线程；原有单一期限和输出上限保持。
3. **真实读取失败**：详情与日志检查退出状态，失败返回首条有效 stderr 或固定提示，不返回 stdout 中的命令行或日志正文。systemd/launchd 保留 stderr；通用日志用 POSIX awk 流式保留末尾行，区分无匹配、文件缺失/不可读和读取错误。Windows 仅将 `NoMatchingEventsFound` 视为空结果，其余事件查询错误返回失败。真实 NativeExecutor 经实际任务队列触发面板错误收尾、密码缓存/草稿清理与提权入口。
4. **macOS 用户域**：有效 UID 为 0 时使用通过纯数字校验的 `SUDO_UID`，其他情形使用 `id -u`；不再把提权前的 `gui/501` 切换成 `gui/0`。本地与 SSH 使用同一脚本，服务 key 保持 scope/name 身份。

新增回归先在本轮基线上确认了三个失败：后台工作被杀、dash 信号命令拒绝、认证失败返回成功空详情；launchd 脚本夹具也确认原代码在 `uid=0/SUDO_UID=501` 时错误列出 gui/0。日志分别为 `/tmp/pr1197-round2-baseline-background.log`、`/tmp/pr1197-round2-baseline-signals.log`、`/tmp/pr1197-round2-baseline-reads.log`、`/tmp/pr1197-round2-baseline-launchd.log`。

| 第二轮检查 | 结果 | 本机日志 |
| --- | --- | --- |
| remote-ops 全部测试 | 最终复测 90 项通过：76 单元、3 原生适配器/服务启动、4 本地、5 迁移兼容、2 SSH 兼容 | `/tmp/pr1197-round2-remote-ops-final.log` |
| desktop `host_tools::` | 31 项通过；真实 NativeExecutor 子进程夹具覆盖五种结果，经真实队列完成日志/详情 | `/tmp/pr1197-round2-host-tools.log` |
| `cargo check --workspace --locked` | 通过 | `/tmp/pr1197-round2-check.log` |
| `cargo test --workspace --locked --no-fail-fast` | 6041 通过、1 失败、37 忽略；唯一失败为 root 下既有 files 只读权限用例。desktop 2755 通过、15 忽略 | `/tmp/pr1197-round2-workspace-tests.log` |
| `cargo fmt --all -- --check` | 通过 | `/tmp/pr1197-round2-format.log` |
| `cargo clippy --workspace --all-targets --locked` | 通过 | `/tmp/pr1197-round2-clippy.log` |
| 架构检查 | 通过 | `/tmp/pr1197-round2-architecture.log` |
| 独立 Windows 监督器编译检查 | 通过；临时适配输出契约并编译最新生产监督器，未运行 Windows 测试 | `/tmp/pr1197-round2-windows-compile.log` |
| 独立 Windows 监督器测试源码编译 | 通过；生产监督器及生命周期测试源码使用临时输出/外部适配器桩交叉检查，不代表完整 remote-ops 编译或运行 | `/tmp/pr1197-round2-windows-test-compile.log` |
| 普通用户日志权限回归 | 同一 remote-ops 测试二进制以 nobody 运行，实际覆盖 mode 000 文件不可读及提权识别，1 项通过 | `/tmp/pr1197-round2-log-permissions-unprivileged.log` |
| 普通用户 files 只读权限回归 | 全量产出的同一 files 二进制以 nobody 运行，1 项通过 | `/tmp/pr1197-round2-files-unprivileged.log` |

全量测试期间进一步加强了后台测试的显式同步和日志不可读诊断；最终 remote-ops 全部测试、工作区 check/clippy、格式及架构检查随后复验通过。全量统计保留实际运行数字，定向复测不重复计入。权限测试在 root 下无法验证 mode bits，已另以普通用户运行同一回归补齐。

新增测试位于 `services/regression_tests.rs`（launchd 脚本、流式日志）、`services/windows_log_tests.rs`（事件错误分类）、`tests/host_tools_regressions.rs`（原生错误传播和 SysV 服务启动）、既有 Unix/Windows 监督器测试和 desktop host_tools 测试模块。PATH 仅设置在专属测试子进程上，不修改并发测试进程的全局环境；不操作真实服务。

本轮代码提交：`31c2b6131`（正常执行保留后台服务及异常清理顺序）、`5cb32f0b1`（信号、读取失败、launchd 用户域和真实适配器回归）。推送目标仍为原 PR 的 `WerVa/nyaterm:feat/host-tools-upstream`，不强制覆盖源分支。

## 第三轮：Ports／Services 默认隐藏与按需采集

从已推送的 `c3336f331` 继续，使用原 worktree 和分支。没有新增设置字段、独立开关、数据库转换或依赖 revision。

- core/store 的默认右上列表移除 Ports／Services。桌面纯布局方法 `ensure_host_tool_entries()` 为自定义菜单补齐隐藏槽位：Ports 在 Docker 后、Services 在 Ports 后；锚点不在右上时追加。已有条目视为用户配置，保留位置、排序和显示状态；旧 `ports`／`services` 别名归一化并保留隐藏状态。
- 默认构造、初始化和通用缺失 ID 恢复前补齐槽位；初始化同步既有设置摘要的四个区和隐藏集合。全量回归发现并修正设置草稿的兼容问题：冲突校验双方使用与启动相同的纯布局补齐，避免将缺失的隐藏槽位误判为外部改动；新增测试确认真正的跨窗口显示／位置变化仍阻止覆盖。保存流程与序列化字段不变，未知 ID 和未知配置字段继续保留。
- 显示入口只改变布局。刷新、日志、详情与时钟都检查当前 demand；停靠、浮动、多面板、折叠、隐藏、移动、模式切换和移动端抽屉完成后立即同步 `panel_is_rendered()` 对应的需求。关闭时撤销轮询、取消排队读取，已运行读取按期限结束，已开始操作正常收尾。面板注册、导航和后端接口继续可用。
- 配置回归覆盖新安装、缺字段、旧默认、空布局、别名、重复规范化、保存／重新打开数据库和应用、移动／显示／隐藏，以及恢复默认后重新隐藏。菜单回归确认默认 unchecked 且可启用。受控执行器验证未开面板时会话切换／显示入口／推进时钟产生零读取，打开后采集，关闭及各种布局变更立即撤销需求，排队取消不会调用执行器。

检查上一轮 Windows runner 时发现两个相关失败，另行修正：PowerShell 的 `-Command -` stdin 模式可能不执行 EOF 前的多行块，改为将完整脚本作为一个参数传入，新增无尾部空行、非零退出和 Unicode 回归；Windows 后台生命周期夹具显式取消测试程序原始 stdout/stderr 句柄的继承，避免仅重定向子进程 stdio 时仍携带原管道。生产 Job 正常释放及异常清理策略未改变。这两项修正仍需新 Windows runner 验证。

| 第三轮检查 | 结果 | 日志 |
| --- | --- | --- |
| core/store 全部测试 | 1069 通过、2 忽略 | `/tmp/pr1197-opt-in-core-store.log` |
| desktop 相关布局／菜单／host-tools／设置草稿回归 | 99 通过（含 38 项设置草稿测试） | `/tmp/pr1197-opt-in-focused-desktop-final.log` |
| desktop workspace 单元测试 | 2765 通过、15 忽略 | `/tmp/pr1197-opt-in-workspace-tests.log` |
| remote-ops 全部测试 | 90 通过 | `/tmp/pr1197-opt-in-remote-ops.log` |
| `cargo check --workspace --locked` | 通过 | `/tmp/pr1197-opt-in-check.log` |
| `cargo test --workspace --locked --no-fail-fast` | 6053 通过、2 失败、37 忽略；失败为既有 files root 权限用例和 transport shell 环境超时；desktop 全部通过 | `/tmp/pr1197-opt-in-workspace-tests.log` |
| `cargo fmt --all -- --check` | 通过 | `/tmp/pr1197-opt-in-format.log` |
| `cargo clippy --workspace --all-targets --locked` | 通过 | `/tmp/pr1197-opt-in-clippy.log` |
| `python3 scripts/ci/check_architecture.py` | 通过 | `/tmp/pr1197-opt-in-architecture.log` |
| transport 包级复测 | 471 通过、4 项实际 shell 初始化超时、9 忽略；源码未修改 | `/tmp/pr1197-opt-in-transport-retest.log` |
| `c3336f331` transport 同命令基线对照 | 474 通过、同一 missing-variable 用例超时、9 忽略；临时独立 worktree 执行 `cargo test -p nyaterm-transport --locked --lib` | `/tmp/pr1197-opt-in-transport-baseline.log` |
| transport 超时用例单独复测 | 1 通过（8.95 秒，接近原 10 秒期限） | `/tmp/pr1197-opt-in-transport-timeout-probe.log` |
| 普通用户 files 权限回归 | 同一测试二进制以 nobody 运行，1 项通过 | `/tmp/pr1197-opt-in-files-unprivileged.log` |
| Windows 监督器夹具交叉编译 | 通过；独立临时契约桩检查，不代表 Windows 行为验证 | `/tmp/pr1197-opt-in-windows-compile.log` |

本轮代码提交：`45fc2b1ae`（Windows 脚本执行及平台回归夹具）、`e462bcf14`（默认隐藏、布局兼容和按需采集）、`334b3a366`（设置草稿使用相同布局兼容转换）。推送继续使用原 PR 源分支，推送前核对 head 并保持正常 fast-forward。

新 head 的平台 CI 结果待 runner 回报；实时状态见 [PR #1197 检查页](https://github.com/nyakang/nyaterm/pull/1197/checks)。上轮已经执行的 runner 结果和无关排除项分别记录在下方，不能用 Linux 或交叉编译结果替代 Windows/macOS 行为验证。

GPUI 使用命令级 `RUST_MIN_STACK=16777216`；构建继续复用 `/data/kang/nyaterm-gpui/target`，原工作区 tracked 文件未修改。

## 第一轮本机验证记录

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
- Windows：第一轮受控子进程和 host-tools 状态/菜单测试已在 [CI 42748387363](https://github.com/nyakang/nyaterm/actions/runs/42748387363/job/129605973210) 通过，覆盖信号拒绝、显式 KILL、Job 回收、继承管道、大输入、双路输出和输出上限。第二轮 [Windows CI](https://github.com/nyakang/nyaterm/actions/runs/42762757380/job/129652182097) 的 remote-ops 为 64 通过、2 失败：正常后台工作夹具超时、PowerShell 错误夹具得到未执行脚本的空成功。panic、信号限制及其余监督器用例通过。第三轮已修正这两项，待新 head runner 复验。独立监督器编译通过仅证明编译，**不是 Windows 行为验证**。
- macOS：第一轮 workspace 测试已在 [macOS-arm64 CI](https://github.com/nyakang/nyaterm/actions/runs/42748387363/job/129605973266) 通过。第二轮 [macOS-arm64 CI](https://github.com/nyakang/nyaterm/actions/runs/42762757380/job/129652182029) 的 desktop 与 remote-ops 用例通过，包括 launchd UID 脚本夹具；workspace 因无关 transport 超时夹具失败。真实 launchd 提权交互仍待实机验证，第三轮布局和采集用例待新 head runner。
- 已运行的读取使用 SSH 原有期限或修复后的本地期限结束；取消句柄用于阻止排队读取与丢弃过期结果，不承诺即时终止已运行的 SSH 调用。已开始的控制操作正常收尾。

## 单列的无关阻塞

- 上述第一轮 CI 的 Linux/macOS workspace 测试通过，但 Windows workspace 在未改动的 translation-client `rejected_http_status_and_malformed_payload_are_returned_to_the_caller` 失败：Windows 接受的 socket 继承非阻塞属性，`read().unwrap()` 收到 Win32 10035。已比对 CI 合并提交与主分支，该测试不属于 PR 修改。
- 同次 [Lint CI](https://github.com/nyakang/nyaterm/actions/runs/42748387363/job/129605973361) 在主分支已有的 `completion_runtime.rs` 两处测试线程创建触发架构白名单检查。相同命令在主分支 `ec9e97528` 可复现；本轮 worktree 的架构检查通过。
- 本机 root 身份下既有 files 只读权限测试失败，原 PR 基线同命令已复现；对应文件在本轮和原 PR 基线间无修改。第一轮以普通用户运行同一测试二进制通过，详情见上方记录。

- 第三轮 Linux 全量中的 `transport::environment::test::missing_variable_is_cached_until_refresh` 超时；包级复测另有 3 项真实 shell 环境加载超时。以 `c3336f331` 建立临时独立 worktree，执行同一包级命令也复现 missing-variable 超时（474 通过、1 失败、9 忽略），对应 transport 源码完全相同。单独执行该用例通过但耗时 8.95 秒；保留完整失败数字，不把定向复测累计为全量通过。
- 第二轮 macOS-arm64 CI 的 `transport::network_route::tests::stalled_proxy_handshake_has_a_network_timeout` 在准备阶段读取到 EOF，尚未进入预期握手断言。对应源码与原 PR 基线完全相同，本轮没有修改；该平台夹具失败单列，未声称 macOS workspace 通过。
- 第二轮 Windows CI 再次确认 translation-client 的三项测试因 Win32 10035 失败；Lint CI 再次确认相同主分支架构白名单问题。相关 remote-ops 失败单列在第三轮修正说明中，不作为无关排除项。

以上问题按本轮明确范围保留，不能将整个 CI 描述为通过。
