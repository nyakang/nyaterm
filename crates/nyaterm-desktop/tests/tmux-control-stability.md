# tmux Control Mode 稳定性修复验证

验证平台：Windows + WSL Ubuntu / tmux 3.4。修复分支从 `origin/main`
`b3c0f249efd627dcae5818224d2c763ad9f3483b`（已合并 #1080）建立。
没有修改持久化格式、依赖版本、SSH Host Key Verification 或命令审批机制。

## 根因与修改

- **Zoom 误删**：transport 使用完整 layout 保留了虚拟 Pane，但发给桌面的
  window.tree 来自 visible_layout。桌面层又用这些可见树推断 Pane 存亡，
  于是清理 Zoom 隐藏的 Session，并经普通 close 发出 kill-pane。
  `TmuxStatePayload.panes` 现在携带 transport 的完整映射集合；window.tree
  仅负责可见布局。后台 Window 的 Pane 同样保留。
- **Pane 移动**：不同 Window 的 layout-change 分批到达，局部布局的并集也可能
  短暂缺少正在移动的 Pane。现在只有成功的完整 list-windows 快照（或可靠退出
  通知）可以移除虚拟映射；局部通知可以创建、调整投影，但不按缺失销毁 Pane。
  快照先完成 reconcile，再发布状态，避免 Reattach 中间状态误删终端缓存。
- **Drop 死锁**：SessionManager 析构在持有 sessions Mutex 时 join SSH Worker，
  而 Worker 的 teardown 会通过 remove_tmux_pane 获取同一把锁。
  现在先关闭 EventQueue 唤醒输出生产者，在短锁作用域内取出 Session 集合，
  释放锁后再关闭及 join；普通 close 原本就释放锁后 join，继续保留此顺序。
- **隐式远端删除**：TmuxPaneTransport.close 和 close_detached 原先都会发送
  TmuxPaneClose。现在普通关闭只回收本地资源；专用 tmux_close_pane API 承载
  用户明确的 Pane 关闭动作，重复请求合并，失败可重试。成功请求保留映射，直到
  远端状态确认删除。关闭整个 tmux 标签也会释放投影树之外的宿主 SSH Session。
- **字节损坏**：ControlParser.next_line 原先先做 UTF-8 有损转换，污染后续
  octal decode。现在分帧和 output / extended-output 载荷按字节处理，只解码
  元数据；capture-pane 初始画面也保留字节，最终仍进入既有 SessionEvent::Output。
  octal 只接受有效的单字节三位八进制转义。
- **响应错位**：真实 tmux 3.4 中，new-window ; new-window 返回两个响应块，
  含未知命令的整行解析失败则只返回一个块。现在自由命令后另发随机 nonce
  display-message 作为边界，在边界前保留同一个响应关联；不按分号估算。
  异步服务端命令块不消费客户端查询 FIFO。嵌入 CR、LF、NUL 的自由命令在
  mailbox 入队前拒绝；响应标记失配时关闭控制通道，避免继续使用错误关联。

主要修改位置：

- transport/src/lib.rs：SessionManager::{drop,close_detached,tmux_close_pane}、
  TmuxPaneTransport::close。
- transport/src/tmux/session.rs：emit_state、reconcile_panes、apply_window_list、
  handle_message、handle_block、queue_user_command、response_kind、compose_seed_payload。
- transport/src/tmux/control.rs：ControlParser::next_line、parse_line、unescape_octal。
- transport/src/tmux/mailbox.rs：自由命令校验。
- desktop/src/features/tmux.rs：handle_session_tmux_state_event。
- desktop/src/features/terminal/terminal_runtime/sessions.rs：标签及 Pane 关闭入口。

## 生命周期

| 操作 | 本地行为 | 远端行为 |
| --- | --- | --- |
| 用户明确关闭 Pane | 专用 API 排队一次，确认后清理终端及映射 | kill-pane |
| 用户关闭 tmux 标签 | reaper 关闭宿主及本地投影，取消输出等待 | 退出控制客户端，保留 tmux Session / Pane |
| Detach | teardown 虚拟 transport，桌面折回 SSH Shell，可重新 Attach | detach-client，不删除 Pane |
| SSH EOF / 异常断开 | 清理映射和输出队列，丢弃旧代输入 | 不发送 kill-pane |
| 远端已经删除 Pane | 完整快照或退出通知清理本地状态 | 不重复发送 kill-pane |
| 应用退出 / SessionManager Drop | 关闭队列、移出集合、解锁、关闭及 join | 只关闭客户端，不隐式删除 Pane |

## 自动化验证

实际执行结果：

| 检查 | 结果 |
| --- | --- |
| cargo check -p nyaterm-transport -p nyaterm-desktop | 通过 |
| cargo test -p nyaterm-transport --lib | 最终 693 通过，11 忽略（含 4 项显式执行的真实 tmux 测试） |
| cargo test -p nyaterm-desktop --lib | 2513 通过，12 忽略 |
| 真实 tmux 3.4 / SSH 集成测试 | 4 项通过；历史范围断言修正后，额外连续 3 轮全部通过 |
| cargo check --workspace | 通过 |
| cargo test --workspace | 5030 通过，0 失败，42 忽略；随后新增的原始字节事件测试在 transport 全量测试中通过 |
| cargo clippy --workspace --all-targets | 通过，无警告（新增代码警告已修复并重跑） |
| cargo fmt --all -- --check | 未通过：main 原有 update/install.rs 第 432、668 行格式差异；该文件未修改 |
| 修改的 Rust 文件单独 rustfmt --check、git diff --check | 通过 |

可复现命令：

```powershell
cargo check -p nyaterm-transport -p nyaterm-desktop
cargo test -p nyaterm-transport --lib
cargo test -p nyaterm-desktop --lib
$env:NYATERM_TEST_TMUX_WSL = 'Ubuntu'
cargo test -p nyaterm-transport --lib tmux::integration_tests::real -- --ignored --nocapture
cargo check --workspace
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets
```

真实集成测试通过 loopback SSH 连接运行 `script` 提供的 PTY 上的 `tmux -CC`，
覆盖 nonce 握手、输出暂停/恢复与抓屏历史、复合命令及解析/执行错误、快速命令、
内部查询、嵌套 Split、Zoom/Unzoom、后台 Window、Pane 移动、显式 kill-pane、
Detach/Reattach、宿主关闭、服务端退出。退出验证比较了远端 Pane ID 和 PID。
一次并发重跑发现原有大输出历史断言受调度影响：本地保留 5000 行、tmux 默认
保留 2000 行，本地历史可能合法地比远端更长。测试现在逐行严格比较服务端仍保留的
整个历史范围，并允许更早的本地历史；没有放宽该范围内的内容一致性要求。
独立测试覆盖输出背压下的 Drop 锁顺序、8 个并发关闭请求去重、SSH EOF 清理、
原始非 UTF-8 / GBK / Big5 / NUL / FF / octal / 数据分片，以及 GPUI 隐藏 Pane 状态保留。

## 手工 GUI 回归（尚未执行）

1. 在独立测试 SSH 服务器中执行 `tmux -CC new-session -A -s nyaterm-stability`。
   通过工具栏建立左右 Split，再在其中一个 Pane 上下 Split；另建后台 Window。
   在另一条 SSH 连接记录 `tmux list-panes -s -t nyaterm-stability -F
   '#{pane_id} #{pane_pid} #{pane_dead}'` 的结果。
2. 对不同 Pane 反复 Zoom/Unzoom，Zoom 时切换 Window，再回原 Window。
   验证嵌套布局恢复、输入/Focus/Resize 正常、输出和历史保留，外部记录的 PID 不变。
   将一个 Pane 移到另一 Window，验证原终端状态保留。
3. 点击 Detach，再 Attach；关闭整个标签，再连接 Attach；正常退出 NyaTerm 后
   再启动 Attach。每一步都在外部 SSH 验证 Session、Pane 和 PID 继续存在。
   退出应用应能完成，且没有卡在输出生产者或 SSH Worker join。
4. 通过“关闭当前 Pane”执行明确关闭，验证只移除对应远端 Pane；快速重复点击
   不应重复执行删除。其余 Pane 的 PID 应保持不变。
5. 在测试环境中主动中断 SSH 连接（保持 tmux 服务端运行），验证本地退出响应，
   无僵留虚拟 Session；重连 Attach 后仍能看到原进程。
6. 同时测试 tmux 下的 SFTP 浏览、独立 SSH Shell、Local PTY；按设备条件补测
   Telnet / Serial。确认现有主机密钥确认和命令审批仍正常出现。

只验证了 tmux 3.4；未声称已完成其他 tmux 版本、macOS/Linux 原生 GPUI、
真实 GUI 点击、整机网络异常及实体串口的验证。自动化宿主关闭和 Drop 测试不能
替代上述完整应用退出测试。恶意服务端超大未结束协议行/响应块的总量限制不属于
本次修复，既有每 Pane 输出及发送队列流控测试不代表所有协议内存均有全局硬上限。
