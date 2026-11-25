# 独立原生组件

Android、Builtin RDP、VNC、SPICE 的实现、解码器依赖、资源、测试和发布工作区位于
`nyakang/nyaterm-plugins` 的 `native/`。本地检出位置是 `temp/nyaterm-plugins`。
Windows Native RDP 继续留在主仓库。

主仓库保留会话管理、UI、ADB 文件/终端接口和持久化模型，通过 Git 精确 revision
引用 `nyaterm-native-protocol`。RDP、VNC、SPICE、Android 分别维护 IPC 版本；
helper 的 SemVer 不再与应用版本相等。现有连接、设置、凭据、同步和备份格式不变。

## 安装与更新

插件页可安装、检查更新、手动更新、回退到上一版本、禁用和卸载。
检查更新只读取签名索引，不自动替换组件。应用升级后继续使用兼容版本。
兼容性同时检查宿主 SemVer 范围、IPC、target 和运行时基线。
Linux glibc 2.31 和 glibc 2.35 分开发布；Windows 使用 windows-msvc。

索引位于 `downloads.nyaterm.app/native/v1/index.json`，是含原始 index 字符串和
Minisign signature 的 JSON 信封。先验签再选择版本。下载进一步校验 ZIP 大小、
SHA-256、签名 manifest 和完整文件清单。构建提交只用于追溯，不参与宿主版本相等判断。

组件存储于 `plugins/native/<id>/<target>-<runtime>/<version>-<manifest-sha256>`。
redb 中独立的选择表保存 active 和 previous，原有启停偏好表保持不变。
会话租约锁住具体版本；更新不终止旧会话，卸载等待租约释放。
损坏修复使用 staging journal，启动时恢复中断的目录替换。

## 开发与发布

在独立仓库运行 `cargo test --manifest-path native/Cargo.toml`；Android 需先准备
FFmpeg 并设置 FFMPEG_DIR。完整命令见其 `native/README.md`。
宿主集成测试位于 `crates/nyaterm-remote-desktop/tests`，设置对应 helper 环境变量后
显式运行 ignored 测试。开发覆盖路径仍受 IPC 握手验证。

应用发布不再编译或携带四个 helper；MCP 仍是应用基础 helper。
组件发布工作流针对一个组件构建七个平台/运行时组合，经受保护环境签名后先上传
不可覆盖资产，再以 ETag 条件写入发布索引。需要独立仓库配置 R2 和签名 secrets。
本次代码变更不执行线上发布，也不代表真实设备/远程服务器验收。

## 本次验证（2026-11-25，Windows x64）

- 主工作区全量测试：7,527 通过，102 按条件忽略；RUST_MIN_STACK=16777216。
- 最后修改的定向测试：IPC 模型转换 2、Android 48、组件宿主 30、选择表存储 3，全部通过。
- 跨仓库实际 helper 集成：RDP 10、VNC 生命周期 13、VNC manager 8、SPICE 9，全部通过。
  SPICE 使用本机已准备的原生运行时；这些测试不连接真实远程桌面。
- 独立 helper 工作区：195 通过，6 按条件忽略；启用 Android test-support 验证合成视频输入。
- 主仓库 Python 发布测试：281 项运行，22 跳过；全部通过。
- 独立仓库 Python 准备/打包/发布测试：48 项（其中新增架构测试单独补跑），全部通过。
- 两个工作区的 cargo check / clippy / fmt 检查通过。

独立仓库分支为 feat/native-components，完整迁移提交为 3b8f0ef，后续 CI 修正已推送至 ffd1dce。
应用固定 IPC 提交 0343eb97ed94e87761978ee9733314925b783b5a；后续 helper 实现提交
未修改该协议。现有 GPUI 依赖 revision 更新保留，未包含在插件仓库提交内。

尚未执行线上 R2 发布、Linux/macOS 运行测试或真实 Android/远程桌面验收。
发布环境需配置签名和 R2 secrets，并在 main 上运行受保护的独立发布工作流。
