---
sidebar_position: 3
---

# Frontend Development

Frontend code lives in `src/` and uses React 19 + TypeScript.

## Entry points and window model

The frontend entry point is `src/main.tsx`. It decides which app tree to load based on the `?window=` query parameter in the URL:

- **Main window** — `AppProvider` + `App.tsx`
- **Child windows** — `ChildAppProvider` + `ChildWindowRouter`

Current child-window flows include:

- settings
- new-session
- quick-command
- auto-upload

If you are changing these flows, start with:

- `src/main.tsx`
- `src/ChildWindowRouter.tsx`
- `src/lib/windowManager.ts`

## Component and directory structure

```text
src/
├── components/          # UI components
│   ├── dialog/          # Dialog and child-window related components
│   ├── panel/           # Left/right sidebar and bottom helper panels
│   ├── terminal/        # xterm workspace and terminal-related components
│   ├── layout/          # Outer layout, title bar, activity bars
│   └── ui/              # Shared base UI components (shadcn/ui)
├── context/             # React Context providers
├── hooks/               # Custom hooks
├── i18n/                # Internationalization
├── lib/                 # invoke wrapper, window manager, workspace helpers
├── pages/               # Child-window pages
├── types/               # Shared type definitions
├── App.tsx              # Main application shell
└── main.tsx             # Frontend entry point
```

## State management

### AppContext

`src/context/AppContext.tsx` is the main state container for the primary window. It manages:

- Tabs and pane trees
- Active tab and active pane
- Saved connections and group refreshes
- App settings and UI settings
- Startup restoration of the workspace

### ChildAppProvider

`src/context/ChildAppProvider.tsx` is the lightweight provider for child windows:

- Loads and saves settings only
- Does not hold the full workspace state
- Syncs settings changes back to the main window through events

### TransferContext

`src/context/TransferContext.tsx` listens to `transfer-event` and centrally manages:

- Transfer queue items
- Progress, paused, canceled, and error state
- Pause / resume / cancel / retry actions

## Calling Tauri commands

Frontend code should prefer the shared wrapper in `src/lib/invoke.ts` rather than scattering raw `@tauri-apps/api/core` `invoke()` calls everywhere.

```ts
import { invoke } from '@/lib/invoke';

const sessionId = await invoke<string>('create_ssh_session', {
  connectionId: 'uuid-here',
});
```

This wrapper centralizes error logging and makes future call behavior easier to change.

## Listening to backend events

Many frontend features rely on Tauri events, for example:

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

Terminal rendering, file browsing, resource monitoring, transfer queues, and the Sync & Backup status / history / conflict UI all sit on top of these events.

## Workspace model

The workspace has two layers.

### `workspaceTabs.ts`

This file manages the persisted logical workspace:

- Tabs
- Pane trees
- In-tab splits
- Serialization / restoration of `ui.open_tabs`

### `tabWindows.ts`

This file manages the live runtime terminal layout:

- Which tabs are attached to which leaf
- The active tab for each leaf
- Runtime split ratios

When editing tabs, splits, or multi-area terminal layout behavior, first decide which layer you are actually changing.

## Terminal integration

`src/components/terminal/XTerminal.tsx` is the main xterm.js integration point. It handles:

- Search / Fit / WebLinks addons
- Shell integration and command suggestions
- Gutter rendering for line numbers and timestamps
- Action links and keyword highlighting
- Large-output protection
- Reconnect-related behavior

If you are changing terminal presentation, this is usually the first file to inspect.

## Sync & Backup frontend entry points

If you are changing sync / backup UI flows, start with these files:

- `src/pages/SettingsPage.tsx` — settings-tab structure, save blocking, and master-password prerequisites
- `src/components/settings/SyncBackupTab.tsx` — provider config, automatic strategies, manual actions, remote backups, and conflict handling
- `src/components/panel/SyncBackupHistoryPanel.tsx` — workspace history panel and quick conflict actions
- `src/App.tsx` — how the Sync & Backup panel is wired into the main workspace
- `src/lib/cloudSync.ts` — frontend defaults, formatting helpers, and provider validation utilities

Together these files define the full user flow through settings state, Tauri commands, and cloud-sync events.

## Internationalization

User-facing UI text uses `react-i18next`. Locale files are in:

- `src/i18n/locales/zh-CN.json`
- `src/i18n/locales/en.json`

Whenever you add or change visible UI text, update both locale files.

## UI component conventions

The project uses shadcn/ui as its base component layer. Shared UI components live in `src/components/ui/`.

If you need a new reusable UI piece, prefer existing components and project style patterns over building a parallel base component system.
