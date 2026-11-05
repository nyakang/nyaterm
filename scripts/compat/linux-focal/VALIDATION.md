# Compatibility validation (2026-10-29)

Current source policy after the published 2.0.1 build: prefer the system GPU
renderer by default; select the bundled CPU path with `--software` or
`NYATERM_RENDERER=software`. The dated sections below retain the original CPU
default's historical results. See the final GPU-default section for the new
launcher's tests and hardware-validation boundary. The latest upstream sync and
release validation is recorded in the 2026-11-04 section. The 2026-11-05 review
follow-up below records lighter checks only, without a new release build.

Target from the supplied environment guide: customized Ubuntu 20.04, x86-64,
glibc 2.31, GTK 3.24.23, GLib 2.64.6, Linux 5.4, isolated network; GT 1010
(GP108), proprietary driver not confirmed. The offline default uses CPU OpenGL
and does not depend on Vulkan or that proprietary driver.

## Verified

| Check | Result |
| --- | --- |
| Final builder | Ubuntu 20.04, glibc 2.31, GTK 3.24.20, GLib 2.64.6, Rust 1.99.1, ordinary UID 1000 |
| Frozen release build | All four default-member executables built successfully |
| Main executable + three helpers | Highest required GLIBC version **2.30**; system interpreter; no RPATH; focal `ldd -r` resolved |
| Private OpenGL runtime | **83 ELF libraries** audited; highest required GLIBC version **2.17**; required C++ libraries included; focal `ldd -r` resolved |
| Runtime reproducibility | Full checksum-pinned preparation script executed from an empty project-local download cache |
| Actual archive relocation | Extracted into a new temporary directory and launched using its own `start-nyaterm.sh` |
| Graphics on focal | **Passed**: OpenGL/llvmpipe, Vulkan ICDs disabled, no NVIDIA device/driver; 1280×800 NyaTerm window and workbench screenshot inspected |
| RDP/VNC lifecycle tests | **22 passed**, no skips or failures, release-profile binaries on focal |
| ABI, EGL adapter and packaging/release Python tests | **65 passed**, **1 skipped** (Windows NSIS unavailable), no failures |
| Shell/workflow validation | `bash -n`, `git diff --check`, actionlint 1.7.7 passed |
| Archive integrity | SHA-256 verification passed |

Evidence produced by the current run:

- `target/linux-focal/smoke/application.log` explicitly records `backend=Gl`,
  `type=Cpu` and `Selected GPU ... llvmpipe (LLVM 23.1.4, 256 bits) (Gl)`.
- `target/linux-focal/smoke/workbench.xwd` and `window-tree.txt` record the actual
  final archive's rendered window.
- The archive contains `abi-report.json`, individual library hashes,
  `opengl/packages.json`, the SHA-256 package lock, the EGL adapter source and
  third-party notices/recipes.

## Diagnosed failures that led to the default OpenGL path

The ABI-only build initially failed graphics startup with stock focal Mesa 21:
lavapipe exposed two color attachments while GPUI requested four; the GL path
fell back to surfaceless EGL because the old driver lacked the XCB platform
extension. These were not libc failures. They are resolved for the default
launcher by the private Mesa/llvmpipe runtime.

The newer conda-forge Mesa runtime enables desktop OpenGL, not GLES. The pinned
wgpu config selector requests an ES2 config before creating a desktop context.
The application-private EGL adapter corrects that filter only for a desktop-only
driver. Unit tests verify GLES-capable drivers retain the original filter, and
the final archive's real OpenGL surface/device and rendered frame were checked.
No glibc symbol tags, GPU limits, third-party Cargo sources or system files were
modified to make the checks pass.

## Remaining target-machine acceptance

The test containers share host kernel **6.18.55**. They do not emulate Linux 5.4
or the downstream security patches. Target-machine testing is still required
for its kernel/JIT policy, Chinese input, clipboard, local shell, actual internal
SSH/SFTP/RDP/VNC services and sustained software-renderer performance. No claim
is made that every feature is fully certified on that unavailable machine.

Network-dependent AI, external OAuth, public cloud sync, online translation and
online updates cannot work on an isolated network without reachable internal
services. Use the same offline compatibility build to upgrade, retaining `data/`.
Hardware-accelerated system-driver mode is optional and remains unverified on
the target GT 1010; the default software OpenGL mode avoids that dependency.

## Upstream sync and icon packaging revalidation (2026-11-02)

Merged upstream `main` through `4671cc964de45063a5c5541178cf284c7e251d12`
without rewriting the compatibility branch. The merged 2.0.1 code built in the
pinned focal/Rust 1.99.1 image. New validation artifacts were written under
`target/validation-20261102/artifacts`; the existing 2.0.0 tarball, deb and their
checksum files in `dist` were checked against their original SHA-256 values and
were unchanged. No Release, tag or attachment was published or modified.

| Check | Result |
| --- | --- |
| Merged-source frozen optimized build | Passed on Ubuntu 20.04 / glibc 2.31, x86-64 |
| Application, MCP, RDP and VNC executables | Highest required GLIBC **2.30** for each; focal ABI/SONAME/interpreter/RPATH and `ldd -r` gates passed |
| Private OpenGL runtime | **83 ELF libraries** audited, highest required GLIBC **2.17**; used the existing checksum-pinned runtime cache |
| Portable archive | Extracted into a new directory; rendered a 1280×800 workbench under Xvfb using llvmpipe, `backend=Gl`, `type=Cpu`; Vulkan ICDs disabled |
| Real compatibility deb install | `dpkg -i` succeeded in a separate disposable focal container; `/usr/bin/nyaterm-compat` rendered a workbench as ordinary UID 1000 |
| Installed icon lookup and decoding | Ordinary-user GTK icon-theme lookup and GdkPixbuf decoding passed at **128, 256 and 512 pixels**; root:root / 0644 |
| Install repair | Intentionally changed an installed icon to 0600; repeated `postinst configure` restored 0644 successfully |
| X11 identity | Real application window reported `("nyaterm" "NyaTerm")`; native adapter regression tested stable and preview class/instance pairs against an Xvfb server |
| Helper lifecycle regression | **22 passed**, release-profile RDP/VNC helpers on focal |
| Python scripts | **122 tests**, 114 passed and 8 platform/tool skips; focal Python 3.8 ABI/package suite **9 passed** |
| Packaging regressions | Real dpkg archives checked for root ownership and readable icons, including restrictive source permissions, repeated configure, missing sizes, symlinks and unrelated-file preservation |
| Source checks | Formatting, architecture boundaries, shell syntax, diff whitespace and actionlint 1.7.7 passed |

Evidence remains in `target/validation-20261102/`: `build-final.log`,
`abi-summary.json`, `package.log`, `native-x11-test.log`, `lifecycle.log`,
`deb-install.log`, `smoke/` and `deb-smoke/`. Each new artifact also has a SHA-256
file and embeds the full ABI report and third-party notices.

These checks validate the packaged files and X11/OpenGL behavior in focal
containers. They do not certify icon presentation in the unavailable internal
machine's actual desktop shell, its Linux 5.4 kernel/custom security patches,
real driver or internal services. The target-machine acceptance requirements
above still apply.

### release 测试配置试跑记录（后续已完成标准调试复核）

- `cargo check --release --workspace --locked` 通过。
- 严格 `cargo clippy --release --workspace --all-targets --locked -- -D warnings`
  通过（7 分 19 秒）。
- 首轮全优化工作区测试在编译 `nyaterm-desktop` 测试目标时触及容器的
  16 GiB 内存上限；cgroup 记录 `oom_kill=1`，编译器收到 SIGKILL。
- 后续采用 `-j1`，并仅将测试构建中 `nyaterm-desktop` 的优化级别设为 0；
  全量断言执行终态为 **4833 通过、154 失败、28 忽略**。失败集中在
  `nyaterm-desktop --lib`（128 项）和 `nyaterm-ui --lib`（26 项），不能将
  该次 release 配置试跑标记为通过。
- 已核对锁定的 GPUI 源码：`VisualTestContext::debug_bounds()` 明确说明，
  未启用 `debug_assertions` 的构建不会记录调试选择器，因而返回 `None`。
  失败日志中的界面测试依赖这些边界；本次使用 release 测试配置不满足该
  测试前提；下节给出代表性对照和完整标准调试复核的实测闭环。
- 从现有完整工作区测试二进制直接在 Xvfb 下运行新增的原生 X11 类名测试，
  **1 项通过**，没有重新编译依赖。应用优化构建、实际 tar/deb 软件 OpenGL
  窗口、普通用户 GTK 图标读取、22 项助手生命周期测试及 ABI 审计均已独立通过。

终态证据：`clippy.log`、`workspace-tests.log`、`workspace-tests-retry.log`、
`rust-workspace-summary.json`、`memory-events-after-test-kill.txt` 和
`x11-cached-binary.log`。本地标准调试测试已在下节另行完整复核；GitHub PR
工作流仍须上游维护者批准，未自动批准或手动重复重跑工作流。


### 标准调试配置复核与归因闭环（2026-11-02）

正式 Linux CI 在 `.github/workflows/rust.yml` 中执行
`cargo test --workspace --locked --no-fail-fast`，没有 `--release`。
本轮在同一 Ubuntu 20.04／Rust 1.99.1 环境，以该命令复核；复用下载缓存，
一次构建完整测试二进制后运行代表用例和汇总。仅通过
`CARGO_PROFILE_DEV_DEBUG=0`、`CARGO_PROFILE_TEST_DEBUG=0` 关闭调试符号，
以控制磁盘占用，没有关闭调试断言或改变仓库的优化级别设置。
Cargo 编译元数据确认 GPUI 为 `opt_level=1`、`debug_assertions=true`、
`overflow_checks=true`、`test-support` 启用；UI／桌面测试同样保留调试断言。

| 复核内容 | 实际结果 |
| --- | --- |
| 弹窗层、密码输入、原生审批交互、资产菜单四项同名代表用例 | 原 release 二进制全部以 101 失败；标准调试二进制全部以 0 通过 |
| `nyaterm-desktop --lib` 完整套件 | **2485 通过、0 失败、13 忽略** |
| `nyaterm-ui --lib` 完整套件 | **145 通过、0 失败** |
| 此前失败名称逐项对照 | 原先 **154／154** 项均在标准调试汇总日志中找到同名通过记录 |
| 完整工作区标准调试汇总 | **4987 通过、0 失败、28 忽略**，退出码 0 |
| 新增原生 X11 类名测试 | 从现有调试测试二进制直接在 Xvfb 下执行，**1 通过**，退出码 0 |

这使此前 154 项界面失败的归因同时有源码前提、编译配置、代表用例前后
对照及全部同名用例的实际通过记录支撑；没有通过改动断言、增加忽略项或
修改无关生产代码取得通过。

完整标准调试汇总的首轮还发现一项额外的 Fish 测试失败（4986 通过、1 失败、
28 忽略），堆栈明确显示 `fish: Unknown command: less`。本地最小镜像安装
zsh／fish 时省略了推荐包，缺少 Fish history 使用的 pager。已验证：

- 对比合入的上游 `4671cc964de45063a5c5541178cf284c7e251d12`，该测试文件、
  transport 的全部本地依赖闭包（core、mcp-protocol、plugin-api、terminal、
  transport）、根 Cargo 清单与锁文件均零差异。
- 同一个已编译的 transport 测试二进制，缺少 `less` 时该项失败；仅安装
  `less` 后该项通过，二进制 SHA-256 保持不变，没有重新编译或修改测试。
- 补齐 `zsh fish less` 后，再执行完整标准命令；构建直接复用缓存（2.36 秒），
  最终得到上述 **4987／0／28** 结果。最小 focal 镜像进行完整测试时应包含
  这三个系统工具；官方 CI 的完整 runner 环境与只用于打包的最小镜像不同。

新增证据位于 `target/validation-debug-20261102/`：
`release-representatives.json`、`debug-representatives.json`、
`ui-compiler-profiles.json`、`workspace-tests-with-less.log`、`final-summary.json`、
`transport-dependency-closure.json`、`fish-without-less.log`、`fish-with-less.log`、
`fish-binary-unchanged.txt` 和 `x11-test-with-less.log`。
功能源码在本轮复核期间未改变；本次补充只更新验证记录。内网目标机的
真实桌面壳层、Linux 5.4／安全补丁、驱动和服务仍按前述要求验收。


## 再次同步与 2.0.1 兼容发行验证（2026-11-03）

本轮从兼容分支 `873422e716b89d03d455b17ab6bb93d7728e705a` 开始，普通 merge
合入启动时实际抓取的上游 `e855789e2269841a4e84c9ffb78c2808c30cc793`（11 个新增提交），
合并提交为 `e5fb1f083c380e76043b0b13e411cea34ed4584b`。唯一实际冲突在
`scripts/tests/test_package_native.py`：同时保留本分支真实 deb 图标／权限测试和
上游 Windows 静态 CRT 测试。GPUI `fa96acbd`、gpui-kit `e87ad22c`、Windows RDP
新架构和上游备份修复均保留。兼容补丁没有额外改动持久化格式。

`4c343583e382db09472c5bee6f3dd7729d62f7c4` 另补齐便携 tar 的目录 0755、
可执行文件 0755、数据 0644 及 root 归档元数据，避免继承构建者身份或受限权限。
实际归档回归覆盖 0600 源文件与 umask 0077。

本轮构建期间，上游继续推进 9 个提交至 `84015d4542b1a2fef98cbe182f2c0ee1b3e4d087`，
包含 GPUI／gpui-kit 0.9 等大幅依赖更新。本轮固定上述启动时基线，不将这些后来更新
计入发行或沿用本轮结果宣称其兼容。当前兼容分支与该新 tip 的 `git merge-tree --write-tree`
预演退出 0、无冲突；预演没有改变工作区、提交或正在执行的构建。

| 本轮新树检查 | 实际结果 |
| --- | --- |
| 标准调试工作区测试 | `RUST_TEST_THREADS=2 cargo test --workspace --locked --no-fail-fast`：**4995 通过、0 失败、28 项既有忽略**，退出 0 |
| 测试配置 | Cargo 元数据确认 desktop／ui 的 `debug_assertions=true`、`overflow_checks=true`；仅设 `CARGO_PROFILE_DEV_DEBUG=0`、`CARGO_PROFILE_TEST_DEBUG=0` 关闭调试符号，保留仓库优化配置 |
| 严格 Clippy | `cargo clippy --workspace --all-targets --locked -- -D warnings` 通过（标准调试配置） |
| 源码检查 | `cargo fmt --all -- --check`、架构边界、Shell 语法、actionlint 1.7.7 和相对固定上游基线的兼容差异空白检查通过 |
| Python | 151 项：143 通过、8 项平台／工具跳过；focal Python 3.8 专项 9 项通过 |
| 原生 X11 类名 | 缓存的标准调试二进制另行执行 ignored 类名回归：1 项通过，覆盖稳定／预览类名与实例组合 |

默认并发首轮完整汇总为 4994 通过、1 失败、28 忽略。唯一失败是
`remote_file::shell::transfer::tests::pause_does_not_consume_idle_budget_and_upload_activity_keeps_reply_wait_alive`：
该用例使用 80ms 空闲预算，在 oneshot sender 处因 receiver 已释放而失败。
同一个缓存二进制单独连续复测 5 次全部通过，SHA-256 不变，且在上述两线程完整汇总中通过。
该模块及 transport 的全部本地依赖闭包、根 Cargo 清单与锁文件，与本轮固定上游的 Git
对象逐项相同。保留首轮失败日志，不将首次失败隐去，也没有修改断言、超时或忽略项。

| 最终发行制品检查 | 实际结果 |
| --- | --- |
| 锁定依赖优化构建 | 四个默认发行可执行文件在 focal 普通 UID 1000 下构建通过，用时 51m35s；生成本轮独立目录的新 tar／deb |
| 可执行文件 ABI | 主程序、MCP、RDP、VNC 最高要求均为 **GLIBC 2.30**；解释器、SONAME、RPATH、GLIBCXX／CXXABI 与 focal `ldd -r` 审计通过 |
| 私有 OpenGL | **83 个 ELF 库**审计通过，最高要求 **GLIBC 2.17**；使用原校验和锁定的运行库缓存并重新执行整理／EGL 适配层构建 |
| 真实 tar | 新目录重新解压并以普通用户启动；绘制 1280×800 工作台，截图已检查；日志确认 llvmpipe／backend=Gl／type=Cpu，Vulkan ICD 禁用 |
| 真实 deb | 独立一次性 focal 容器 `dpkg -i` 安装，普通 UID 1000 执行 `/usr/bin/nyaterm-compat` 绘制工作台，截图已检查 |
| 实际窗口类名 | tar 和安装版均为 `("nyaterm", "NyaTerm")`，与 desktop 的 StartupWMClass 一致 |
| 归档权限与图标 | tar 的 1339 个条目、deb 的 1363 个条目均为 root 元数据；目录 0755，常规数据 0644、可执行文件 0755；五种尺寸图标与仓库原 PNG 逐字节一致 |
| 安装后图标与修复 | 普通用户 GTK 图标主题查找和 GdkPixbuf 解码 128／256／512 均通过；root:root／0644；主动改为 0600 后重复 postinst configure 两次成功修复 |
| 优化版辅助进程 | RDP／VNC 生命周期回归 **22 通过**，无失败或忽略 |
| 发布内容与完整性 | 两包及两个校验文件 SHA-256 通过；仅显式应用／运行库／许可证／诊断材料，无个人 data、凭据、日志或构建缓存 |

最终收尾提交只补充本验证文档；发布前再次锁定构建，并对照包内四个 ELF 的 SHA-256，
确认最终源码对应的输出与已测制品逐字节一致，记录见 `source-provenance.json`。


本轮编译和测试在现有 Codex 云实例的本地 Docker 中完成。构建器为 glibc 2.31、
Rust 1.99.1、普通 UID 1000，
容器共享宿主内核 **6.18.57**。旧 2.0.0 四个本地发行文件的 SHA-256 与已发布资产记录一致。

证据保存在 `target/validation-20261103/`：`standard-final-summary.json`、
`standard-debug-profiles.json`、`workspace-tests.log`（首轮）、
`workspace-tests-two-threads.log`、`timing-recheck-*.log`、
`timing-binary-unchanged.txt`、`transport-upstream-equivalence.json`、`clippy.log`、
`x11-test.log`、`builder-info.txt` 及新制品验证记录。

内网目标机的真实桌面壳层图标、Linux 5.4／定制安全策略、实际驱动、中文输入、剪贴板、
内部 SSH／SFTP／RDP／VNC 服务和软件渲染性能仍需真机验收。Windows／macOS 未在本轮
Linux 环境运行；新上游基线的依赖升级也不属于本轮已测范围。


## 默认 GPU 与显式 CPU 回退（2026-11-03）

基于已发布的兼容源码 `04477c441203cd37821fc4bae200c72c04870167` 修改启动与打包脚本。
检查时用户兼容分支及 2.0.1 tag 仍指向该提交，上游 main 已推进至 `da26ec5f`；本轮聚焦
启动方式，没有合入后来上游。应用 Rust 源码与 Cargo 清单／锁文件没有改动。

默认 `./start-nyaterm.sh` 和新 deb 的 `nyaterm-compat` 使用系统图形栈，不添加私有
Mesa 路径、不强制 llvmpipe，也不禁用 Vulkan。锁定 GPUI 的适配器排序在用户／显示服务
显式选择之后优先硬件设备，再比较图形后端；保留用户自定义的驱动环境变量。
`--software` 或 `NYATERM_RENDERER=software` 显式选择原有私有 Mesa／llvmpipe CPU
路径；首个 `--gpu`／`--software` 参数覆盖环境变量，首个 `--` 用于转发同名应用参数。
新 deb 增加“使用 CPU 软件渲染启动”桌面动作。应用参数与退出码保留，不因普通错误码
自动重新启动应用。无效模式或不完整软件运行库在启动应用前明确报错。

| 检查 | 实际结果 |
| --- | --- |
| Python 全量脚本 | 158 项：**150 通过、8 项平台／工具跳过** |
| focal Python 3.8 专项 | ABI／打包与启动器 **16 项全部通过** |
| 默认 GPU 路径 | 记录型可执行文件验证默认／空模式、无私有运行库启动、系统驱动环境保留、参数含空格、CLI 覆盖、选项转义及退出码；默认没有注入七项软件渲染环境变量 |
| 安装版入口 | 实际安装新 deb 后，以普通 UID 1000 调用 `/usr/bin/nyaterm-compat`；使用临时记录型可执行文件验证 GPU 默认及 `--software` 选择，随后恢复原 ELF 并确认其 SHA-256 未变 |
| tar 软件回退 | 实际新 tar 解压后执行 `start-nyaterm.sh --software`，普通用户在 focal Xvfb 绘制工作台，截图已检查；日志为 llvmpipe／backend=Gl／type=Cpu，实际 WM_CLASS 为 ("nyaterm", "NyaTerm") |
| deb 软件回退 | 新 deb 独立安装，普通用户执行 `nyaterm-compat --software` 绘制工作台；128／256／512 图标主题查找与解码、重复 postinst 权限修复均通过 |
| 实际包与 ABI | 四个 ELF 与已发布 2.0.1 二进制逐字节一致，重新执行 ABI 审计，各自最高 GLIBC 2.30；83 个私有库最高 GLIBC 2.17；归档 root 所有权、0755／0644 权限、五尺寸图标与 SHA-256 通过 |
| 桌面动作与源码 | 实际 deb 的 desktop-file-validate 退出 0（只有原有 Categories 提示）；Shell 语法、actionlint、架构边界和差异空白检查通过 |

本地验证包使用单独目录 `target/validation-gpu-default-20261103/artifacts/` 和测试版本
`2.0.1+gpu.1`，没有覆盖旧制品。详细证据包括 `python-all.log`、`python38-focused.log`、
`artifact-verification.json`、`installed-launcher-selection.json`、`unchanged-release-binaries.json`、
`software-smoke/`、`deb-smoke/` 与 `deb-install.log`。

当前云环境没有提供可用硬件 GPU，本轮默认路径通过启动环境／参数契约和已安装入口验证，
真实窗口验证只覆盖显式 CPU 回退。用户报告其内网设备 GPU 已可启动，但没有据此声称
本轮已在该设备验证硬件加速、性能、驱动、输入法或内部服务。

已发布的 `compat-glibc231-2.0.1` 仍保留 CPU 默认，其 Release／tag／附件 ID、大小和
SHA-256 已重新读取，保持不变。本轮只提交本地源码，不推送、发布或替换附件。
旧便携包可先直接执行 `./nyaterm` 使用系统驱动；就地部署本修改仅需关闭应用、备份并
以 0755 替换 `start-nyaterm.sh`，保留原二进制、`opengl/` 与 `data/`。安装版对应文件为
`/opt/nyaterm-compat/start-nyaterm.sh`，桌面回退动作由新 deb 提供。


## 上游再次同步与 GPU 默认兼容修订版（2026-11-04）

本轮完整恢复上一轮尚未推送的 GPU 默认提交
`9733e232a8d2351bf487c3a02c86c54a4423bebe`：重建后的 Git 对象 SHA 与原记录
完全一致，保存已验证的独立 bundle，并推送至 fork 的安全备份分支
`backup/gpu-default-9733e232a`。没有重写兼容分支历史。

普通 merge `eefd9da9857521f6e72da224d3dde34557f2cb67` 合入本轮启动时实际抓取的
上游 `a7f5382766e610f8d7943a4f767d5f820f1ced91`，相对上一发行基线新增 27 个提交。
本轮没有内容冲突；`scripts/release/README.md` 自动合并。保留上游 GPUI
`a755e55a`、gpui-kit 0.9／`9143df97`、wgpu 30、Windows RDP、WebDAV 和终端等更新，
同时保留 GPU 默认、显式 CPU 回退、glibc 2.31、图标与权限兼容方案。
已读取新锁定 GPUI 的 `sort_adapters`：用户／显示服务的显式选择之后，硬件类型
仍优先于 CPU，后端优先级在设备类型之后。

上游 workspace 仍为 2.0.1，因此使用新 tag `compat-glibc231-2.0.1-r2` 和独立打包版本
`2.0.1+compat.2`，不覆盖旧 2.0.1。`19a48f8f2db83147c1358cc40928e7b0a498f16c`
添加 `NYATERM_FOCAL_VERSION`，将兼容修订版本传给现有打包器；Debian 版本为
`2.0.1+compat.2-1+glibc231`，`dpkg --compare-versions` 确认高于旧版。
优化制品首先由该提交生成；最终文档提交后再执行相同正式构建命令并比对四个 ELF，
确认最终源码与已实际测试制品对应，文档提交没有改变构建输入。

| 本轮新树与实际制品检查 | 实测结果 |
| --- | --- |
| 标准调试工作区 | `RUST_TEST_THREADS=2 cargo test --workspace --locked --no-fail-fast`：**5070 通过、0 失败、28 项既有忽略**，51 个套件，退出 0 |
| 调试配置 | Cargo 元数据确认 `debug_assertions=true`、`overflow_checks=true`；仅用 `CARGO_PROFILE_DEV_DEBUG=0`／`CARGO_PROFILE_TEST_DEBUG=0` 去除调试符号 |
| 原生 X11 类名 | 从本轮调试二进制在 Xvfb 下另行运行 ignored 回归，**1 项通过** |
| 严格源码检查 | `cargo clippy --workspace --all-targets --locked -- -D warnings`（4m09s）、`cargo check --workspace --locked`、格式、架构边界、Shell、actionlint 1.7.7 与差异空白检查通过 |
| Python 全量 | **165 项：156 通过、9 项平台／工具跳过**，包括实际 Rust 编译的诊断模块回归；focal Python 3.8 ABI／打包／启动器专项 **16 项通过** |
| 锁定依赖优化构建 | Ubuntu 20.04、glibc 2.31、Rust 1.99.1、普通 UID 1000、`CARGO_BUILD_JOBS=2`；四个默认发行程序构建成功，用时 **13m53s**，未发生 OOM |
| 发行程序 ABI | 主程序、MCP、RDP、VNC 最高均为 **GLIBC 2.30**；解释器、SONAME、RPATH、GLIBCXX／CXXABI 和 focal `ldd -r` 门禁通过 |
| 私有 OpenGL | **83 个 ELF 库**，最高要求 **GLIBC 2.17**；复用校验和锁定下载缓存，重新执行运行库整理和 EGL 适配层构建 |
| 真实 tar 软件回退 | 新目录解压本轮归档，以普通用户运行 `start-nyaterm.sh --software`，绘制 1280×800 工作台并检查截图；llvmpipe／`backend=Gl`／`type=Cpu`，Vulkan ICD 禁用 |
| 真实 deb 软件回退 | 独立一次性 focal 容器安装，普通 UID 1000 运行 `nyaterm-compat --software` 绘制工作台并检查截图；同为软件 OpenGL |
| GPU 默认契约 | 启动器回归和真实安装入口的记录型可执行文件确认：参数原样转发、默认不注入七项软件渲染变量；随后恢复真实 ELF 并确认 SHA-256 不变 |
| 窗口身份 | 实际 tar／deb 均为 `("nyaterm", "NyaTerm")`，与 `StartupWMClass=NyaTerm` 一致；实际 desktop 通过 `desktop-file-validate`，含显式 CPU 软件渲染动作 |
| 归档、权限、图标 | tar **1339** 条目、deb **1363** 条目检查 root 元数据、目录／可执行文件 0755 与数据 0644；32／64／128／256／512 图标与原 PNG 逐字节一致 |
| 安装图标 | 普通用户 GTK 主题查找和 GdkPixbuf 解码 128／256／512 均通过，root:root／0644；主动改为 0600 后重复 `postinst configure` 两次修复成功 |
| 实际发行助手生命周期 | 直接复用本轮缓存测试二进制，调用与包内逐字节一致的正式 RDP／VNC 可执行文件，**22 通过、0 失败**；前后 SHA-256 相同 |
| 制品完整性 | tar、deb 及各自校验文件的 SHA-256、包内二进制与 ABI 报告、tar／deb 相同载荷逐项核对通过；仅打包明确发行载荷，不含个人 data、凭据、日志或构建缓存 |

制品复核首轮发现测试命令会改写共享 target 目录中的两个 helper 输出：正式发行包内
RDP／VNC 摘要分别以 `f0b7197f826c`／`6c322b30bdf2` 开头，而
`cargo test -p nyaterm-rdp-helper -p nyaterm-vnc-helper --test lifecycle` 后目录输出变为
`3e4b41ac6d3d`／`4202723e8e41`。Cargo 指纹证据确认两组同为 release、helper 的
features 均为空，但依赖图指纹不同。没有修改源码或归档；重新执行原正式构建命令，
缓存命中用时 0.77 秒，四个 ELF 全部恢复为包内哈希，之后完整制品审计通过。
再直接运行缓存生命周期套件（其 `CARGO_BIN_EXE` 指向已恢复的正式输出），取得上表的
22 项实际发行助手通过结果；完整保留首轮失败及两组产物／指纹证据，没有跳过校验。

Python 环境准备阶段曾缺少官方 `v2.0.1` tag，已抓取并核对真实上游对象后重新通过。
Windows 诊断脚本的 `tomllib` 依赖使用外层 Python 3.12 验证，focal Python 3.8 仅执行
其支持的兼容专项；诊断 Rust 回归通过已有 focal 编译器执行，转发公开构建元数据后通过，
没有修改产品代码、测试断言或增加忽略项来绕过环境问题。

| 新制品 | SHA-256 |
| --- | --- |
| `NyaTerm_2.0.1+compat.2_linux_x64_glibc231.tar.gz` | `0f02f995961051ffae7b030eb46c0b15658e696d6542b0c05e8d5141a8414a33` |
| `NyaTerm_2.0.1+compat.2_linux_x64_glibc231.deb` | `b2950b36ad192d97ad08dfdf739b412cd4a5b38e14418282fb3d5a29582001c5` |

本轮证据位于 `target/validation-20261104/`：`recovered-gpu-9733.bundle`、
`workspace-summary.json`、`workspace-tests.log`、`debug-profile.json`、`x11-test.log`、
`clippy.log`、`cargo-check.log`、`python-all-with-rust-final.log`、`python38-focused.log`、
`release-build.log`、`artifact-audit-first.log`、`helper-build-variants.json`、
`profile-restore-build.log`、`packaged-lifecycle.log`、`artifact-audit.json`、
`deb-install.log`、`installed-default-contract.json`、`software-smoke/`、`deb-smoke/`
及最终源码／制品和发布后下载核对记录。短暂连接重启后已先读取原进程日志并核对哈希，
提交和制品未丢失，没有因此重复编译或发布。

云环境没有可用硬件 GPU，本轮真实窗口仅验证显式 CPU 软件回退；GPU 默认通过环境／
参数契约与安装入口核实，不能据此宣称新 GPUI／wgpu 已在用户内网设备完成硬件加速
或性能验收。容器共享宿主内核 6.18.57；目标 Linux 5.4、定制安全策略、真实桌面壳层、
驱动、中文输入、剪贴板、内部 SSH／SFTP／RDP／VNC 服务仍需真机验收。
Windows／macOS 未在本轮 Linux 环境实测。上游 PR CI 若等待维护者批准，属于外部审批
边界，本地验证不等于上游 CI 成功；不代审批、重跑或合并 PR。

## Review follow-up (2026-11-05)

PR #1106 was updated from `ff2ebcdf975e` and merged with upstream `main` at
`2c8af4b41` (merge commit `1d5381a28`). The compatibility packaging now generates
Debian dependencies from all four executables using `dpkg-shlibdeps`, isolates
installed data by a compiled compatibility namespace, marks updates as externally
managed, and keys Mesa/patchelf caches by their respective lock hashes.

Checks after the upstream merge:

| Check | Result / environment |
| --- | --- |
| Focal ABI, packaging, cache and launcher suites; native package and verifier suites | 73 tests run, 72 passed, 1 skipped (NSIS unavailable), WSL Linux / Python 3.12 |
| Real dependency scanner regression | A compiled C++ helper contributes `libstdc++6` even when the application fixture does not need it |
| Default installed/portable runtime paths | 5 passed, Windows / Rust 1.99.0, `cargo test -p nyaterm-core --lib runtime::tests --offline` |
| Compatibility installed/portable paths | Same 5 passed with `NYATERM_LINUX_COMPAT=glibc231` and the compatibility `NYATERM_PACKAGE_MANAGER` value |
| Packaging policy normalization; MCP discovery | 1 + 1 passed with the compatibility build variables, package-specific offline tests |
| Formatting / architecture | `cargo fmt --all -- --check` and `python scripts/ci/check_architecture.py` passed; one upstream zmodem expression was reformatted |
| Shell / Python baseline / whitespace | Focal shell scripts passed `bash -n`; `package.py` passed Python 3.8 syntax parsing; `git diff --check` passed |

Permission-sensitive Python suites used a Git archive extracted onto WSL's native
filesystem, preserving tracked Unix modes. Windows/DrvFS mode reporting cannot
validate Unix archive/icon permissions. The dependency regression used the WSL
package database, not focal's; it proves scanner coverage rather than focal ABI
compatibility of a newly built application.

No new focal Release build, Mesa download/preparation, full-workspace Rust
check/test/clippy, clean-focal dependency-only install, Xvfb smoke, or real desktop
GPU/software/network acceptance was performed in this follow-up. Historical
artifact hashes and graphics results above apply only to their recorded source.
Software mode retains the full private `LD_LIBRARY_PATH` inherited by children;
the pending SSH/SFTP, shell, clipboard/IME and RDP/VNC matrix is in `README.md`.

The persisted data and encryption formats are unchanged. New compatibility debs
start with their own empty configuration directory; old shared configuration is
left in place. Migration uses supported backup/restore, as documented in both
operator readmes. No release assets were replaced by this follow-up.
