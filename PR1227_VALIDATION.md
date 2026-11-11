# PR #1227 独立分析与修复验证

验证日期：2026-11-11（Asia/Shanghai）。

## 源码与交付

- 仓库：`nyakang/nyaterm`；原 PR head：`bc066fdb439a8812a92df99512fe1188aa1d8f2c`。
- 用户核实的主线整合基线：`3959330f0ef128eb5e4db6e408b4a17427e7ae43`；原 PR base：`326313bbd80c3486c93b473d3a202759349b9773`。
- Worktree：`/data/kang/nyaterm-pr1227`；分支：`fix/pr1227-review`。
- 主线整合提交：`811fd3f59`，保留原 PR 历史；首次交付时未推送，未合并远端 PR。
- 修复提交：`426a95301`（终端安全与会话边界）、`c610a1c08`（设置取消保留布局）、`1cb95da0e`（Windows ConPTY 回归与 CI）。测试覆盖的源码随后提交，未改变源码内容。
- 主工作区有其他任务的未提交修改；收尾时其 main 已由其他任务推进到 `68cca59a2`。本任务只编辑独立 worktree，整合并验证上述核实基线，保留主工作区的并行修改与提交。
- 构建使用 worktree 自己的 `target/`。从现有缓存复制第三方编译产物以减少冷启动；NyaTerm workspace crates 重新编译，不复用其他工作区的测试可执行文件。
- Diff 交付：`/data/kang/nyaterm-pr1227-fixes.patch` 相对整合提交 `811fd3f59`；`/data/kang/nyaterm-pr1227-vs-main.patch` 是相对主线 `3959330f0` 的完整最终 diff。两份均包含本验证记录。
- 后续按用户授权，将修复正常推送到 PR #1227 的来源分支 `fonaix/nyaterm:main`，首轮发布由 `bc066fdb439a` 快进到 `4837cab3a`；本发布记录随后随分支推送。Windows CI 实测仍待运行结果。

## 审查结论与处理

| 项目 | 调用链结论与处理 |
| --- | --- |
| 首行补全／纯空白 | 保留空 tracker resize 不失步、真实文本 resize 失步及纯空白抑制；修正原 PR 首行、尾随空格、su 测试，输入须先获得终端回显确认。 |
| CMD FillPolicy | 原 PR 的提交能力绕过与 main 搜索策略不一致。使用同一策略：确认输入、行尾、Shell 编辑证据及当前编辑器均成立才允许替换。标准 CMD 提示符在 main 已可提供编辑证据，不能将风险概括为所有 CMD 候选必然被过滤。 |
| CMD 回显确认 | 无 OSC 不等于无回显。恢复追加、替换的确认请求与超时；失配与超时取消旧请求；接受候选不会发送 Enter。 |
| 嵌套 Shell | core 分类当前提示符，desktop 持有临时编辑证据。实时 CMD／PowerShell 证据覆盖启动提示，同时修正补全解析方言；证据变化递增版本并使旧候选失效。未知 Windows 编辑器仅允许追加。 |
| su／分页器 | 用单一有来源的抑制状态取代 bool。确认子 Shell 提示符只解除 OSC 执行抑制，不解除明确的分页器／交互程序抑制；外层 su 历史记录尚未完成也不影响子 Shell 内分页器的抑制判断。 |
| Action Link | 保留 CR 提交；当前 Windows SSH 提示符选择 tracert；WSL、POSIX 提示符和 POSIX PowerShell 选择 traceroute。PowerShell 方言本身不再被当作 Windows OS 证据；未知远端沿用 POSIX 默认。 |
| 凭据 TTL | 区分缺帧与真实空提示符；有效非凭据／空提示符清理旧 TTL，不依赖已清空的 buffer。保留有效待填充凭据，先检查有效期及所属会话。 |
| 设置草稿 | 保留完整设置比较，统一会话布局字段投影用于比较和取消；取消真实设置修改时保留当前导航。设置页可编辑的 header、行角色高亮和文件管理器偏好仍参与脏判断。 |

## 兼容性与架构

- 不变更 redb 表、字段名、加密、凭据、备份或命令历史的持久化格式。
- 不变更依赖 fork 或固定 revision；不编辑 `temp/`。
- 新提示符／抑制策略类型不序列化；core 保持无 GPUI，任务与快照协调仍由 desktop 拥有。
- 保留 core 的 `actions_for_match` 兼容入口和现有 CompletionFillPolicy 变体。
- 不安装 Windows Shell 动态 Hook。无法识别的 Windows／远程自定义提示符保守降级为仅追加；即使远程启动方言是 Bash，也不能凭它排除嵌套 CMD。原有未分类 ANSI 编辑器回退仅保留给本地 POSIX 会话。PowerShell 自定义 provider 路径无法单独确定 OS。

## 实际验证

环境：Linux x86_64，Rust 1.99.1。下表只记录本分支实际运行结果，原作者的测试描述不作为独立验证。

| 命令／场景 | 结果 |
| --- | --- |
| `python3 scripts/ci/check_architecture.py` | 通过。 |
| `cargo test -p nyaterm-core --locked` | 通过：844 项单元测试、27 项集成测试；2 项原有忽略项。 |
| `cargo check -p nyaterm-desktop --all-targets --locked` | 通过；首轮的新测试类型适配遗漏已修正。远程降级补充后的编译由最终 workspace 检查／测试覆盖。 |
| `cargo test -p nyaterm-desktop --lib features::terminal:: --locked -- --test-threads=4` | 最终通过：547 项，5 项原有忽略项。 |
| `cargo test -p nyaterm-desktop --lib settings_runtime::draft:: --locked -- --test-threads=4` | 通过：42 项。 |
| `cargo test -p nyaterm-transport --lib local_shell_integration --locked -- --test-threads=1` | 通过：14 项。 |
| `python3 -m unittest scripts.tests.test_select_rust_jobs -v` | 通过：30 项；Windows CI 的选择规则保留。 |
| `cargo check --workspace --locked` | 最终通过。 |
| `cargo test --workspace --locked --no-fail-fast`（下述权限 runner） | 完整执行：6257 项通过、1 项权限夹具失败、40 项原有忽略项；退出码 101。唯一失败及 files 全库已按正确身份复测通过，详见下文。 |
| `cargo fmt --all -- --check` | 最终通过。 |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | 通过，无警告。 |
| `git diff --check` | 通过。 |

首轮 desktop 编译发现测试比较类型未随编辑器证据类型更新，已修正。首轮终端回归有两项夹具失败：重复初始化残留 `awaiting_snapshot`；重建屏幕使等长文本替换复用了旧行 revision。分别显式重置夹具和使用同一 screen 重绘，保留主线旧帧拒绝逻辑；重跑全部终端回归后通过。

进一步调用链复核发现：远端方言为 Unknown 或旧 Bash 时，自定义提示符也可能属于嵌套 CMD，不能沿用未分类 ANSI 回退。已收紧到本地 POSIX 会话并增加回归。为验证最终代码，第一轮 workspace 测试在构建阶段主动终止，随后重新运行；不能把被中断的运行计为通过。最终终端回归包含这个新场景。

本机以 root 执行。最终完整测试通过 Cargo runner 使用 `setpriv --bounding-set=-dac_override,-dac_read_search --` 去掉 DAC 权限绕过能力，使只读／无权限文件场景仍按文件权限检查；未跳过任何失败测试。

唯一失败是未被本 PR 修改的 `nyaterm-files::remote_file::upload_tests::posix_shell_upload_preserves_metadata_through_public_entry_points`：测试见 UID 为 0，会把夹具文件所有者改成 65534；去掉 root 的 DAC 绕过能力后，下一次重写夹具在上传调用之前就收到 Permission denied。这是 root 分支与 runner 的条件冲突。使用同一轮 Cargo 编译产物复测：普通用户身份的 files 全库 215 项通过、1 项原有忽略项；正常 root 权限的失败用例单独执行也通过。未修改 files 代码，也未把原命令退出码 101 改记为通过。

```bash
cargo test --workspace --locked --no-fail-fast --config 'target.x86_64-unknown-linux-gnu.runner=["setpriv", "--bounding-set=-dac_override,-dac_read_search", "--"]' -- --test-threads=4
setpriv --reuid=65534 --regid=65534 --clear-groups target/debug/deps/nyaterm_files-09d31311042e9b26 --test-threads=4
target/debug/deps/nyaterm_files-09d31311042e9b26 --exact remote_file::upload_tests::posix_shell_upload_preserves_metadata_through_public_entry_points --test-threads=1
```

全量运行还独立验证了 desktop 2857 项（15 项原有忽略项）、transport 493 项（9 项原有忽略项），以及存储、终端、GPUI 终端、各 helper 和其他 workspace crate 的用例。计数包括单元、集成和文档测试，不包含另行定向运行的重复次数。

日志位于 `/tmp/nyaterm-pr1227-{core-tests,terminal-tests-final2,settings-tests-final,transport-tests,workspace-check-final2,workspace-tests-final2,files-nonroot-tests,files-root-upload-test,clippy,fmt-final,architecture-final,ci-tests}.log`。Windows 测试源码另外经过 Linux host 的 `rustc --test --emit=metadata` API／类型检查；这项检查不代表 Windows 编译或 ConPTY 运行通过。

关键回归证据（测试均在行为所属模块）：

| 行为 | 回归 |
| --- | --- |
| 首行、空白与 su | `startup_layout_resize_keeps_first_line_input_eligible_for_suggestions`、尾随空格及完整 su 往返；首行输入未回显时仍拒绝搜索。 |
| 嵌套 Windows 编辑器 | `nested_windows_prompt_changes_invalidate_candidates_and_select_current_delete_encoding`；CMD／PowerShell 启动方言均覆盖，验证当前解析方言、搜索 FillPolicy、旧候选撤销与实际写入字节。 |
| 自定义提示符与安全门禁 | `unclassified_windows_editor_filters_replacement_and_rejects_stale_fill`、`cmd_replacement_requires_confirmed_input_and_editor_capability`。 |
| Unknown／旧 POSIX 方言的远程自定义提示符 | `unclassified_remote_editor_never_infers_ansi_from_a_shell_launch`；即使已跟踪 Shell 启动，也只追加，不发送假定的删除字节。 |
| CMD 确认与禁止自动执行 | `cmd_completion_appends_and_waits_for_confirmed_echo`、失配和超时取消回归；TCP 写入断言验证没有额外 Enter。 |
| 明确交互程序抑制 | `shell_like_pager_echo_cannot_release_explicit_interactive_suppression`；外层 su 历史未结束时仍能识别子 Shell 的 less。 |
| Action Link 平台 | `action_links_use_current_windows_remote_and_posix_powershell_prompts`、core 工具拼写及桌面 CR 归一化回归。 |
| 凭据空行与缺帧 | `credential_window_ends_at_a_real_empty_prompt_but_survives_missing_frames`、`empty_prompt_preserves_only_current_unexpired_pending_credentials`。 |
| 设置取消与可编辑偏好 | `cancelling_settings_restores_real_edits_and_preserves_live_navigation`、`header_and_file_manager_preferences_still_dirty_the_whole_settings_comparison`，以及原 PR 导航不标脏回归。 |

## 平台验证边界

Windows 实际 ConPTY 回归已添加到 transport，并接入现有 Windows CI 的独立串行步骤：

```text
cargo test -p nyaterm-transport --lib windows_command_input_tests --locked -- --ignored --test-threads=1
```

该回归从 CMD／Windows PowerShell 两种启动程序开始，检查未提交行不执行、整行删除后 CR 执行，以及嵌套另一 Shell、退出返回的完整往返。使用临时文件结果作为执行证据，避免把输入回显误当成执行输出。

本环境无法实际运行 Windows ConPTY、Windows SSH 或 macOS；新增 Windows 回归尚待 Windows CI 执行，不能标为通过。Linux 的确定性快照及真实 TCP 写入测试覆盖方言、编辑字节、确认与拒绝状态，但不替代上述平台实测。
