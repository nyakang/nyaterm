# GPUI 桌面开发

NyaTerm 的原生界面位于 `crates/nyaterm-desktop`，共享控件位于 `crates/nyaterm-ui`。这里的“桌面层”同时包含 GPUI 状态、视图、窗口交互、平台适配和后台任务结果的接入。

## 入口与窗口

`nyaterm-app` 创建根窗口和 `AppShell`。存储启动完成后，`AppShell` 创建 `NyaTermApp` Entity 并启动工作区恢复。

设置、连接编辑、快捷命令、远程文本编辑等独立窗口通过 GPUI `open_window` 创建。窗口之间传递 Entity、typed state 或明确的回调，不使用 URL 路由或消息桥。

## 模块结构

```text
crates/nyaterm-desktop/src/
├── app_shell/       # 根 shell、启动/恢复/退出生命周期和原生菜单
├── entities/        # 窗口 runtime、启动恢复和 quick switch 的权威 Entity
├── features/        # focused feature state、运行时适配和视图
├── i18n/            # locale 加载与翻译
├── models/          # 桌面呈现模型
├── http/            # 原生 HTTP 适配
└── terminal.rs      # 终端展示层入口
```

`features/` 按领域组织连接、会话、终端、设置、安全、传输、隧道、同步、AI、远程操作、布局和面板。新增功能应进入拥有该行为的领域目录，不要创建新的通用迁移桶或共享 prelude。

## 状态管理

`NyaTermApp` 组合各个 focused feature state。只操作一个领域的方法优先放到对应 state 上；需要通知 GPUI、访问窗口或协调多个领域时，再由 `NyaTermApp` 提供薄适配。

遵守以下所有权规则：

- 每份可变状态只有一个权威 owner。
- 不在 `NyaTermApp`/feature state 和 Entity store 中维护双写镜像。
- 由视图直接读取当前状态，不在 render 中发布再读回快照。
- 跨线程结果通过 typed event 或 typed task result 进入 GPUI update。

## 视图与控件

构造 GPUI element 的 helper 留在 view 或桌面 feature 中，不要为减少 `impl NyaTermApp` 数量而把视图构造移动到纯状态模型。

普通输入、选择、菜单、开关和对话框使用 `nyaterm-ui` 暴露的稳定组件 API。桌面 feature 不直接依赖 `gpui-kit`。普通文本字段使用 `NyaInput`/`NyaInputState` 或 `features/text_inputs.rs` 的 id registry，并为输入框提供明确尺寸。

终端输入、粘贴审查和 `RemoteTextEditor` 是完整编辑面，不应替换成普通单行输入。

## 输入与原生窗口交互

全局快捷键和 pointer 事件从根视图路由到当前 feature。处理事件时应明确何时调用 `cx.stop_propagation()`，并避免父级 click handler 抢回文本字段焦点。

平台相关窗口、剪贴板、拖放、PTY 和输入法行为需要在受影响的操作系统上验证。创建子窗口时复用已有窗口生命周期和 modal 协调模式。

## 终端展示

终端职责分为两层：

- `nyaterm-terminal` 维护终端网格、scrollback、控制序列、搜索和图形协议状态。
- `nyaterm-terminal-gpui` 负责像素布局、键盘事件转换、选区、高亮、图片和绘制。

桌面层把会话输出送入终端状态机，再把 snapshot 和交互状态交给 GPUI terminal element。不要在视图中重新实现控制序列解析或 wire protocol。

## 国际化

应用语言包位于：

- `crates/nyaterm-desktop/src/i18n/locales/zh-CN.json`
- `crates/nyaterm-desktop/src/i18n/locales/en.json`

新增或修改用户可见文本时同步更新两种语言，并复用现有 translation key 命名方式。

## 后台任务与测试

render 和长时间 GPUI update callback 中不得执行数据库、文件系统、网络、SSH、SFTP、子进程或图片解码工作。使用 GPUI executor、专用 runtime 或已有 job coordinator，并在结果返回时更新权威状态。

状态迁移尽量通过纯方法测试；GPUI 交互使用相邻模块中的 `#[gpui::test]` 或现有 test context。涉及窗口、剪贴板、拖放和 IME 的改动还需做平台 smoke test。
