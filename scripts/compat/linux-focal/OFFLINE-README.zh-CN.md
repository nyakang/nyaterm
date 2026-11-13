# Ubuntu 20.04 定制环境离线版

此包在 Ubuntu 20.04 x86-64 原生构建，主程序与 MCP/RDP/VNC/Android 辅助程序、ADB 和 FFmpeg 都经过
GLIBC ≤ 2.31、GLIBCXX ≤ 3.4.28、CXXABI ≤ 1.3.12 检查，以及 focal 上的
`ldd -r` 检查。附带独立的 OpenGL 软件渲染库及其所需 C++ 库，所有附带库也检查
GLIBC ≤ 2.31。不替换系统 glibc、动态加载器、GTK 或显卡驱动。

## 使用

1. 在摆渡前和摆渡后，在压缩包所在目录执行 `sha256sum -c *.tar.gz.sha256`。
2. 解压到当前用户可写目录，例如 `~/apps/`。无需 root、apt、Rust、Python、FUSE
   或联网下载。保留主程序、四个辅助程序、`adb/` 和 `android-libs/` 的相对位置。
3. 从目标电脑的 X11 桌面终端进入解压目录，执行 `bash diagnose.sh`。
4. 执行 **`./start-nyaterm.sh`**，默认使用系统图形栈，优先选择可用的硬件 GPU。
   首次启动会在包内生成 `data/`，包含设置和凭据，迁移前请备份。
   更新兼容包时关闭应用，仅替换程序文件，保留原有 `data/`。

此构建使用项目原有配置/备份格式。便携版数据与已安装版数据隔离，导入已有配置时
使用应用支持的备份/恢复功能。不要把凭据或 `data/` 连同程序重新分发。

新构建的 deb 使用独立的 `~/.nyaterm-compat/`，预览版使用
`~/.nyaterm-compat-preview/`；缓存、日志、单实例锁和 MCP 发现文件也隔离。
旧兼容 deb 曾与标准版共享目录，升级后首次启动新目录为空。请关闭两种版本，
从旧版导出备份，再通过新版备份/恢复界面导入。程序不会自动复制或修改旧目录。
不要用软链接共享配置，也不要复制正在使用的数据库；只恢复目标版本支持的备份。
便携版仍使用包内 `data/`。标准版与兼容版可以各用自己的数据同时运行。

本构建已在编译时禁用标准发行渠道的更新检查和下载。升级请手动安装同一兼容
构建流程产生的新包，避免误装要求更高 glibc 的标准 Linux 包。

## 图形与功能边界

需要可用的 X11。默认启动不注入包内 Mesa、不强制 CPU，也不禁用 Vulkan，沿用系统
驱动与用户显式设置的图形环境变量。实际是否硬件加速取决于系统驱动；如果系统环境
本身设置了 `LIBGL_ALWAYS_SOFTWARE=1` 等强制软件选项，默认启动会保留该设置。

如驱动初始化失败或需要离线 CPU 回退，可选择：

```sh
./start-nyaterm.sh --software
# 或
NYATERM_RENDERER=software ./start-nyaterm.sh
```

安装 deb 后，普通入口 `nyaterm-compat` 默认走 GPU／系统驱动路径；CPU 回退使用
`nyaterm-compat --software`，或桌面入口右键菜单中的“使用 CPU 软件渲染启动”。
`NYATERM_RENDERER=gpu` 或首个参数 `--gpu` 可显式选择系统路径；首个命令行选择优先于环境变量。
程序报错后如需 CPU 回退，请用上述入口重新启动，不会因普通退出码自动重启应用。

只有软件模式会加载包内 Mesa／llvmpipe，强制 CPU OpenGL 并禁用 Vulkan，无需 NVIDIA
专有驱动。该回退路径仍保留 glibc 2.31 兼容性；软件渲染会增加 CPU 占用，尤其在大量
画面更新时。附带库只作用于应用进程及其子进程，不需要修改系统库或显卡驱动。
软件模式会把整个私有库目录放入 `LD_LIBRARY_PATH`，包括 C++、X11 和 XCB 库，
本地 shell、RDP/VNC 辅助进程及其他子进程也会继承，可能影响非图形功能。
请分别验证系统/GPU 和软件模式下的本地命令、SSH/SFTP 传输校验、剪贴板、中文输入、
RDP/VNC 连接及退出；现有 Xvfb 冒烟结果不代表这些真机功能均已通过。
不要混用从其他发行版复制的 NVIDIA `.so`，也不要通过升级系统 libc 解决驱动错误。

本行为适用于本次源码生成的新包；已发布的 `compat-glibc231-2.0.1` 附件保持原样。
旧便携包可直接执行 `./nyaterm` 使用系统驱动。要更新默认入口，先关闭应用、备份旧脚本，
仅替换本次的 `start-nyaterm.sh` 并设置 0755，保留原有二进制、`opengl/` 和 `data/`。
已安装版对应脚本位于 `/opt/nyaterm-compat/start-nyaterm.sh`；新的桌面回退菜单随新 deb 安装。

`diagnose.sh` 会在相应工具已安装时显示驱动、OpenGL 和 Vulkan 信息；缺少诊断工具
不等于驱动不可用。若无图形驱动或发生渲染错误，请保留脱敏后的启动日志及驱动版本。

ABI 与软件渲染检查不等于目标真机全部功能验收。仍需在目标内核 5.4 上验证：
窗口绘制、中文输入、复制粘贴、本地 shell、内网 SSH/SFTP、RDP/VNC 连接和辅助进程退出。
测试时使用测试账号与可丢弃数据。

`opengl/licenses/` 保留第三方许可证、GCC 运行时例外和源代码构建配方；
`opengl/packages.json` 及 `opengl/opengl-lock.txt` 记录来源和 SHA-256。
`opengl/egl_desktop.c` 是应用私有 EGL 配置适配层源码，仅在运行库提供桌面 OpenGL
而无 OpenGL ES 时转换配置过滤条件，不改变系统库或伪造 GPU 功能。

隔离内网中 AI、在线翻译、外部云同步、OAuth 和在线更新不可用，除非配置了可达的
内部服务。这些不是 glibc 故障。不要通过在线更新替换为普通 Linux 发布包；普通包
的 glibc 基线可能更高。使用同一兼容构建流程产生的新离线包升级。
