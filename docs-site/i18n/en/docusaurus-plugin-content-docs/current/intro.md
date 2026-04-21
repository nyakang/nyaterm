---
sidebar_position: 1
slug: /
---

# Introduction

**Dragonfly** is a desktop client built around remote terminal workflows. It pairs a Tauri + React interface with a Rust backend that handles SSH, SFTP, session lifecycle, tunnels, authentication, and config persistence, so you can work with remote servers, local shells, serial devices, and network helpers inside one workspace.

## Where Dragonfly fits best

- Managing multiple SSH hosts at the same time
- Switching between local terminals, Telnet sessions, and serial devices during troubleshooting
- Working with remote files while watching terminal output
- Standardizing common operations with reusable commands, jump-host chains, and saved connection metadata
- Using OTP, recording, resource monitoring, auto-upload, cross-device config sync, and encrypted backup in the same desktop app

## Core capabilities

### Multiple session types

Dragonfly supports more than SSH:

- **SSH** — remote login, file transfer, resource monitoring, tunnels, OTP, and related workflows
- **Local Terminal** — open a local shell inside the same workspace
- **Telnet** — support for legacy systems and lab environments
- **Serial** — useful for network gear, embedded boards, and debug ports

### Composable workspace

- Multi-tab workflow for different tasks and environments
- **Horizontal and vertical splits** inside a tab
- Left and right activity bars for file explorer, network, Security/Auth, saved connections, active sessions, command history, and resource monitor panels
- Bottom helper areas for quick commands, serial send, recording, and lock controls
- Separate child windows for settings, new-session, quick-command editing, and auto-upload prompts

### Terminal-focused enhancements

- Command history and fuzzy suggestions
- Terminal search, copy/paste, and context actions
- **Online search** and **translation** from selected terminal text
- Optional **line-number / timestamp gutter**
- Optional **action links** for IPv4 addresses, `host:port`, and archive names
- Optional **keyword highlighting** with built-in presets and custom rules
- Large-output protection, session recording, and SSH keep-alive

### Remote file and transfer workflows

- Built-in SFTP file explorer for SSH sessions
- Upload, download, rename, move, delete, properties, and symlink actions
- Transfer queue with pause, resume, cancel, retry, timestamp preservation, and configurable concurrency
- Open a remote file in a local editor, then send changes back through the watcher-driven auto-upload flow

### Security and networking

- Passwords, private keys, host-key policies, and encrypted local storage
- OTP management with TOTP/HOTP, QR import, and SSH auto-fill support
- Proxy configs, jump hosts, and local / remote / dynamic tunnels
- Screen lock and master-password support

### Cloud sync and encrypted backup

- Sync Dragonfly's portable configuration data through **WebDAV** or **S3-compatible** storage
- Configure a master password in **Settings → Security** before using **Settings → Sync & Backup**
- Support startup checks, debounced auto-push after supported local changes, scheduled encrypted backups, and remote backup restore
- Resolve snapshot-level conflicts from the settings page or the in-workspace history panel when both local and remote state changed

## Suggested reading order

If you are new to Dragonfly, this order works well:

1. [Quick Start](./getting-started/quick-start)
2. [Session Types](./guide/session-types)
3. [SSH Connection Management](./guide/ssh-connection)
4. [Layout and Workspace](./guide/layout-and-workspace)
5. [Terminal Features](./guide/terminal)
6. [SFTP File Transfer](./guide/file-transfer)
7. [Tunnels and Proxy](./guide/tunnels-and-proxy)
8. [OTP and Authentication](./guide/otp-and-auth)
9. [Sync & Backup](./guide/sync-and-backup)
