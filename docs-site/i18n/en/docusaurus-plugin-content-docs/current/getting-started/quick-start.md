---
sidebar_position: 2
---

# Quick Start

This chapter helps you experience Dragonfly's core workflow as quickly as possible: create a connection, open sessions, split the workspace, browse files, and turn on terminal enhancements when you need them.

## Step 1: Pick a session type

When you click **New Connection**, Dragonfly offers four session types:

- **SSH** — the most complete remote-operations workflow
- **Local Terminal** — open a local shell inside Dragonfly
- **Telnet** — useful for legacy systems or lab environments
- **Serial** — useful for serial debugging devices

If this is your first time using Dragonfly, start with one **SSH** session and then add one **Local Terminal** to compare the mixed-workspace experience.

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

## Step 3: Understand the workspace

Double-click a saved connection, or use the connection context menu, to launch the session.

After the connection is established, you will see:

- **Center area** — the current terminal tab and any split panes inside it
- **Left activity bar** — entry points for file explorer, network, Security/Auth, and related panels
- **Right activity bar** — saved connections, active sessions, command history, and resource monitor
- **Bottom area** — quick commands, serial send, recording, and lock actions

## Step 4: Try the highest-frequency workflows

### 1. Open a local terminal too

Use the ``Ctrl/Cmd + ` `` shortcut or the menu entry to create a local terminal so you can compare local and remote work in one app.

### 2. Try split panes

Right-click a tab and choose:

- **Horizontal Split**
- **Vertical Split**

This is useful when you want to watch logs, run commands, and compare output from different hosts at the same time.

### 3. Open the remote file explorer and transfer queue

Once an SSH session is active, the file explorer lets you browse remote directories and perform upload, download, delete, move, rename, and properties actions.

When you start uploads or downloads, the transfer panel shows queue progress and supports pause, resume, cancel, and retry.

### 4. Open command history and quick commands

- **Command History** is useful for recall and fuzzy lookup
- **Quick Commands** is useful for reusable actions with categories, execution modes, and variable prompts

### 5. Try search / online search / translation

When text is selected in the terminal, the context menu can:

- **Find** inside the current output
- Send text to an **online search** engine
- Open a **translation** dialog with a configured provider

### 6. Turn on optional terminal enhancements

In **Settings → Terminal**, you can enable:

- Line numbers
- Timestamps
- Action links
- Keyword highlighting
- Remote resource stats

These features are intentionally conservative by default, so you can enable them only where they help your workflow.

## Step 5: Optionally configure sync and backup

If you want cross-device configuration sync or recoverable encrypted backups for this device, continue with:

1. Open **Settings → Security** and set a **Master Password**
2. Open **Settings → Sync & Backup**
3. Choose **WebDAV** or an **S3-compatible** storage provider
4. Fill in the connection details and run **Test Connection** first
5. Then decide whether to enable automatic sync and scheduled backup

If you are just evaluating Dragonfly for the first time, this step is optional. For the full workflow, see [Sync & Backup](../guide/sync-and-backup).

## Step 6: Keep exploring by use case

- Want to understand the differences between sessions? See [Session Types](../guide/session-types)
- Want to configure auth, proxy, or jump hosts? See [SSH Connection Management](../guide/ssh-connection)
- Want to manage files and auto-upload? See [SFTP File Transfer](../guide/file-transfer)
- Want to learn terminal enhancements and recording? See [Terminal Features](../guide/terminal)
- Want to configure OTP? See [OTP and Authentication](../guide/otp-and-auth)
- Want to enable cloud sync and encrypted backups? See [Sync & Backup](../guide/sync-and-backup)
