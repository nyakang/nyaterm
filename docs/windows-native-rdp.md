# Windows Native RDP

Windows Native 使用系统安装的 `MsRdpClient9NotSafeForScripting`，要求
`IMsRdpClient9`。它是用户明确选择的进程内系统组件；Builtin/IronRDP 和
VNC 仍使用隔离 helper。系统 `mstscax.dll` 不随应用分发。

## 所有权与线程

`nyaterm-windows-rdp` 的生产依赖不包含 GPUI。Rust 宿主不可 Send/Sync，
窄 C ABI 不公开 COM 指针。C++17 ATL shim 从本机系统类型库生成接口。
事件回调只记录类型化事件，桌面层通过 generation 校验提交结果。

桌面维护循环先从 Entity 提取操作，释放 GPUI 借用，再在创建控件的
UI/STA 线程执行操作，最后提交。布局阶段只记录与窗口客户区相交后的
bounds，不调用 COM。初始化取消、会话替换和迟到事件不能恢复旧会话。

Connected、登录完成、重连和绘制子树就绪分开处理；连接建立且绘制子树
有效后即显示登录界面，避免等待用户输入时被隐藏；登录完成仍单独推进
会话状态。切页签只改变可见性。FitWindow 使用测量后的物理尺寸和
DPI，动态更新防抖 400 ms，每个尺寸最多尝试四次；确认接口不支持后才
启用 SmartSizing。固定分辨率保持配置尺寸。

## 浮层与输入

GPUI fork 将原窗口同一组件树中的浮层绘制到主窗口拥有的透明 Win32
视觉 HWND，另用按交互区域裁剪的输入 HWND 接收鼠标。两者均为 owned
popup，不是全局置顶窗口，也不拥有独立业务状态。视觉层与主窗口共用
Direct3D 设备、纹理图集和渲染管线，设备重建后重建合成资源。

`nyaterm-ui` 提供区域、被动和模态三种呈现封装。菜单控件区域接收
输入；定位用的整窗壳、阴影和 tooltip 不扩大输入区域；模态层阻断整个
客户区。菜单外原始按下先通知关闭菜单，再交由原命中目标处理。原生子
窗口的原始输入通过在 UI 线程回调的低层鼠标 hook 观察，仅处理注册
owner 的子窗口点击；不重放、不再分发 GPUI bubble。
原生键鼠、IME 和 Windows 快捷键继续由 ActiveX 处理，Canvas 全局键盘
捕获在 Native 会话上禁用。Ctrl+Alt+Home 返回本地焦点。

合成初始化或运行失败会使 Native 会话显示明确错误。失败界面提供手动
切换 Builtin；仅改变当前会话，不修改保存的连接。系统认证授权、服务器
身份验证和剪贴板/资源授权继续执行，不能通过自动降级绕过。

## 认证与剪贴板策略

选择 Windows 后端即使用系统证书验证，`AuthenticationLevel = 2`。验证失败时，
由系统控件提示用户继续或取消连接，不再使用严格拒绝模式。“不再询问”复选框
及其保存行为以本机系统控件为准，不承诺与独立 mstsc 完全一致；不增加自动信任
或绕过提示的逻辑。认证弹窗通过 `UIParentWindowHandle` 关联应用主窗口，
避免默认关联隐藏的 ActiveX 子窗口；调用 Connect 前先设置控件尺寸。旧数据中缺失的
`windows_authentication` 在启动配置中补为 `system`，未知值仍拒绝使用。
保存的 Builtin 证书策略保持不变，不转换为系统信任授权。

RDP 剪贴板始终开启，连接编辑器不再提供模式选项。Builtin 始终启用文本和文件／目录通道；Native 将 `RedirectClipboard` 设置为 true，由微软控件处理完整重定向，实际格式能力仍受控件及服务端策略限制。应用的剪贴板轮询继续跳过 Native，避免重复处理。

保存字段与旧值继续兼容读写；只在临时启动配置中归一化为 `TextAndFiles`，覆盖启动、恢复、重连和后端切换，不批量改写旧连接。新连接默认保存完整模式。本机离线真实控件测试读回确认剪贴板已开启、动态资源仍关闭；该结果不能代替真实双向复制验收。

光标诊断默认关闭；设置 `RUST_LOG=warn,gpui::native_cursor=trace,nyaterm_windows_rdp::presentation=trace` 可记录消息来源 HWND、命中窗口、焦点 HWND、光标句柄，以及实际视口／远程分辨率更新，不记录窗口标题、连接数据或剪贴板内容。GPUI 不覆盖原生子树的光标，owned popup 菜单仍使用 GPUI 光标。相同视口继续去重，动态分辨率继续防抖。

动态驱动器和设备重定向通过 `IMsRdpClientNonScriptable3` 关闭；其他设置使用
类型库的强类型接口，初始化失败返回具体属性和 HRESULT，不包含连接数据。

## 关闭与发布

关闭期间停止新输入和操作，将焦点归还父窗口。优先 RequestClose 并等待
断开确认，正常等待预算为 3 秒。注销事件和释放前检查回调计数；无法
安全释放的宿主留在 UI 线程退休队列，每秒重试，最长再等待 60 秒。
最终仍不安全时保留上下文并记录脱敏计数，避免悬空回调。
同步 COM 调用本身若卡死，截止时间无法抢占它。

应用退出先处理 Native 宿主和退休队列，再等待后台任务退出。旧
`nyaterm-activex-helper`、专用 IPC 和环境变量入口已移除；IronRDP/VNC
共享 IPC 版本保持不变。便携包、NSIS 和 MSI 更新均清理旧 helper。
CI 安装 x64/ARM64 MSVC ATL 工具链。Windows 发布脚本统一为应用和 helper
启用 `target-feature=+crt-static`，使 Rust 与 C++ 使用同一种静态 CRT，
避免增加 VC++ redistributable DLL 的必需依赖。开发构建继续使用默认
工具链运行库；无需手动进入 developer shell 即可发现本机 MSVC/ATL。

## 验收记录

用户已授权使用 computer-use 验证真实会话，Windows x64 桌面已可访问。
测试连接使用已有保存项；不要
将密码放入命令行、日志或测试文件。独立 smoke 程序启动方式：

```powershell
cargo run -p nyaterm-windows-rdp --example native_overlay_smoke -- <保存的连接名称>
```

此前在严格拒绝模式下，真实连接到达 Windows 证书验证界面，但服务器证书被
系统拒绝。改为提示模式后的继续/取消行为仍待真实连接验收。此前结果只说明
创建/连接已到达系统验证阶段，不能
视为远程画面或交互验收通过。

| 项目 | 验收要求 |
| --- | --- |
| 登录与安全 | 真实登录、失败认证、证书警告后的继续/取消、剪贴板双向文本及文件／目录传输 |
| 输入 | 中文 IME、系统快捷键、Ctrl+Alt+Home、无双重输入 |
| 浮层 | 菜单区域命中、外部点击只执行一次、tooltip 穿透、模态阻断；远程画面持续更新 |
| 布局 | 连续切页签、分屏、最大化、跨 DPI 显示器、动态/固定分辨率 |
| 生命周期 | 断网重连、正常/强制关闭、退出、100 次循环后 HWND/句柄/内存无持续增长 |
| ARM64 实机 | 在 ARM64 Windows 上单独记录真实会话；交叉编译不能替代 |

自动化检查结果在交付说明中列明。编译、日志和 HRESULT 不能替代上表的
真实画面与交互验收。

## Fork revision

- GPUI/Zed `nyaterm`: `8ecf76c3abf4f3c564b14ce9de5181c9b42244b5`。
- gpui-kit `nyaterm`: `30552806c6baf9e86b5ee5e087f462c6a1332972`。

两者的补丁、验证和剩余实机验收均记录在各自 `NYATERM.md`；主仓库
`Cargo.toml` / `Cargo.lock` 固定远端 revision，不使用本地路径覆盖。

## 本轮自动化验证（Windows x64）

- `cargo check --workspace --locked` 已通过。
- `cargo test --workspace --locked` 通过：5037 项通过、0 失败、42 项忽略，
  包含 IronRDP/VNC helper 握手、正常断开和崩溃/挂起回收测试，以及
  RDP 历史后端别名、未知值、认证和剪贴板限制的备份/同步往返。
- `cargo clippy --workspace --all-targets --locked -- -D warnings` 已通过。
- `cargo fmt --all -- --check` 和 `git diff --check` 已通过。
- `cargo build -p nyaterm-app -p nyaterm-rdp-helper -p nyaterm-vnc-helper
  --bins --locked` 已通过；本机调试版为 `target/debug/nyaterm.exe`。
  本轮构建使用单任务以避开 Windows 页面文件并发压力。
- Native 控制器/生命周期/显示/清理测试 10 项通过，包含退出发生在已提取
  操作期间的取消与等待；GPUI 浮层测试 5 项、Windows HWND 生命周期测试
  1 项通过。这些测试不连接远程服务器。
- Python 脚本测试共 141 项，135 项通过、6 项因本机工具条件跳过，包含两种 Windows
  架构的包契约、旧 helper 清理和静态 CRT 构建参数。
- 新 crate 的 Linux 生产路径检查通过：
  `cargo check -p nyaterm-windows-rdp --lib --target x86_64-unknown-linux-gnu --locked`。
  这不代表 Linux/macOS 整个应用的构建已在本机验证。
- 独立 Windows x64 静态 CRT 链接程序已执行；无效父 HWND 被明确拒绝。
  正式发布脚本的 x64 `build_application` 已完整通过，包含主程序、IronRDP/VNC
  helper 和已有的 MCP 可执行文件。产物位于
  `target/x86_64-pc-windows-msvc/release/`；主程序为 `nyaterm.exe`。
  `dumpbin /DEPENDENTS` 确认这四个产物均无 MSVCP/VCRUNTIME 动态依赖。
  这些检查不能替代安装包和真实 RDP 画面的验收。
- ARM64 交叉检查因本机缺少 `atlmfc/lib/arm64/atls.lib` 停止；自动安装
  ATL 返回 5007。CI 已增加 ATL/ARM64 组件安装，但尚未在本轮运行远端
  发布任务，不能记录为 ARM64 构建通过。

## ActiveX 初始化与设置联动修复验证（Windows x64）

- 本机真实 ActiveX 控件离线初始化、策略读回及释放测试通过，不连接服务器。
- `cargo check --workspace --locked -j 1` 通过。
- `cargo test --workspace --locked -j 1`：5046 项通过、0 失败、42 项忽略；
  包含旧配置归一化、未知认证拒绝、编辑器联动及启动/重试回归。
- `cargo clippy --workspace --all-targets --locked -j 1 -- -D warnings` 通过。
- `cargo fmt --all -- --check` 和 `git diff --check` 通过。
- 尚未完成真实服务器登录、证书警告后的继续/取消、断网重连、剪贴板双向不传输及 ARM64 实机验收。

## 系统证书提示模式修复验证（Windows x64）

- `AuthenticationLevel = 2` 的真实控件离线读回测试通过；按当时策略，剪贴板及动态资源保持禁用（现行剪贴板策略见上文）。
- workspace check、test、Clippy（`-D warnings`）、fmt 和 diff 检查均通过。
- workspace 测试：5046 项通过、0 失败、42 项忽略。
- `cargo build -p nyaterm-app --bin nyaterm --locked -j 1` 通过，调试版程序已更新。
- 真实服务器证书警告的继续/取消，以及“不再询问”复选框和记忆行为，仍待实机验证。

## 登录界面与认证弹窗修复验证（Windows x64）

- 系统控件离线读回确认 `UIParentWindowHandle` 为应用父窗口，证书提示模式为 2。
- 状态回归覆盖连接建立但尚未登录完成、绘制未就绪、重连及关闭时的显示条件。
- Connect 前先设置控件视口尺寸；Connected 时允许显示并聚焦远端登录界面。
- workspace 测试 5046 项通过、0 失败、42 项忽略；check、Clippy（`-D warnings`）、fmt 和 diff 检查通过。
- 调试版 `target/debug/nyaterm.exe` 已重新构建。
- computer-use 启动因 `GetCursorPos` 返回拒绝访问（0x80070005）受阻，停止 UI 操作；
  待用户解锁可访问桌面后，再验证真实会话画面和认证弹窗。尚未宣称现场问题已复现或验收通过。

## 持续显示“正在协商”的后续排查（Windows x64）

- 用户确认同一连接在 mstsc 可登录；NyaTerm 无认证弹窗且会打断同账号既有会话。
  此现象说明不能仅凭应用占位文案判定仍处于安全协商，尚需核对控件事件与画面。
- 修正 OLE 宿主顺序：`CoCreateInstance` → `IPersistStreamInit::InitNew` →
  订阅事件 → `AtlAxAttachControl` → 配置。ATL 的 AttachControl 假定调用者已初始化，
  不会代为调用 InitNew；失败时返回对应接口/方法及 HRESULT。
- 新增开始连接、认证警告显示/关闭、登录通知的事件诊断，以及连接数字状态、
  实际控件画面尺寸/可见性和显示条件的变更日志。不记录地址、账号、凭据或会话内容。
  `drawing_ready` 改为检查控件报告的实际子窗口及有效尺寸，连接状态读取失败不再默认为未连接。
- 离线真实控件测试检查 800×600 画面布局；回环 TCP 测试验证开始连接及断开回调，
  不传输凭据，也不连接真实服务器。回环事件测试在补 InitNew 之前也能通过，
  因此它只验证事件链路，不能证明 InitNew 是现场卡住的唯一原因。
- 解锁桌面后已通过 computer-use 复现占位文案持续显示；同次连接的脱敏事件
  依次到达 `Connected`、`LoginComplete`，`get_Connected = 1`，实际子窗口尺寸
  有效且可见。由此确认远端已登录，问题在本地画面合成层级。
- GPUI owner 的 DirectComposition target 原先固定在 HWND 子窗口上方，遮住
  ActiveX 画面。原生子窗口模式现在切换到下方 target；菜单和弹窗继续使用
  独立 owned popup。切换时先从旧 target 移除 visual root，再挂到新 target 并
  一次提交，避免同一 visual 同时挂载导致 `E_INVALIDARG`；设备恢复也保留层级。
- Zed Windows 测试通过：55 项通过、0 失败、1 项忽略，包含真实 compositor
  的反复切换和幂等回归。真实远程画面与交互验收在主程序依赖同步后进行。

## 合成层修复后的自动化验证（Windows x64）

- 使用远端固定 revision，`Cargo.lock` 中 GPUI 与 gpui-kit 各只有一套来源。
- `cargo check --workspace --locked -j 1` 通过。
- `cargo test --workspace --locked -j 1`：5048 项通过、0 失败、42 项忽略。
  原生 RDP 13 项测试全部通过，包括真实系统控件离线初始化、策略读回、
  有效绘图窗口尺寸及回环连接事件测试。
- `cargo clippy --workspace --all-targets --locked -j 1 -- -D warnings` 通过。
- `cargo fmt --all -- --check` 和 `git diff --check` 通过。
- `cargo build -p nyaterm-app --bin nyaterm --locked -j 1` 通过，
  调试版程序位于 `target/debug/nyaterm.exe`。

## 主窗口重定向的后续修正（Windows x64）

- 仅调整 DirectComposition target 层级后，实机依然显示占位文字；同次连接
  已完成 `LoginComplete`。此前自动化通过不能视为画面问题已修复。
- GPUI 主窗口创建时的 `WS_EX_NOREDIRECTIONBITMAP` 也会妨碍传统 HWND
  子窗口绘制。Windows 拒绝运行中切换该标志，因此在窗口创建参数增加
  `native_child_surfaces`，NyaTerm 工作区和 smoke 窗口在创建时启用。
- 未声明原生子窗口支持的窗口拒绝进入原生浮层模式，并返回明确提示。
  普通 GPUI 窗口默认保持原有创建样式。
- 主窗口保留重定向表面后，实机仍显示占位文字；还必须启用父窗口的
  `WS_CLIPCHILDREN`，使 DirectComposition 遵循 HWND 子窗口裁剪。
  原生宿主创建时补上该样式，并保留到父窗口销毁，避免同一父窗口的多个
  宿主互相还原样式。离线真实控件测试检查父窗口确实已启用该标志。

## 父窗口裁剪修复的实机验证（Windows x64，2026-11-02）

- 使用上述固定 fork revision 重新构建 `target/debug/nyaterm.exe`。
  computer-use 已观察到启动恢复的真实远程 Windows 桌面，持续协商占位
  不再遮挡画面；修复前同一连接已到达 `LoginComplete`，由此区分登录
  状态与本地合成故障。未将连接信息或远程桌面内容保存到仓库。
- 远程键盘 `Ctrl+Escape` 打开开始菜单，`Escape` 关闭；本地文件菜单
  正常显示在远程桌面上方。窗口还原后远程分辨率跟随视口调整，再次
  最大化后恢复布局。
- 关闭连接页签后原生画面正常释放；从同一保存项重新连接后再次显示
  远程桌面。启动恢复和手动重新打开均已验证；尚未模拟断网自动重连。
- 真实控件离线初始化、策略读回、父窗口裁剪和回环事件测试 13 项通过。
  当前 revision 的 workspace check、fmt 和 diff 检查通过。
- 后续用户确认鼠标点击无效，不能将画面恢复视为完整交互验收；证书提示的继续/取消和记忆行为、
  剪贴板双向不传输、跨 DPI、长时间循环与 ARM64 实机仍未验收。

## 原生浮层鼠标修正（Windows x64）

- 全窗口 owned popup 的 `HTTRANSPARENT` 未使实际鼠标消息到达 ActiveX。
  将浮层拆为带 `WS_EX_TRANSPARENT` 的视觉 HWND，以及仅覆盖交互区域的
  输入 HWND。输入区域按当前 DPI 缩放后通过 `SetWindowRgn` 替换；空区域
  隐藏输入窗口，被动提示继续由视觉窗口显示，不向远端重放鼠标消息。
- 在 Zed `e33b65da` / Kit `8c068ea2` 的实机验证中，远端开始按钮、
  空白点击和右键菜单均响应，本地菜单仍能显示；同时发现点击远端未关闭
  本地菜单。原 UI 线程 `WH_MOUSE` 钩子无法观察 ActiveX 私有线程输入，
  改为在安装 UI 线程回调的低层钩子，并仅通知注册 owner 的原生子窗口
  点击；原始鼠标消息继续传递。
- 当前固定版本：Zed `fa96acbd51d347ba7a4fff2053036a1559ef9387`、
  Kit `e87ad22ca5590ed1c30e52113cfb09b1b87c6a77`，均为 `nyaterm` 分支。
  Windows fork 测试 57 项通过、0 失败、1 项忽略，包括输入区域缩放、
  替换、清空和父窗口销毁后的两个浮层 HWND 清理。

## 最终鼠标交互实机验收（Windows x64，2026-11-03）

- 使用上述最终固定 revision 构建的 `target/debug/nyaterm.exe`，启动恢复后
  显示真实远程桌面。鼠标点击远端开始按钮能打开菜单，右键能打开远端
  桌面上下文菜单；键盘 Escape 能关闭开始菜单。
- 本地文件菜单正常显示在远端画面上方。菜单打开时，一次点击远端开始
  按钮会关闭本地菜单并打开远端开始菜单，没有重复点击或消息重放。
  被动连接提示显示时，鼠标仍能操作远端。
- 退出确认弹窗及遮罩显示正常。点击遮罩覆盖的远端开始位置只取消本地
  弹窗，未打开远端开始菜单，确认模态输入不会穿透。
- 最大化后远端分辨率跟随视口，稳定后画面填满视口。关闭连接页签后
  原生画面释放，从同一保存项重新连接能恢复画面，鼠标开始按钮仍响应。
- 上述验证未将连接信息或远端桌面内容保存到仓库。尚未验证证书警告的
  继续/取消及记忆行为、模拟断网自动重连、剪贴板双向不传输、跨 DPI、
  长时间循环与 ARM64；离线策略读回或编译通过不代表这些项目已验收。

## 最终固定版本自动化验证（Windows x64，2026-11-03）

- Zed `fa96acbd51d347ba7a4fff2053036a1559ef9387` 和 Kit
  `e87ad22ca5590ed1c30e52113cfb09b1b87c6a77` 已提交并推送到各自 fork
  的 `nyaterm` 分支，主仓库 manifest 和 lock 同步引用这两个 revision。
- `cargo build -p nyaterm-app --bin nyaterm --locked -j1` 通过。
- `cargo check --workspace --locked -j1` 通过。
- `cargo test --workspace --locked -j1`：5048 项通过、0 失败、42 项忽略，
  包含原生 RDP 的 13 项回归测试。
- `cargo clippy --workspace --all-targets --locked -j1 -- -D warnings` 通过。
- `cargo fmt --all -- --check` 和 `git diff --check` 通过。
- 主仓库改动保持未提交；未修改 `temp/`。


## 剪贴板默认全开与光标归属修复（Windows x64，2026-11-03）

- 当前 Zed revision 为 `597ca983eb6facf1a8528a56204aeeb7c857eaf6`，
  Kit revision 为 `7952df45d0f719525fe969b8991b39f7692f7f58`；均已推送到
  `nyakang` fork 的 `nyaterm` 分支。主项目同步 manifest 和 lock，未修改
  `temp/` 或 Cargo 缓存中的依赖源码。
- 删除两个 RDP 后端的剪贴板选择器及旧禁用校验；磁盘、打印机和设备
  重定向策略保持原样。Builtin 的完整能力仍限于文本和文件／目录，
  本次未增加图片、HTML 或其他富文本协议。Native 交由微软控件协商格式。
- 旧连接和编辑草稿的剪贴板值继续往返保存，临时启动配置统一全开；
  没有数据库迁移或批量重写。helper 入口始终注册支持文件的 CLIPRDR，
  Native 入口始终启用 RedirectClipboard。
- 静态分析发现父窗口 WM_SETCURSOR 及异步样式／隐藏恢复路径可覆盖
  原生子窗口光标。修复保留消息来源 HWND，并在原生子树拥有光标时跳过
  SetCursor；真实 HWND 测试覆盖嵌套子窗口、父窗口、owned popup 和隐藏
  光标。视口去重和动态分辨率防抖保持原样，尚无实机会话日志证明存在
  重复缩放，也没有据此更改分辨率调用链。
- 相关四个 crate 测试共 775 项通过、0 失败、1 项忽略；包含旧值兼容、
  两个后端启动策略、Builtin 文件能力和取消／错误回归，以及 Windows
  真实控件离线重定向策略读回。Zed Windows 测试 58 项通过、0 失败、
  1 项忽略。这些离线结果不代表真实远程会话已验收。
- 本次 computer-use 原生管道不可用（os error 2），浏览器宿主也不可用。
  因此尚未完成新版真实会话的双向文本、文件／目录复制、光标形状和大小、
  停留与移动、菜单开关、切换页签、缩放及跨 DPI 验收；无法确认用户报告
  的闪变已完全消失。其他平台文件剪贴板和 ARM64 本次也未实测。

- 最终依赖下 `cargo check --workspace -j 2`、
  `cargo fmt --all -- --check`、`git diff --check` 通过；锁文件中 GPUI 和
  GPUI Platform 各只有一个来源。首轮默认并行的 workspace test 在桌面
  套件遇到四项失败：AI 模拟服务管道关闭、安全列表排序超时、壁纸加载
  超时、凭据回调未完成。当时另有 workspace 构建并行运行；本次 RDP
  启动／重连及旧配置兼容回归均通过。后续串行复跑结果另列，不将首轮
  失败隐藏，也不据此修改无关模块。

- 中间复跑发现共享 target 中的 core 测试产物缺少新增兼容回归，该轮已
  中止且不计入最终结果。重新编译改动涉及的 crate 后，最终日志确认
  core 兼容、两个后端启动／重连、helper 文件能力及 Native 策略读回
  回归全部实际执行并通过。
- 最终 `cargo test --workspace --locked -j 2 -- --test-threads=1` 通过：
  5053 项通过、0 失败、42 项忽略；首轮失败的四项均在这轮通过。
- 最终 `cargo check --workspace --locked -j 1` 通过。
- `cargo build -p nyaterm-app -p nyaterm-rdp-helper -p nyaterm-vnc-helper
  --locked -j 1` 通过，三个最新可执行文件均位于 `target/debug/`：
  `nyaterm.exe`、`nyaterm-rdp-helper.exe`、`nyaterm-vnc-helper.exe`。
- 最终 `cargo fmt --all -- --check` 和 `git diff --check` 通过。
- 最终 `cargo clippy --workspace --all-targets --locked -j 1 -- -D warnings`
  通过。两个 fork 工作区干净；主仓库保留未提交改动以供审阅。
- 上述结果均为构建、单元／集成或离线真实控件验证；本次真实远程会话
  验收仍受桌面控制服务不可用阻塞，不能据此确认用户报告的光标闪变已
  完全消除，双向复制和跨 DPI 等待实机补验。
