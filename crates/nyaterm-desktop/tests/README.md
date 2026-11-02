# Shell Free Type 验证

## 命令建议安全填充

建议填充只允许对最新快照确认的行尾输入追加原始字符串后缀，不发送清行、移动、
退格或 Enter。相同内容不写入；非前缀目标拒绝替换。Enter 选择建议后也需要再次
手动按 Enter 执行。OSC、SSH 类型和提示符不作为编辑键或自动执行语义的证明。

自动化入口：

```powershell
cargo test -p nyaterm-desktop --lib features::terminal
cargo test -p nyaterm-desktop --test shell_free_type_windows -- --ignored
```

GPUI/TCP 测试检查实际字节、输入版本、回显确认、超时及迟到回显、Session 边界、
密码/锁屏/alternate screen、同步配置修订、编码失败、Kitty 手动 Enter 和历史记录。
真实 ConPTY 测试额外确认 PowerShell/PSReadLine 能把普通后缀留在未提交的编辑行。
这些测试不证明未知远端程序支持任何编辑快捷键。

人工测试矩阵（需要分别记录执行环境和结果，未运行项不能记为通过）：

| 环境 | 操作与检查 |
| --- | --- |
| Windows PowerShell/PSReadLine | Tab/Enter 选择精确前缀建议，仅追加；再次独立 Enter 执行一次；检查首尾空格、中文和历史 |
| SSH Bash/Readline，有/无 OSC | 重复填充、拒绝整行替换、切换/断开、等待时输入及 Ctrl+C；不得自动 Enter |
| 真实 H3C 类 CLI | `display ver` → `display version` 只追加 `sion`；`show ver` → 目标应拒绝；由用户手动执行 |
| 同步输入、密码、TUI | 接受旧建议不能发送内容或 Enter；同步开关切回原值也不能恢复旧请求 |

可使用隔离会话的测试服务端捕获发送字节；报告只保留合成输入，避免记录真实凭据、
终端正文或用户命令。命令建议之外的普通手动编辑仍复用既有 Shell Editing。

本次调用链由 `command_suggestion_input()` 拼接清行/命令/Enter 后直接发送，改为：

```text
选择建议 → 校验 Session/输入版本/快照/同步修订 → plan_suggestion_fill()
  → 相同：零写入；非前缀或不可信：拒绝
  → 精确前缀：发送后缀 → note_shell_editing_input() → Pending
  → reconcile_shell_editing()：只确认填充，不发送 Enter
  → 用户独立按 Enter → 会话按键编码 → 成功写入 → 记录一次普通命令历史
```

实际改动位置与核心类型：

| 文件（相对 desktop） | 作用 |
| --- | --- |
| `src/features/terminal/command_suggestions/fill.rs` | `SuggestionFill`、`SuggestionSubmitResult` 及纯后缀决策 |
| `src/features/terminal/command_suggestions/mod.rs` | 搜索版本校验、接受门禁、提交与弹窗提示 |
| `src/models/connections.rs` | `CommandSuggestionState` 的输入版本和同步修订 |
| `src/features/terminal/editing_state.rs` | `SuggestionFillRequest`、会话所有权及确认身份 |
| `src/features/terminal/editing_runtime.rs` | 回显、超时及失效时消费或取消请求 |
| `src/features/sync_input.rs` | 非持久化同步路由修订号 |
| `src/features/terminal/terminal_runtime/view_io/input.rs` | 输入取消旧请求；Kitty Enter 的逻辑历史处理 |
| `src/features/terminal/terminal_selection_runtime/smart_input.rs` | 其他编辑取消建议请求，保留既有编辑算法 |
| `src/features/terminal/command_suggestions/fill_tests.rs` | GPUI/TCP 字节与生命周期回归 |
| `tests/shell_free_type_windows.rs` | 真实 PowerShell 后缀回显验证 |
| `tests/README.md` | 验证说明与人工矩阵 |

本版明确不提供建议整行替换和自动执行；缺失回显、复杂字素、未知编码、未知远端
语义均不能通过追加控制字符补救。无 OSC 的空输入不能得到可靠边界时也拒绝填充。
未改持久化格式、依赖或 vendor；没有新增设置、设备特例和远端探测。

### 本轮验证结果（2026-10-30，Windows）

以下均为本次实际运行结果，不沿用后文旧轮次记录：

| 命令 | 结果 |
| --- | --- |
| `cargo fmt --all -- --check` | 通过 |
| `cargo check -p nyaterm-desktop` | 通过 |
| `cargo check --workspace` | 通过 |
| `cargo test -p nyaterm-core -p nyaterm-terminal -p nyaterm-terminal-gpui` | 1052 通过，4 忽略，0 失败 |
| `cargo test -p nyaterm-desktop --lib features::terminal` | 443 通过，2 忽略，0 失败 |
| `cargo test -p nyaterm-desktop --test shell_free_type_windows -- --ignored` | 两项真实 PowerShell/ConPTY 测试通过 |
| `cargo test --workspace -j 2` | 合计 4750 通过，36 忽略，0 失败；desktop 单元测试 2400 通过、11 忽略 |
| `cargo clippy --workspace --all-targets` | 通过，无警告 |

workspace 的忽略数包括需显式启用的环境测试；上表中的 PowerShell 两项已另外运行。
SSH Bash/Readline、真实 H3C/Huawei/Cisco 设备及原生窗口人工交互本轮未实测，不能
据 GPUI/TCP 测试宣称设备语义兼容。真实设备仍须按上方矩阵验证。

编辑模型位于 `nyaterm-core::terminal::editing`，真实 cell 映射和 OSC 输入锚点位于
`nyaterm-terminal`，按会话保存的状态与 GPUI 协调位于 desktop 的
`features/terminal/editing_state.rs` 和 `editing_runtime.rs`。

编辑只在输入内容与实际光标完整匹配后启用。无 OSC 的回退要求光标所在的单行或
软换行区域中输入唯一匹配。移动、删除和覆盖写入成功后进入 Pending，完整回显
才确认；部分重绘、重复文本、复杂字素和过期选区均不能授权合成编辑。

## 自动验证

```powershell
cargo test -p nyaterm-core -p nyaterm-terminal -p nyaterm-terminal-gpui
cargo test -p nyaterm-desktop --lib features::terminal::editing
cargo test -p nyaterm-desktop --lib replacement_paste_frames_only_inserted_encoded_text
cargo check -p nyaterm-app
```

desktop 状态测试覆盖建议关闭和外层命令抑制下的无 OSC 回退、MouseUp
保留选区、失败写入、部分回显、旧快照禁用、超时、最新点击、OSC 边界和密码/鼠标报告/广播
门禁。IME 测试使用 GPUI 输入处理器及真实本地 TCP 连接，检查预编辑不修改输入、
物理文字键交由提交入口、覆盖只发送一次。粘贴载荷测试检查编码后的正文被包装，
移动与删除序列在 bracketed-paste 外。

## 本轮结果（2026-10-04）

为保留主工作区正在并行修改的 AI、Serial、transfers 和 VNC 代码，本次功能补丁
另以 `e77524b39` 为基线应用到隔离工作树验证，未加入那些并行功能的补丁。

* `cargo check --workspace`、`cargo test --workspace`、
  `cargo fmt --all -- --check` 和 `cargo clippy --workspace --all-targets` 均通过。
* desktop 全量测试为 1707 项通过、7 项忽略；冻结后的编辑状态测试 10 项通过。
* terminal 178 项通过；terminal-gpui 145 项通过、1 项忽略。
* Windows PowerShell ConPTY 测试显式运行并通过。

这些结果覆盖本次功能及基线代码。主工作区验证期间出现过并行 AI proxy 测试失败、
VNC 新事件匹配分支未覆盖及其他功能文件的格式差异；上述结果不代表那些并行改动
已完成验证。

## Windows shell 验证

```powershell
cargo test -p nyaterm-desktop --test shell_free_type_windows -- --ignored
```

该测试启动隔离的 Windows PowerShell + PSReadLine，经真实 ConPTY 传输验证光标
移动、删除、文字覆盖、粘贴覆盖、软换行以及跨软换行向左移动。测试不执行输入的
命令，不保存历史，不打印终端正文，并在退出时关闭会话。

本轮已运行并通过该 ConPTY 测试。原生窗口的实际鼠标点击/拖选、操作系统 IME、
SSH bash/zsh/fish 及 Docker shell 尚未实测；GPUI/TCP 测试不能替代这些人工场景。

人工验证时，分别在有/无 OSC 的 shell 输入尾空格、中文宽字符和软换行命令，
依次检查点击定位、拖选及双/三击后的删除、左右键折叠、文字/IME/粘贴覆盖。
关闭命令建议后重复；在 Docker/SSH 嵌套 shell 确认外层 command-running
不阻止唯一回显匹配。在重复文本、历史滚动、密码、alternate screen、鼠标报告和
广播模式检查操作回退为普通终端行为。
