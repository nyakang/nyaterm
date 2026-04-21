---
sidebar_position: 3
---

# 前端开发指南

前端代码位于 `src/`，使用 React 19 + TypeScript。

## 入口与窗口模型

前端入口在 `src/main.tsx`，它会根据 URL 中的 `?window=` 参数决定加载哪套应用：

- **主窗口**：`AppProvider` + `App.tsx`
- **子窗口**：`ChildAppProvider` + `ChildWindowRouter`

当前子窗口包括：

- settings
- new-session
- quick-command
- auto-upload

如果你要修改这些流程，优先查看：

- `src/main.tsx`
- `src/ChildWindowRouter.tsx`
- `src/lib/windowManager.ts`

## 组件与目录结构

```text
src/
├── components/          # UI 组件
│   ├── dialog/          # 对话框与子窗口相关组件
│   ├── panel/           # 左右侧栏 / 底部区域面板
│   ├── terminal/        # xterm 工作区与终端相关组件
│   ├── layout/          # 外层布局、标题栏、活动栏
│   └── ui/              # 基础 UI 组件（shadcn/ui）
├── context/             # React Context providers
├── hooks/               # 自定义 hooks
├── i18n/                # 国际化
├── lib/                 # invoke、window 管理、工作区模型等工具
├── pages/               # 子窗口页面
├── types/               # 类型定义
├── App.tsx              # 主应用壳层
└── main.tsx             # 前端入口
```

## 状态管理

### AppContext

`src/context/AppContext.tsx` 是主窗口核心状态容器，管理：

- 标签页与 pane 树
- 活动 tab / pane
- 已保存连接 / 分组刷新
- 应用设置与 UI 设置
- 启动恢复工作区

### ChildAppProvider

`src/context/ChildAppProvider.tsx` 是子窗口用的轻量 Provider：

- 只加载 / 保存设置
- 不持有完整工作区状态
- 通过事件向主窗口同步设置变化

### TransferContext

`src/context/TransferContext.tsx` 监听 `transfer-event`，集中维护：

- 传输队列列表
- 进度 / 暂停 / 取消 / 错误状态
- pause / resume / cancel / retry 操作

## 调用 Tauri 命令

前端应优先通过 `src/lib/invoke.ts` 中的统一包装调用后端，而不是直接到处散写 `@tauri-apps/api/core` 的 `invoke()`。

```ts
import { invoke } from '@/lib/invoke';

const sessionId = await invoke<string>('create_ssh_session', {
  connectionId: 'uuid-here',
});
```

这个包装会统一做错误日志记录，也便于以后集中调整调用行为。

## 监听后端事件

前端大量能力依赖 Tauri 事件系统，例如：

- `terminal-output-{id}`
- `cwd-changed-{id}`
- `session-closed-{id}`
- `transfer-event`
- `sessions-changed`
- `connections-changed`
- `otp-request`
- `cloud-sync-status-changed`
- `cloud-sync-history-changed`
- `cloud-sync-conflict`

终端、文件浏览器、资源监控、传输队列，以及 Sync & Backup 的状态 / 历史 / 冲突提示都建立在这些事件之上。

## 工作区模型

工作区有两层模型：

### `workspaceTabs.ts`

负责“会保存下来的逻辑工作区”：

- 标签页
- pane 树
- 标签页内分屏
- `ui.open_tabs` 序列化 / 恢复

### `tabWindows.ts`

负责“运行时终端布局”：

- 哪些标签当前挂在哪个 leaf
- 每个 leaf 当前的 active tab
- 运行时 split ratio

修改标签 / 分屏 / 多区域终端布局时，先判断你碰的是哪一层。

## 终端集成

`src/components/terminal/XTerminal.tsx` 是 xterm.js 集成核心，负责：

- Search / Fit / WebLinks addon
- shell integration 与命令建议
- gutter（行号 / 时间戳）
- 动作链接与关键词高亮
- 大输出保护
- 会话重连相关行为

如果你改的是终端表现层，这通常是第一落点。

## Sync & Backup 前端落点

如果你改的是同步与备份相关 UI，优先查看这些文件：

- `src/pages/SettingsPage.tsx` — 设置页 tab 结构、保存拦截和主密码前置逻辑
- `src/components/settings/SyncBackupTab.tsx` — provider 配置、自动策略、手动操作、远程备份与冲突处理
- `src/components/panel/SyncBackupHistoryPanel.tsx` — 工作区历史面板与冲突快速处理入口
- `src/App.tsx` — Sync & Backup 面板在主工作区中的接入方式
- `src/lib/cloudSync.ts` — 前端默认值、格式化和 provider 校验工具

这些文件之间通过设置状态、Tauri commands 和 cloud-sync 事件一起驱动完整体验。

## 国际化

界面文案使用 `react-i18next`，语言包位于：

- `src/i18n/locales/zh-CN.json`
- `src/i18n/locales/en.json`

新增或修改用户可见文本时，应同时更新两个 locale 文件。

## UI 组件约定

项目使用 shadcn/ui 作为基础组件层，共享组件位于 `src/components/ui/`。

如果需要新增通用 UI，优先复用现有组件与项目里的样式模式，而不是单独造一套新的基础组件。
