# NyaTerm 扩展平台 v2

## 设计与职责

v2 将原先只生成文本的插件改为可以调用宿主服务的扩展平台。Wasm 扩展与 SPICE、Builtin/IronRDP、VNC、Android 官方原生组件使用同一管理入口，但保留各自的执行和签名边界。Windows Native RDP 仍是用户明确选择的系统组件例外。

- `nyaterm-plugin-api`：2.0.0 WIT、Rust guest SDK、类型化请求、任务事件和面板模型。
- `nyaterm-core::plugins`：manifest、商店目录、贡献点和授权数据契约。
- `nyaterm-plugin-host`：包快照、Wasmtime、作用域检查、I/O 任务、商店下载、官方原生组件。
- `nyaterm-store::extension_authorizations`：独立的本地授权表，不进入云同步文档。
- desktop：会话/窗口归属、SSH/SFTP 调度、终端操作和 GPUI 面板。
- `temp/nyaterm-plugins`：独立 Git 仓库中的 v2 商店工具、目录、签名和发布流程。

不加载 v1 manifest/WIT。v2 数据位于应用数据目录 `plugins/v2`，原目录保留。连接、凭据、备份和同步格式不变。`temp/vendor/zed` 和 `temp/extensions` 只用于参考，不参与编译，也不修改。

## 生命周期与任务

安装时编译并验证真实组件接口，不执行 guest。首次命令、菜单或会话事件调用才初始化插件；I/O worker 在首次宿主请求时创建。每个实例有独立的 CPU、内存预算和取消状态。

`host.submit(request)` 返回任务 ID；guest 收到 `task_ready(id)` 后调用 `host.poll(id)`。事件包括 Pending、Output、Completed、Failed、Cancelled。Output 是中间事件，必须继续消费到终态；Completed 可附带完整输出和退出码。`host.cancel(id)` 取消任务。

每实例最多 16 个尚未消费终态的任务、2 个 I/O worker。单次读写、HTTP 正文、命令完整输出上限为 1 MiB；流式输出还有队列条目和字节限制，慢消费者会使任务失败。关闭不会依赖任务回调持有的通道发送端。Windows 子进程使用 Job，Unix 子进程使用独立进程组。

## 宿主能力

| 能力 | 范围与行为 |
| --- | --- |
| HTTP | 精确 origin 授权，不自动跟随重定向 |
| 本地文件 | 已授权目录和安全相对路径，分块读写 |
| 远程文件 | 显式 SSH 连接，SFTP 分块读写 |
| 会话列表/事件 | 必须声明并授权 `sessions_read` |
| 终端快照/输入 | 显式连接，输入不依赖命令草稿 |
| SSH 执行 | 显式连接，独立执行通道、流式输出、取消和退出码 |
| 本地程序 | 已授权的绝对可执行文件路径 |
| 私有存储 | 插件自己的普通配置数据 |
| 通知/工具面板 | 声明 UI 能力，多窗口时提供明确 session |

分块写入 offset=0 会截断旧文件，后续块按指定偏移写入。远端分块操作目前要求 SFTP，shell 文件后端返回不支持。HTTP 返回有界正文；没有暴露任意宿主客户端对象。私有存储不是加密凭据仓库，当前没有加密秘密 API，不应存放密码或令牌。

会话句柄包含 UUID、generation 和可选的保存连接 ID。重连使用新 UUID，当前 generation 为 0。宿主验证句柄、保存连接关联、断开状态和 SSH lease，不用活动标签替代请求目标。工具面板按钮携带创建时的 session 上下文；更新或禁用后旧面板失效。

## 授权和来源

Manifest 声明能力不等于外部资源授权。未授权的操作被拒绝，并在管理页列出所需能力和具体网站、目录或连接。授权绑定当前插件 revision，旧页面不能给替换后的插件授权。授权保留 guest 状态和输入，用户直接重试操作；撤销会使任务失效、禁用插件，并清除该插件所有历史版本的授权。

本地/开发包授权绑定 manifest 和组件内容摘要，改动代码需要重新授权。商店安装检查完整 ZIP 的 SHA-256、大小、Minisign 签名以及 ID、publisher、version、API、host requirement、capabilities。保存签名 ZIP 收据，创建实例时重新核验。相同 key/publisher/id 的验证更新保留既有资源授权，新能力或新资源仍需授权。

商店公钥由编译环境 `NYATERM_EXTENSION_PUBLIC_KEY` 注入，默认空值会拒绝商店安装。官方原生组件使用独立应用信任链。商店下载在后台执行，有大小和时间限制、取消检查及关闭等待。商店删除条目不会自动禁用已安装扩展；签名 key 轮换需要客户端更新和重新授权。

## UI 与示例

管理页提供发现/已安装、搜索、在线安装、更新、启停、卸载和授权。离线扩展和官方组件都直接使用文件选择器；开发目录注册放在开发工具区。原先“只能接收文本”“无法访问文件、网络或终端”的限制文案已移除。模板动作和可编辑命令结果仍可按需要使用。

贡献点包括命令搜索、终端菜单、文件菜单、连接/断开事件。会话事件按连接实例去重并检查授权。工具面板支持文本、滚动日志、表格、输入、按钮和进度。

运行 `python scripts/plugins/build.py --fixtures`，产物位于 `target/plugin-examples/`：

1. 导入 `remote-logs.zip`，从已连接的 SSH 终端或远程文件菜单打开工具，填写路径，授权读取后再次读取日志。
2. 导入 `upload-deploy.zip`，从 SSH 菜单打开，填写本地目录、相对文件路径、远端目标和命令。分别授权本地读取、远端写入和远端执行。文件分块上传完成后才执行 SSH 命令，输出显示在面板中，Stop 可取消。

示例不从命令搜索猜测当前连接。失败或取消的上传可能留下远端部分文件。日志面板仅保留显示尾部，避免无限增长。

## 商店联调与上线条件

`nyaterm-plugin-tool inspect <directory-or.zip>` 输出协议版本 2 和已验证 manifest；`catalog <json>` 使用应用实际解析器验证目录。这两个命令不运行 guest。

商店 `tooling/v2`、`public/v2/plugins.json` 为权威入口，旧 v1 工具不再由 npm 脚本调用。`npm run validate -- --packages` 需要 `NYATERM_PLUGIN_CLI` 和 Minisign，检查目录、下载签名包并比较真实 manifest。Node 测试设置 `NYATERM_PLUGIN_CLI` 后执行 Rust 交叉验证；`NYATERM_TEST_CATALOG_OUTPUT` 可导出非空测试目录。

发布需配置受保护的 `plugin-signing` 环境、审核后的 40 位 `NYATERM_VALIDATOR_REF` 和签名公钥。工作流签署已审核的 ZIP 原始字节，上传不可覆盖的资产，再创建目录 PR；不会构建或运行候选插件代码。本次没有执行线上发布。当前公开目录为空，不能据此宣称线上包下载已经验收。

## 验证（2026-11-25，Windows x64）

- `cargo check --workspace`、`cargo clippy --workspace --all-targets`、主工作区和 guest 工作区格式检查通过。
- 全工作区测试：7,713 项通过、75 项按条件跳过；使用 `RUST_MIN_STACK=16777216`。
- 最后授权调整后的定向复测：插件宿主 46 项、桌面插件 5 项通过；新增 SFTP 分块测试 2 项通过。
- 商店 Node 测试 4 项通过，覆盖非空目录与 Rust 解析器交叉验证；空发布目录通过 `validate --packages`。
- Python 发布/打包测试共 288 项，22 项跳过。首次运行有一项缺少 MSVC 命令行环境，加载开发环境后所属 Android 组 25 项全部补跑通过。
- 五个示例包和 SDK 生命周期 fixture 已重新构建。

真实 SSH/SFTP、四类原生会话、多窗口交互，以及 Linux/macOS 联机行为仍需要相应环境验收。
