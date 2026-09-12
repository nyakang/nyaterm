<p align="center">
  <img src="./public/icons/app/nyaterm.svg" alt="NyaTerm" width="128" height="128">
</p>

<h1 align="center">NyaTerm</h1>

<p align="center">
  <em>/ˈnjaː tɜːrm/</em>
</p>

<p align="center">
  <strong>A modern remote terminal workspace built with Tauri, React, and Rust.</strong><br/>
  <a href="https://nyaterm.app"><strong>nyaterm.app</strong></a> ·
  <a href="https://nyaterm.app/docs/"><strong>Documentation</strong></a>
</p>

<p align="center">
  SSH, local shells, Telnet, Serial, RDP, VNC, SFTP, tunnels, OTP, AI assistance, and encrypted sync in one desktop client.
</p>

<p align="center">
  <a href="https://discord.gg/Tgb9ugMKdM" target="_blank"><img src="https://dcbadge.limes.pink/api/server/Tgb9ugMKdM" alt="Discord" /></a>
  &nbsp;
  <a href="https://docs.qq.com/doc/DYW5TUkZnakxOWUFW" target="_blank"><img src="https://img.shields.io/badge/WeChatGroup-Join-07C160?style=for-the-badge&logo=wechat&logoColor=white" alt="WeChatGroup" /></a>
  <br/>
  <a href="https://nyaterm.app"><img alt="Version" src="https://img.shields.io/github/v/release/nyakang/nyaterm?style=flat-square&logo=github&color=0EA5E9&labelColor=334155"></a>
  &nbsp;
  <a href="https://github.com/nyakang/nyaterm/releases"><img alt="GitHub downloads" src="https://img.shields.io/github/downloads/nyakang/nyaterm/total?style=flat-square&logo=github&color=0EA5E9&label=Downloads&labelColor=334155"></a>
  &nbsp;
  <a href="#"><img alt="Platform" src="https://img.shields.io/badge/Platform-Windows%20%7C%20macOS%20%7C%20Linux-0EA5E9?style=flat-square&logo=linux&labelColor=334155"></a>
  &nbsp;
  <a href="LICENSE"><img alt="License" src="https://img.shields.io/badge/License-MIT-0EA5E9?style=flat-square&logo=readthedocs&labelColor=334155"></a>
</p>

<p align="center">
  <a href="./README.md">English</a> · <a href="./README.zh-CN.md">简体中文</a>
</p>

---

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="./docs-site/static/img/home/product-dark.png">
    <source media="(prefers-color-scheme: light)" srcset="./docs-site/static/img/home/product-light.png">
    <img alt="NyaTerm main workspace" src="./docs-site/static/img/home/product-light.png">
  </picture>
</p>

---

<a name="ai-assistant"></a>
# AI Assistant

NyaTerm includes an AI Assistant panel for command generation, terminal output explanation, error analysis, and multi-step terminal workflows.

## What It Can Do

- **Ask mode** for one-off help such as generating commands, explaining selected output, and analyzing errors
- **Agent mode** for multi-step work using an observe-decide-run loop against the active terminal session
- **Recent-output actions** so you can ask AI to explain the latest terminal output without manually copying it
- **Structured command cards** with risk levels, execution controls, and optional save-to-quick-command support
- **Approved command execution tools** for Agent workflows, with a separate final-answer step after terminal work completes
- **Inline terminal capture** during Agent execution with configurable `Terminal Output Lines`
- **Session mentions** with `@` to bring other terminal sessions into the AI context
- **Provider management** for built-in providers, manual model entries, credential groups, and custom OpenAI-compatible endpoints
- **Risk control** for high-impact commands, including approval gates and safer alternatives

---

<a name="what-is-nyaterm"></a>
# What is NyaTerm

**NyaTerm** is a desktop client for SSH-centric operations and mixed terminal workflows. It combines a React + Tauri interface with a Rust backend so you can manage remote hosts, local shells, file transfers, authentication, network tooling, AI-assisted terminal actions, session import/export, diagnostics, and encrypted sync/backup from one workspace.

- **NyaTerm is** an SSH client for developers, sysadmins, and DevOps engineers
- **NyaTerm is** a terminal workspace with tabs, horizontal splits, and vertical splits
- **NyaTerm is** an SFTP browser with a transfer queue and local-edit-then-upload-back workflow
- **NyaTerm supports** SSH, Local Terminal, Telnet, Serial, RDP, and VNC sessions
- **NyaTerm is not** a shell replacement; it connects to remote shells, local shells, Telnet endpoints, and serial devices

---

<a name="why-nyaterm"></a>
# Why NyaTerm

NyaTerm is built for people who move between servers, local commands, devices, and configuration files all day.

- **Workspace-first** — keep related terminals together with tabs, split panes, side panels, and child windows
- **Remote operations in context** — browse SFTP files, follow terminal paths, run transfers, and edit remote files without leaving the session
- **Security-aware workflows** — manage credentials, keys, known hosts, OTP, lock screen, and master-password protected storage
- **Portable configuration** — import from existing tools, export encrypted `.nya` backups, and sync encrypted snapshots through WebDAV or S3-compatible storage
- **AI where it is useful** — generate commands, inspect output, and run approved multi-step actions from the active terminal context

---

<a name="features"></a>
# Features

## Sessions and Workspace

- SSH, Local Terminal, Telnet, Serial, RDP, and VNC session support
- Multi-tab workspace with horizontal and vertical pane splits, tab drag docking, and layout restoration
- RDP and VNC remote desktop panes; VNC currently supports direct TCP, None / classic VNC Auth, Raw / ZRLE / Tight / Tight JPEG framebuffer updates, window scaling, bounded reconnects, and text clipboard exchange for Latin-1 text
- Saved connections with folders, icons, metadata, duplication, keyboard copy, reconnect, and import flows
- Command Palette and session quick switcher for finding actions, open sessions, saved connections, and new-session entry points
- Main-window `Background Image` customization with `cover` / `contain` / `stretch` / `tile` sizing and adjustable `Background Content Opacity`
- Left and right activity bars for file explorer, network, Security/Auth, Sync & Backup, AI Assistant, Notes, saved connections, active sessions, command history, asset monitoring, resource monitoring, GPU monitor, process manager, and Docker manager
- Notes panel and editor with tree navigation, context menus, autosave, toolbar controls, and sync/backup-aware persistence
- Asset monitoring workspace with grouped connection views, breadcrumb navigation, table/card layouts, and resource/GPU/NPU status integration
- Remote host monitoring panels for SSH sessions: resource monitor, NVIDIA GPU and Ascend NPU overviews, process manager (signal/renice), and Docker manager (containers, images, volumes, networks, Compose)
- Session input sync groups to broadcast typed input and sent commands to multiple sessions at once
- Temporary SSH links for one-off connections from a pasted `ssh://` URL or `ssh` command without saving a connection
- Child windows for settings, new-session creation, quick-command editing, remote-file editing, and auto-upload prompts
- Tray support with optional minimize-to-tray and hide-main-window behavior

## Terminal Experience

- Terminal search with result navigation, search history, copy/paste, context menus, and selected-text actions
- Command history with fuzzy suggestions, configurable length filters, and suppression in interactive programs
- Unicode grapheme rendering for emoji, combining marks, and ZWJ sequences
- Optional line-number and timestamp gutter
- Optional action links for IPv4 addresses, `host:port`, and archive filenames
- Optional keyword highlighting with expanded built-in presets, custom rules, and JSON import
- Terminal zoom, workspace padding, font weight controls, macOS IME compatibility, image path paste handling, and a clear-terminal action
- AI shortcuts for explaining recent output, plus inline Agent command output with configurable `Terminal Output Lines`
- Large-output protection, configurable scrollback, SSH keep-alive, and session recording
- Online search and translation from selected terminal text
- ZMODEM file transfer support directly from the terminal, plus Serial XMODEM/YMODEM/ZMODEM drag-and-drop uploads surfaced in the transfer queue
- Confirmation dialog before closing all sessions
- Customizable keyboard shortcuts for terminal and UI actions, including `Backspace Mode` selection for Telnet and Serial sessions

## SFTP and File Workflows

- Built-in SFTP file explorer for SSH sessions
- Upload, download, rename, move, delete, properties, new file/folder, and OpenSSH-compatible symlink actions
- Folder upload/download, multi-folder upload selection, multi-select, editable path bar, and manual/automatic sync with terminal cwd
- Transfer queue with speed display, pause, resume, cancel, retry, duplicate-target handling, timestamp preservation, and configurable concurrency
- Enhanced SCP and directory-transfer handling with original-property preservation where available
- Open remote files in a local editor and upload saved changes back through the watcher-driven auto-upload flow, with content fingerprinting so only real content changes trigger re-upload
- SFTP channel concurrency limiting and automatic retry on transient channel-open failures
- External drag-and-drop upload support on Windows
- Zmodem transfers track local paths and can reveal completed files in the system file manager

## Security, Authentication, and Networking

- Password authentication, private keys, host-key verification, and encrypted local persistence
- Credential management with regex-based terminal auto-fill
- OTP management with TOTP/HOTP, QR import, and SSH auto-fill support
- Per-connection SSH algorithm preferences with Compatible / Secure / Custom modes and security-risk labels for key exchange, ciphers, MACs, and host keys
- Proxy configurations including SOCKS5, HTTP, and ProxyCommand; SSH jump hosts with cycle prevention; local / remote / dynamic tunnels
- SSH X11 forwarding for environments with a local X server
- Screen lock, idle app lock, master password, diagnostics settings, local log management, and diagnostics bundle export

## Sync, Backup, and Migration

- Encrypted cloud sync and backup through WebDAV, S3-compatible storage, and GitHub Gist
- Master password required before sync, backup, encrypted import/export, or scheduled encrypted backup actions
- Startup sync checks, debounced auto-push after supported local changes, detailed status updates, and scheduled backup retention
- Manual test / push / pull / backup actions, remote backup restore, and snapshot-level conflict resolution
- Session import from Xshell, MobaXterm, WindTerm, and NyaTerm JSON definitions
- Full-app encrypted `.nya` import/export for portable NyaTerm configuration

---

<a name="screenshots"></a>
# Screenshots

## Workspace

Manage SSH, local shell, Telnet, and Serial sessions inside one tabbed and split-pane workspace.

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="./docs-site/static/img/home/overview-dark.png">
    <source media="(prefers-color-scheme: light)" srcset="./docs-site/static/img/home/overview-light.png">
    <img alt="NyaTerm workspace overview" src="./docs-site/static/img/home/overview-light.png">
  </picture>
</p>

## Appearance and Background Image

Use a local wallpaper behind the main window, tune `Image Sizing`, `Image Opacity`, and `Background Content Opacity`, and keep child windows readable with solid surfaces.

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="./docs-site/static/img/home/cover-dark.png">
    <source media="(prefers-color-scheme: light)" srcset="./docs-site/static/img/home/cover-light.png">
    <img alt="NyaTerm background image customization" src="./docs-site/static/img/home/cover-light.png">
  </picture>
</p>

## Terminal Enhancements

Use command history, search, translation, action links, timestamps, keyword highlighting, and large-output protection in the terminal.

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="./docs-site/static/img/home/terminal-dark.png">
    <source media="(prefers-color-scheme: light)" srcset="./docs-site/static/img/home/terminal-light.png">
    <img alt="NyaTerm terminal features" src="./docs-site/static/img/home/terminal-light.png">
  </picture>
</p>

## Remote Files

Browse SFTP files beside the terminal, manage transfers, and send local editor changes back to the remote path.

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="./docs-site/static/img/home/files-dark.png">
    <source media="(prefers-color-scheme: light)" srcset="./docs-site/static/img/home/files-light.png">
    <img alt="NyaTerm SFTP file workflow" src="./docs-site/static/img/home/files-light.png">
  </picture>
</p>

## Security and Network Tools

Manage credentials, OTP, known hosts, proxies, jump hosts, and SSH tunnels from dedicated panels.

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="./docs-site/static/img/home/security-dark.png">
    <source media="(prefers-color-scheme: light)" srcset="./docs-site/static/img/home/security-light.png">
    <img alt="NyaTerm security and network tools" src="./docs-site/static/img/home/security-light.png">
  </picture>
</p>

## Sync and Backup

Sync encrypted portable configuration snapshots and restore backups through WebDAV or S3-compatible storage.

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="./docs-site/static/img/home/sync-dark.png">
    <source media="(prefers-color-scheme: light)" srcset="./docs-site/static/img/home/sync-light.png">
    <img alt="NyaTerm sync and backup" src="./docs-site/static/img/home/sync-light.png">
  </picture>
</p>

---

<a name="supported-platforms"></a>
# Supported Platforms

| OS | Support |
| :--- | :--- |
| **Windows** | Windows 10/11, x64 / arm64 |
| **macOS** | macOS 12+, Intel / Apple Silicon |
| **Linux** | Ubuntu 20.04+, Fedora 36+, Arch Linux, and similar distributions |

Download installers from [nyaterm.app](https://nyaterm.app) or the [Releases](https://github.com/nyakang/nyaterm/releases) page.

---

<a name="supported-session-types"></a>
# Supported Session Types

| Type | Typical use | Notes |
|------|-------------|-------|
| SSH | Linux / Unix remote servers | Supports SFTP, OTP, resource / GPU / process / Docker monitoring, proxy, jump host, tunnels, and per-connection algorithm preferences |
| Local Terminal | Local shell workflows | Uses your local shell path and working directory |
| Telnet | Legacy network devices or lab systems | Lightweight terminal session without SSH-only features, with `Backspace Mode` for `Ctrl+H (BS)` or `DEL (0x7F)` |
| Serial | Routers, boards, embedded devices | Configurable port, baud rate, data bits, parity, stop bits, `Backspace Mode`, and XMODEM/YMODEM/ZMODEM drag-and-drop upload protocol |

---

<a name="getting-started"></a>
# Getting Started

## Download

Download the latest build for your platform from [nyaterm.app](https://nyaterm.app) or [Releases](https://github.com/nyakang/nyaterm/releases).

| Platform | Format |
|----------|--------|
| Windows | `.msi` / `.exe` / portable `.zip` |
| macOS | `.dmg` |
| Linux | `.deb` / `.AppImage` |

For the Windows portable edition, extract the zip and run `NyaTerm.exe`. **Help → Check Updates** uses the same Cloudflare R2 update manifest and download source as the installed edition. Tauri updater signatures are always verified before staging an update; NyaTerm preserves the complete `data/` folder while replacing the program files and restarting.

### macOS

macOS users can install NyaTerm with Homebrew:

```bash
brew install nyakang/nyaterm/nyaterm
```

This uses the [`nyakang/homebrew-nyaterm`](https://github.com/nyakang/homebrew-nyaterm) tap and installs the `nyaterm` cask. You can also download the `.dmg` installer from [nyaterm.app](https://nyaterm.app) or [Releases](https://github.com/nyakang/nyaterm/releases), then drag NyaTerm into `/Applications`.

NyaTerm is currently not signed with an Apple Developer certificate. If macOS reports that the app is damaged or cannot be opened after installation, remove the quarantine attribute and open it again:

```bash
sudo xattr -cr /Applications/NyaTerm.app
```

### Arch Linux / AUR

Arch Linux users can install NyaTerm from the AUR:

```bash
yay -S nyaterm-bin
```

Or with `paru`:

```bash
paru -S nyaterm-bin
```

You can also build it manually from the AUR repository:

```bash
git clone https://aur.archlinux.org/nyaterm-bin.git
cd nyaterm-bin
makepkg -si
```

AUR package: [`nyaterm-bin`](https://aur.archlinux.org/packages/nyaterm-bin)

> `nyaterm-bin` is a community-maintained binary AUR package. If the AUR package has not yet caught up with the latest release, download the official package from [Releases](https://github.com/nyakang/nyaterm/releases).


## Prerequisites for Development

- Node.js 18+
- Rust stable via [rustup](https://rustup.rs/)
- pnpm

## Development

```bash
git clone https://github.com/nyakang/nyaterm.git
cd nyaterm
pnpm install
pnpm tauri dev
```

## Project Structure

```text
├── src/                    # React frontend
│   ├── components/         # UI, terminal, panels, dialogs, settings
│   ├── hooks/              # Frontend state and workflow hooks
│   ├── lib/                # Terminal, AI, sync, theme, platform helpers
│   ├── pages/              # Child-window pages
│   └── i18n/               # Application translations
├── src-tauri/              # Tauri 2 + Rust backend
│   ├── src/cmd/            # Tauri commands exposed to the frontend
│   ├── src/core/           # SSH, SFTP, PTY, Telnet, Serial, AI, backup logic
│   ├── src/config/         # Persistent config models
│   └── crates/otp/         # Local OTP implementation
├── docs-site/              # Docusaurus documentation site
├── public/                 # Static assets
└── scripts/                # Checks, version sync, and demo helper scripts
```

---

<a name="credits"></a>
# Credits
Thanks to the following projects and libraries that make NyaTerm possible:
- [WindTerm](https://github.com/kingToolbox/WindTerm) - Inspired the design and features of NyaTerm
- [tabby](https://github.com/Eugeny/tabby) - An excellent cross-platform terminal that provided many design inspirations
- [xterm.js](https://xtermjs.org/) - A powerful frontend terminal emulator that provides rich terminal functionality and extensibility
- [russh](https://github.com/warp-tech/russh) - An SSH client and server implementation in Rust

---

<a name="sponsor"></a>
# Sponsor

If NyaTerm helps your daily development, operations, or remote terminal workflow, you can support ongoing maintenance from the [Sponsor page](./docs-site/i18n/en/docusaurus-plugin-content-docs/current/sponsor.md).

---

<a name="contributors"></a>
# Contributors

<a href="https://github.com/nyakang/nyaterm/graphs/contributors">
  <img src="https://contrib-wall.coderkang.workers.dev/image?snapshot=EEZNlyhjBOrARLki-MUhXqKk&amp;sealed_token=eyJ2IjoxLCJ0eXBlIjoic25hcHNob3QiLCJzbmFwc2hvdCI6IkVFWk5seWhqQk9yQVJMa2ktTVVoWHFLayIsImV4cCI6MTgxNTAzNjQ0NH0.UwegiwfiT3KSKQ17n5ivt24EFwAtoaHZGRwx97t0ODA&amp;sealed_github_token=A6pS93BW-53hz_Cs.8rPY0yM6koNS_UowTbu8qkWzmPLRco8HJRJO2_yp5MZH9yz9o1Ob2g5MUL4aLFK24gjbDexjdErw6eR72ruRbNcsFciYuM4DghLZJgi9V5D9f1-_xFT-sGYK6ibc1A4aWz8EyvFIQo0byPAXv-d410cJmhwsH-bqc963L8J-z0kU6oIi-2zCSszxllze23WmJsWnTJBFUBrQbllg5cLwuHm6q-BMkjoVxChLdOMVwEs" alt="Contributors" />
</a>

---

## Star History

<a href="https://www.star-history.com/?repos=nyakang%2Fnyaterm&type=date&legend=top-left">
 <picture>
   <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/chart?repos=nyakang/nyaterm&type=date&theme=dark&legend=top-left&sealed_token=B9FcRzP_4KoFVgTzgY_L9J4F1huRnhx9N962VMQBHjFsF-VtiApJNnKYWg5IaJDZtDm7iCd3epIU3uEZTSG2XVoU8jVssH-ge-gPjPGq5cRz9xtF2N_piA" />
   <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/chart?repos=nyakang/nyaterm&type=date&legend=top-left&sealed_token=B9FcRzP_4KoFVgTzgY_L9J4F1huRnhx9N962VMQBHjFsF-VtiApJNnKYWg5IaJDZtDm7iCd3epIU3uEZTSG2XVoU8jVssH-ge-gPjPGq5cRz9xtF2N_piA" />
   <img alt="Star History Chart" src="https://api.star-history.com/chart?repos=nyakang/nyaterm&type=date&legend=top-left&sealed_token=B9FcRzP_4KoFVgTzgY_L9J4F1huRnhx9N962VMQBHjFsF-VtiApJNnKYWg5IaJDZtDm7iCd3epIU3uEZTSG2XVoU8jVssH-ge-gPjPGq5cRz9xtF2N_piA" />
 </picture>
</a>

---

<a name="license"></a>
# License

This project is licensed under the [MIT License](LICENSE).
