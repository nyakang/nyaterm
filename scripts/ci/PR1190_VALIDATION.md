# PR #1190 实施与验证记录

实施及完整工作区验证基线：`da2ba3ea596fe91fdb5f75b28fc7fe09bf41417b`。
推送前 PR 源分支基线：`c7163d392a5f7c255426d79dea5d5e5bce67d146`。
分支：`fix/pr1190-batch-edit`。
工作区：`/data/kang/nyaterm-pr1190`。
验证日期：2026-11-08，Linux x86_64，Rust 1.99.1，zsh 5.9。

四组修复提交；按后续授权交付到 PR #1190 的源分支 `fonaix/nyaterm:main`，不涉及合并、发布、数据迁移或第三方 fork 修订变更。

推送前保留源分支新增提交，将四组修复无冲突地 rebase 到最新源分支；`git range-diff` 确认代码补丁保持一致。原完整验证版本保存在本地 `fix/pr1190-batch-edit-da2ba3e` 分支。下文完整工作区测试和原生窗口结果对应原实施基线；最新源分支上的集成检查另见文末。

## 实施结果

| 问题 | 最终行为 |
| --- | --- |
| 混合协议批量编辑 | 默认进入通用范围；只显示实际包含的协议及数量。协议页签只修改原协议成员，不生成协议转换补丁。 |
| 草稿和字段状态 | 单一 feature owner 持有装箱草稿；范围切换移动并保留草稿。每个字段使用不修改、设置、清空三态；混合布尔值必须明确选择。 |
| 输入和秘密 | 普通设置值拒绝空白，清空必须显式选择；密码保留空格、使用 SecretString、不回填已有秘密。恢复不修改和关闭时释放秘密及输入历史。 |
| 保存协调 | 冻结补丁、原协议和草稿代次；后台先读取最新目标并检查删除或协议冲突，再校验并应用补丁。旧完成回调不会关闭新编辑器。 |
| 原子保存 | 仅收集实际变化的连接；加密准备后用一个 redb 写事务更新连接、密码、索引和同步记录，失败整体回滚。提交与列表刷新分别报告。 |
| 认证来源 | 显式用户名优先于账号用户名；设置账号默认跟随账号。清空账号同时去掉新旧引用并保持连接密码来源；清空密码不会恢复旧账号密码；密钥补丁不覆盖认证模式。 |
| 多选反馈 | 选中色优先于悬停和条纹，固定操作列同步。全选仅作用于当前结果；隐藏选择保留并计数；Shift 锚点稳定，筛选和删除清理失效锚点或选择。 |
| 输入主题 | input_surface 保留内置和自定义 input 颜色，统一投影到输入及编辑器。保留不透明锁屏、同步分组配色和本地化。 |
| zsh 历史 | 第一物理行使用无文件的 fc -p，下一行捕获状态；ready 独立输出且先于 pop；仅成功 push 才 pop，清理先于可能失败的 pop，不改变用户历史选项。 |
| 结构与 CI | 批量模块拆为字段、范围、草稿、选项、运行协调、视图和窗口；测试使用明确导入；正文滚动仅增加一条有上限和所有权说明的精确 allowlist。 |

## 定向回归

| 命令 | 结果 | 覆盖 |
| --- | --- | --- |
| cargo test -p nyaterm-core connection_batch --lib | 26 通过 | 原协议、字段默认值、认证来源、新旧账号引用、密码来源、密钥模式和未知字段。 |
| cargo test -p nyaterm-store --lib | 249 通过、1 项原有忽略 | 多连接提交；后续失败使密码、连接、三类索引和同步代次回滚；失效引用零写入；锁定密文、旧格式及未知字段兼容。 |
| cargo test -p nyaterm-desktop assets --lib | 22 通过 | 范围草稿、即时输入、秘密清理、冲突、过期回调与刷新失败、多选、按钮隔离、六份字段标题文案。 |
| cargo test -p nyaterm-ui --lib | 178 通过 | 半选转勾选后鼠标及键盘控制、主题投影、内置和自定义输入色，以及既有 UI 回归。 |
| cargo test -p nyaterm-transport zsh --lib | 8 通过 | 真实 PTY；Full/CwdOnly；上传、内联、rc 激活；原文件和用户命令保留；无注入标记和额外历史栈；SAVEHIST、历史选项和非 root。 |

所有 Cargo 验证使用独立的 `CARGO_TARGET_DIR=/data/kang/nyaterm-pr1190-target`，项目 crate 从本工作区重新编译；复用了现有外部依赖缓存。没有设置 RUST_MIN_STACK、全局提高栈大小或新增测试跳过。

## Linux 原生窗口

使用临时 portable 数据、三个无凭据的 SSH/RDP 示例连接、Xvfb 和 xdotool，启动实际 GPUI 应用验证：

- Ctrl 增选、半选、选中行与固定操作列底色。
- 混合协议直接打开通用范围，显示 Common (2)、SSH (1)、RDP (1)。
- 输入立即更新摘要并启用应用按钮；跨范围保留描述和端口草稿。
- 实际提交通用描述及 SSH 端口后重新打开：描述已保存，RDP 原协议和 3389 端口保持。
- 620 像素宽的批量窗口保留范围选择器和单列字段；720 像素宽的主窗口批量工具栏换行。
- 筛选隐藏 RDP 选择时显示 1 selected (1 hidden)；全选 SSH 结果显示 3 selected (1 hidden)，取消当前结果全选保留隐藏选择。
- Escape 可以关闭编辑窗口；输入框保持可输入的焦点。

原生截图为 `/tmp/pr1190-native-*.png`。截图生成后修正了混合值和编码标题文案；最终文案由六份 locale 完整性回归验证。原生启动出现系统缺少部分字体的警告，不影响上述观察。原生进程和专用 Xvfb 已停止，临时 portable 标记已移除，测试数据位于 `/tmp/pr1190-native-smoke/data`。

## 平台未完成项

Windows/macOS 原生环境不可用，不报告通过：

- Windows：仍需在 PR 基线与修复分支分别运行既有隐藏终端、窗口焦点测试，比较普通栈大小下的结果。静态比较中，PR 将整个草稿内联在 StartWorkspaceFeatureState，本分支改为 Box，减少嵌入状态及构造/复制的栈占用；这不能单独证明 Windows 栈溢出已经解决。
- macOS：仍需验证原生模态窗口、焦点恢复、Platform 多选以及 zsh。
- Linux 原生窗口只覆盖上述临时 Xvfb 场景；完整桌面环境中的 IME、无障碍辅助工具和所有主题的人工外观验收尚未完成。

主工作区 `/data/kang/nyaterm-gpui` 未被本任务修改，其独立的命令补全改动不属于本分支。

## root 环境下的权限测试

首次直接以 root 执行 `cargo test --workspace`，桌面全套为 2797 通过、15 项原有忽略；随后 files 的 `download_path::tests::staged_write_preserves_existing_permissions` 失败，因为 root 可绕过 0400 文件的写权限。该源文件与 PR 基线的 Git blob 均为 `2fddf63df0c81c86bf31a64897b969f25dccaa79`，本分支未修改它。

对同一已编译测试单独执行：root 重现失败，`runuser -u nobody` 为 1 通过。最终工作区测试使用临时 runner，仅让 `nyaterm_files-*` 测试进程以 nobody 执行，其余测试原样执行：

```text
SHELL=/bin/sh cargo test --workspace --config 'target.x86_64-unknown-linux-gnu.runner=["/tmp/pr1190-test-runner.sh"]' -- --test-threads=8
```

runner 没有筛选、忽略或跳过任何测试，也不修改 RUST_MIN_STACK；files 恢复权限测试预期的普通用户条件，transport 因下面记录的临时可执行脚本竞争使用 1 线程，其余包保持 8 线程。root 重现与非 root 通过的日志分别为 `/tmp/pr1190-permission-root.log` 和 `/tmp/pr1190-permission-test.log`。

临时 runner 内容如下；常规非 root 环境可以省去 files 的 runuser 分支：

```sh
#!/bin/sh
case "$1" in
  */nyaterm_files-*) exec /usr/sbin/runuser -u nobody -- "$@" ;;
  */nyaterm_transport-*) exec /usr/bin/python3 -c 'import os, sys; args = [arg for arg in sys.argv[1:] if not arg.startswith("--test-threads=")]; os.execv(args[0], args + ["--test-threads=1"])' "$@" ;;
  *) exec "$@" ;;
esac
```

仅添加 runner、保留默认测试并发的第二轮中，桌面有两项超时：`sync_sidebar_remote_password_mismatch_preserves_local_state_and_remote_pointer` 和 `windterm_password_prompt_rejects_empty_enter_and_cleans_up_after_submit`。当时机器负载约 20–27，两项在第一轮均通过；对同一桌面测试可执行文件逐项复测也均通过，耗时分别 13.18 秒和 12.51 秒。最终整套使用 `--test-threads=8`；没有修改测试断言、超时上限或跳过这些测试。默认并发失败日志为 `/tmp/pr1190-workspace-default-parallelism.log`，逐项复测日志为 `/tmp/pr1190-desktop-timeout-recheck.log`。

保留当前用户 bash、限制为 8 线程的第三轮中，桌面全套及 store/files 等均通过；transport 的 `environment::test::missing_variable_is_cached_until_refresh` 在原有 10 秒 shell 查询期限内超时。该 environment 模块与 PR 基线完全相同；单独复测通过，耗时 7.50 秒。交互登录 shell 的空命令实测为 bash 3.90 秒、sh 0.03 秒；使用 `SHELL=/bin/sh` 后完整 transport 为 468 通过、9 项原有忽略，耗时 13.65 秒。最终整套因此显式选择 sh 进行环境发现，避免依赖 root 的交互登录 bash 配置；不修改生产超时、用户 shell 配置或缓存逻辑。zsh/Bash/Fish 的专门 PTY 回归仍由测试显式启动对应真实 shell。第三轮与 transport 复测日志分别为 `/tmp/pr1190-workspace-eight-threads.log`、`/tmp/pr1190-transport-timeout-recheck.log` 和 `/tmp/pr1190-transport-controlled-shell.log`。

受控 shell 的第四轮中，transport 另一个既有测试 `resolve_refreshes_the_complete_snapshot_once_for_a_new_variable` 执行刚写入的脚本时遇到 `ExecutableFileBusy (Text file busy)`；对同一工作区测试可执行文件逐项复测通过。并发 fork 期间其他子进程可能短暂继承尚未关闭的写描述符，产生这类临时可执行文件竞争；最终 runner 仅对 transport 使用 `--test-threads=1`，避免同一测试进程内的并发脚本创建和进程启动。未修改 environment 生产或测试代码，也未加入重试或跳过断言。第四轮失败与同一可执行文件复测日志分别为 `/tmp/pr1190-workspace-controlled-shell.log` 和 `/tmp/pr1190-transport-refresh-same-binary.log`。

## 工作区检查

| 检查 | 结果 |
| --- | --- |
| python3 scripts/ci/check_architecture.py | 通过。 |
| cargo check --workspace | 通过。 |
| cargo test --workspace（上述受控 shell、用户和并发设置） | 5,889 通过、0 失败、37 项原有忽略；包含工作区单元、集成及文档测试。 |
| cargo fmt --all -- --check | 通过。 |
| cargo clippy --workspace --all-targets | 通过，最终无警告。 |

最终全套中 desktop 为 2,797 通过、15 项原有忽略；files 为 204 通过、1 项原有忽略；store 为 249 通过、1 项原有忽略；transport 为 468 通过、9 项原有忽略（串行耗时 77.11 秒）；UI 为 178 通过。

全套之后清理了 Clippy 指出的六处多余借用及同类型转换，未改变行为。清理后再次运行资产定向测试（22 通过）、格式检查和完整工作区 Clippy，均通过；没有重复已经通过的整套功能测试。定向补测日志为 `/tmp/pr1190-post-clippy-assets.log`，清理前的 Clippy 建议为 `/tmp/pr1190-clippy-before-cleanup.log`。

最终结果仅表示上述 Linux 环境和执行设置通过；原始 root/default 并发命令的失败、环境诊断和其他平台未完成项已分别记录。日志保存于 `/tmp/pr1190-final-*.log`，命令退出状态保存于 `/tmp/pr1190-matrix-results.json`。

## PR 源分支集成验证

在 `c7163d392a5f7c255426d79dea5d5e5bce67d146` 上保留源分支已有更新并应用四组修复。补齐该基线新增 `nyaterm-ui/src/tabs.rs` 中三处 rustfmt 差异，仅改变代码布局。集成后的检查如下，Cargo 使用上述独立 target 和 `SHELL=/bin/sh`：

| 检查 | 结果 |
| --- | --- |
| python3 scripts/ci/check_architecture.py | 通过。 |
| cargo fmt --all -- --check | 通过。 |
| cargo check --workspace | 通过。 |
| cargo test -p nyaterm-core connection_batch --lib | 26 通过。 |
| cargo test -p nyaterm-store --lib | 249 通过、1 项原有忽略。 |
| cargo test -p nyaterm-ui --lib | 184 通过，包含源分支新增页签回归。 |
| cargo test -p nyaterm-transport zsh --lib -- --test-threads=1 | 8 通过，真实 PTY。 |
| cargo test -p nyaterm-desktop assets --lib -- --test-threads=8 | 22 通过。 |
| cargo clippy --workspace --all-targets | 通过，无警告。 |

集成后执行上述定向回归及完整编译、lint；完整工作区功能测试和原生窗口记录仍对应原实施基线。Windows/macOS 原生验证与 Windows 栈溢出比较仍待完成。集成日志为 `/tmp/pr1190-push-*.log`，退出状态为 `/tmp/pr1190-push-results.json`。
