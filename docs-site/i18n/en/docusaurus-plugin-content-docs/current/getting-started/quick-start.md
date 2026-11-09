# Quick Start

This chapter helps you experience NyaTerm's core workflow as quickly as possible: create a connection, open sessions, split the workspace, browse files, turn on terminal enhancements, and find the migration, AI assistance, and backup entry points.

## Step 1: Pick a session type

When you click **New Connection**, NyaTerm offers six session types:

- **SSH** — the most complete remote-operations workflow
- **Local Terminal** — open a local shell inside NyaTerm
- **Telnet** — legacy systems or lab environments
- **Serial** — serial debugging devices
- **RDP** — Windows Remote Desktop
- **VNC** — VNC services or VM consoles; real-server interoperability is still being validated

If this is your first time using NyaTerm, start with one **SSH** session and then add one **Local Terminal** to compare the mixed-workspace experience.

## Step 2: Create your first SSH connection

In the new-session window, fill in:

- **Connection Name** — a friendly display name
- **Host** and **Port**
- **Username**
- **Authentication** — password or private key

If needed, expand the advanced section to configure:

- Proxy
- Jump host
- OTP binding and auto-fill
- Icon, group, description, and other metadata

After saving, the connection appears in the saved-connections list.

## Step 3: Import existing configuration if you have it

If you already maintain many hosts in another client, you do not need to rebuild them manually. NyaTerm can import from Xshell, MobaXterm, WindTerm, SecureCRT, FinalShell, Termius, and NyaTerm / Electerm JSON — the format list and caveats are in [Importing sessions from other clients](../guide/ssh-connection#import-sessions-from-other-clients).

The two paths serve different goals:

- Migrating a **connection inventory**: start from the import entry in the **Saved Connections** panel
- Restoring a **complete NyaTerm environment**: use a `.nya` encrypted backup, which requires a master password first

## Step 4: Understand the workspace

Double-click a saved connection, or use the connection context menu, to launch the session.

After the connection is established, you will see:

- **Center area** — the current terminal tab and any split panes inside it
- **Left activity bar** — entry points for file explorer, network, Security/Auth, Cloud Sync, settings, and related panels
- **Right activity bar** — saved connections, AI Assistant, active sessions, command history, and resource monitor
- **Bottom area** — quick commands, serial send, recording, and lock actions

To keep the app running after closing the last workspace window, enable **Minimize the last window to tray on close** in **Settings → General**. With multiple workspace windows, × closes the current workspace normally. Launching again or clicking the tray wakes an existing window; use **New Window** or `--new-window` to create a workspace.

## Step 5: Try the highest-frequency workflows

### 1. Open a local terminal too

Use the ``Ctrl/Cmd + ` `` shortcut or the menu entry to create a local terminal so you can compare local and remote work in one app.

### 2. Try split panes

Right-click a tab and choose:

- **Horizontal Split**
- **Vertical Split**

Use it to watch logs, run commands, and compare output from different hosts at the same time.

### 3. Open the remote file explorer and transfer queue

Once an SSH session is active, the file explorer lets you browse remote directories and perform upload, download, delete, move, rename, properties, path-bar navigation, path copy/send-to-terminal, and terminal-cwd sync actions.

When you start uploads or downloads, the transfer panel shows queue progress, transfer speed, and supports pause, resume, cancel, retry, and duplicate-target handling.

On Windows, you can also try dragging files or folders from the system file manager directly into the file explorer.

### 4. Open command history and quick commands

- **Command History** for recall and fuzzy lookup
- **Quick Commands** for reusable actions, with categories, execution modes, and variable prompts
- **Command suggestions** appear while typing based on history and built-in commands

If suggestions are too noisy or include very long commands, tune **Minimum history command length** and **Maximum history command length** in **Settings → Interaction**.

### 5. Try search / online search / translation

When text is selected in the terminal, the context menu can:

- **Find** inside the current output
- Send text to an **online search** engine
- Open a **translation** dialog with a configured provider

### 6. Try AI Assistant

The **AI Assistant** in the right activity bar can:

- Generating a command from your goal
- Explaining recent terminal output or selected text
- Analyzing errors and suggesting fixes
- Saving approved structured command cards as quick commands

If no model is available the first time you open it, configure providers, models, and risk controls in **Settings → AI**.

### 7. Turn on optional terminal enhancements

In **Settings → Terminal**, you can enable:

- Line numbers
- Timestamps
- Action links
- Keyword highlighting
- Remote resource stats

These are off by default; enable the ones that help your workflow.

### 8. Try session recording

`Ctrl / Cmd + Shift + R` records the active session's output to a log file.

To collect recordings in a fixed directory, set **Recording path** in **Settings → Transfer** first.

### 9. Review the settings

Before settling in, read through these settings groups: **General** (session-tab restoration, minimizing the last window to tray on close), **Interaction** (command suggestions, copy and paste behavior), **Terminal** (scrollback, Keep-Alive, optional enhancements), **Transfer** (download directory, default editor, concurrency), **Security** (master password, screen lock, host key policy), and **AI** (provider, model, risk thresholds).

When troubleshooting, **Settings → General** can open the local log directory and export a diagnostics bundle directly.

## Step 6: Optionally configure Cloud Sync

If you want cross-device configuration sync, continue with:

1. Open **Settings → Security** and set a **Master Password**
2. Open **Settings → Cloud Sync**
3. Choose WebDAV, S3-compatible storage, Gist, or a drive provider
4. Fill in the connection details and run **Test Connection** first
5. Then decide whether to enable startup checks and auto-push

If you are just evaluating NyaTerm for the first time, this step is optional. For the full workflow, see [Cloud Sync](../guide/sync-and-backup).

## Step 7: Keep exploring by use case

- Want to understand the differences between sessions? See [Session Types](../guide/session-types)
- Want to configure auth, proxy, or jump hosts? See [SSH Connection Management](../guide/ssh-connection)
- Want to manage files and auto-upload? See [SFTP File Transfer](../guide/file-transfer)
- Want to learn terminal enhancements and recording? See [Terminal Features](../guide/terminal)
- Want to configure OTP? See [OTP and Authentication](../guide/otp-and-auth)
- Want to enable AI assistance? See [AI Assistant](../guide/ai-assistant)
- Want to enable cloud sync? See [Cloud Sync](../guide/sync-and-backup)
