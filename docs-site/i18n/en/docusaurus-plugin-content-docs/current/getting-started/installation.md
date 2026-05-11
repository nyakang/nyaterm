---
sidebar_position: 1
---

# Installation

## System Requirements

NyaTerm supports the following operating systems:

- **Windows** 10/11 (64-bit)
- **macOS** 12+ (Intel & Apple Silicon)
- **Linux** (Ubuntu 20.04+, Fedora 36+, Arch Linux, and similar distributions)

## Download and install

### From releases

Visit the [Releases](https://github.com/nyakang/nyaterm/releases) page and download the installer for your OS:

| Platform | Format |
|----------|--------|
| Windows | `.msi` / `.exe` |
| macOS | `.dmg` |
| Linux | `.deb` / `.AppImage` |

### Build from source

If you prefer to build NyaTerm yourself, see [Development Setup](../development/setup).

## What you see on first launch

After installation, the main window is typically organized into these areas:

- **Top menu and window bar** — File / View / Help and window controls
- **Central workspace** — terminal tabs and split panes inside the active tab
- **Left activity bar and panels** — file explorer, network, Security/Auth, and related capability entry points
- **Right activity bar and panels** — saved connections, active sessions, command history, and resource monitor
- **Bottom helper area** — quick commands, serial send, recording, and lock actions

Some workflows open dedicated child windows instead of interrupting the main workspace, such as:

- Settings
- New session / connection creation
- Quick command editing
- Auto-upload prompts

## Suggested first run

For a first pass through the app, try this order:

1. Open [Quick Start](./quick-start)
2. Create one **SSH** connection
3. Create one **Local Terminal** to experience the mixed workspace model
4. Open the file explorer and transfer queue in the SSH session
5. Try command history, quick commands, and terminal search
