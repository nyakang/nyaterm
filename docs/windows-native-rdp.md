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

Connected、登录完成、重连和绘制子树就绪分开处理；完成登录并确认子树
有效后才显示。切页签只改变可见性。FitWindow 使用测量后的物理尺寸和
DPI，动态更新防抖 400 ms，每个尺寸最多尝试四次；确认接口不支持后才
启用 SmartSizing。固定分辨率保持配置尺寸。

## 浮层与输入

GPUI fork 将原窗口同一组件树中的浮层绘制到主窗口拥有的透明 Win32
overlay HWND。它不是全局置顶窗口，也不拥有独立业务状态。与主窗口
共用 Direct3D 设备、纹理图集和渲染管线，设备重建后重建合成资源。

`nyaterm-ui` 提供区域、被动和模态三种呈现封装。菜单控件区域接收
输入；定位用的整窗壳、阴影和 tooltip 不扩大输入区域；模态层阻断整个
客户区。菜单外原始按下先通知关闭菜单，再交由原命中目标处理。原生子
窗口的原始输入通过线程鼠标 hook 观察，不重放、不再分发 GPUI bubble。
原生键鼠、IME 和 Windows 快捷键继续由 ActiveX 处理，Canvas 全局键盘
捕获在 Native 会话上禁用。Ctrl+Alt+Home 返回本地焦点。

合成初始化或运行失败会使 Native 会话显示明确错误。失败界面提供手动
切换 Builtin；仅改变当前会话，不修改保存的连接。系统认证授权、服务器
身份验证和剪贴板/资源授权继续执行，不能通过自动降级绕过。

## 认证与剪贴板策略

选择 Windows 后端即使用系统证书验证，验证失败拒绝连接。旧数据中缺失的
`windows_authentication` 在启动配置中补为 `system`，未知值仍拒绝使用。
保存的 Builtin 证书策略保持不变，不转换为系统信任授权。

当前 ActiveX 剪贴板固定禁用。编辑器禁用控件，启动/恢复/重连统一覆盖旧的
剪贴板请求，原生宿主再次校验并将 `RedirectClipboard` 固定关闭。归一化
不批量改写保存数据；用户保存编辑时才保存禁用值。不开放“仅文本”或完整重定向。
本机离线回归测试创建隐藏父窗口和真实系统控件，覆盖 NLA/管理会话开关，
读回确认系统验证级别、剪贴板和动态资源策略，再销毁宿主；不调用 Connect。
该测试通过仅证明本机初始化可用，不代表服务器登录或实际会话验收通过。

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

真实会话验收按用户决定由用户后续执行。测试连接使用已有保存项；不要
将密码放入命令行、日志或测试文件。独立 smoke 程序启动方式：

```powershell
cargo run -p nyaterm-windows-rdp --example native_overlay_smoke -- <保存的连接名称>
```

此前真实连接到达 Windows 证书验证界面，但服务器证书被系统拒绝。
未弱化服务器验证。该结果只说明创建/连接已到达系统验证阶段，不能
视为远程画面或交互验收通过。

| 项目 | 验收要求 |
| --- | --- |
| 登录与安全 | 真实登录、失败认证、证书拒绝、剪贴板双向不传输 |
| 输入 | 中文 IME、系统快捷键、Ctrl+Alt+Home、无双重输入 |
| 浮层 | 菜单区域命中、外部点击只执行一次、tooltip 穿透、模态阻断；远程画面持续更新 |
| 布局 | 连续切页签、分屏、最大化、跨 DPI 显示器、动态/固定分辨率 |
| 生命周期 | 断网重连、正常/强制关闭、退出、100 次循环后 HWND/句柄/内存无持续增长 |
| ARM64 实机 | 在 ARM64 Windows 上单独记录真实会话；交叉编译不能替代 |

自动化检查结果在交付说明中列明。编译、日志和 HRESULT 不能替代上表的
真实画面与交互验收。

## Fork revision

- GPUI/Zed `nyaterm`: `be61ba9ce2b8bccc4a5cabd8c1ad135a87a115d6`。
- gpui-kit `nyaterm`: `7d5c9022e4fb3ecdbec7dccb8c1743e995945d2c`。

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
- 尚未完成真实服务器登录、证书拒绝、断网重连、剪贴板双向不传输及 ARM64 实机验收。
