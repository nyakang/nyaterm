---
sidebar_position: 1
---

# Installation

## System Requirements

NyaTerm supports the following operating systems:

- **Windows** 10 version 1809 (Build 17763) or later / Windows 11 (64-bit)
- **macOS** 12+ (Intel & Apple Silicon)
- **Linux** (Ubuntu 20.04+, Fedora 36+, Arch Linux, and similar distributions)

Local Terminal prefers the Microsoft ConPTY bundled with NyaTerm and falls back to the system ConPTY if loading or creation fails. The system must still provide the ConPTY API, so Windows 10 version 1809 (Build 17763) is the minimum supported release. The About dialog's support information shows the backend actually used.

## Download and install

### From releases

Visit the [Releases](https://github.com/nyakang/nyaterm/releases) page and download the installer for your OS:

| Platform | Format |
|----------|--------|
| Windows | `.msi` / `.exe` / portable `.zip` |
| macOS | `.dmg` |
| Linux | `.deb` / `.AppImage` |

For the Windows portable edition, extract the zip and run `NyaTerm.exe`. **Help → Check Updates** uses the same Cloudflare R2 update manifest and download source as the installed edition. Tauri updater signatures are always verified before staging an update; when restarting, NyaTerm replaces the program files and preserves the complete `data/` folder.

The first upgrade to a portable build with bundled ConPTY requires a manual download because older updaters cannot process the new `conpty/` directory. Replace the program files while keeping your existing `data/` directory. In-app updates work again after that upgrade.

Direct downloads for the Windows portable edition:

- [NyaTerm_1.1.18_windows_x64_portable.zip](https://downloads.nyaterm.app/releases/v1.1.18/NyaTerm_1.1.18_windows_x64_portable.zip) (x64)
- [NyaTerm_1.1.18_windows_arm64_portable.zip](https://downloads.nyaterm.app/releases/v1.1.18/NyaTerm_1.1.18_windows_arm64_portable.zip) (ARM64)

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

### Linux (Nix / NixOS)

If you use Nix or NixOS, you can run or install NyaTerm directly using the repository Flake:

```bash
# Run directly without installation
nix run github:nyakang/nyaterm

# Install into your user profile
nix profile install github:nyakang/nyaterm
```

Notes:
- The Nix build automatically disables built-in auto-update checks; updates are managed through Nix.
- It includes both the main application and the `nyaterm-mcp` sidecar, along with desktop entries and `nyaterm://`, `ssh://`, `telnet://` protocol handlers.
- Runtime libraries (WebKitGTK, GTK3, xdg-utils) are wrapped automatically, with Nixpkgs CA certificates bundle provided via `SSL_CERT_FILE` as a fallback when not overridden by the host.
- If using credential import from tools like Termius, ensure a Secret Service provider (such as gnome-keyring or keepassxc) is available in your desktop environment.
- WebDAV and S3 cloud sync work out of the box. Default builds do not embed a GitHub OAuth Client ID for GitHub Gist sync. To enable GitHub Gist sync in your NixOS / Home Manager configuration:

```nix
# NixOS (configuration.nix)
environment.systemPackages = [
  (inputs.nyaterm.packages.${pkgs.system}.nyaterm.override {
    githubGistClientId = "your_client_id";
  })
];

# Or using the exported overlay in NixOS:
nixpkgs.overlays = [ inputs.nyaterm.overlays.default ];
environment.systemPackages = [
  (pkgs.nyaterm.override {
    githubGistClientId = "your_client_id";
  })
];

# Home Manager (home.nix)
home.packages = [
  (inputs.nyaterm.packages.${pkgs.system}.nyaterm.override {
    githubGistClientId = "your_client_id";
  })
];

# Or using the exported overlay in Home Manager:
nixpkgs.overlays = [ inputs.nyaterm.overlays.default ];
home.packages = [
  (pkgs.nyaterm.override {
    githubGistClientId = "your_client_id";
  })
];
```

### Build from source

If you prefer to build NyaTerm yourself, see [Development Setup](../development/setup).

## What you see on first launch

After installation, the main window is typically organized into these areas:

- **Top menu and window bar** — File / View / Help and window controls
- **Central workspace** — terminal tabs and split panes inside the active tab
- **Left activity bar and panels** — file explorer, network, Security/Auth, Cloud Sync, settings, and related capability entry points
- **Right activity bar and panels** — saved connections, AI Assistant, active sessions, command history, and resource monitor
- **Bottom helper area** — quick commands, serial send, recording, and lock actions

Some workflows open dedicated child windows instead of interrupting the main workspace, such as:

- Settings
- New session / connection creation
- Quick command editing
- Remote-file editing
- Auto-upload prompts

## Settings worth checking after install

Before using NyaTerm long term, quickly review:

- **Settings → General**: startup restore, minimize to tray when closing, close confirmation
- **Settings → General**: log level, log retention, open log directory, export diagnostics bundle
- **Settings → Interaction**: command suggestions, history-command length filters, copy, right-click paste, macOS IME compatibility
- **Settings → Terminal**: scrollback, Keep-Alive, action links, line numbers / timestamps, keyword highlighting, resource monitor, workspace padding, font weight, image path paste behavior
- **Settings → Transfer**: default download directory, default editor, recording path, concurrency, retry, duplicate-target strategy
- **Settings → Security**: master password, screen lock, idle auto-lock, host key policy
- **Settings → AI**: providers, models, risk controls, history, and context limits

If you often keep sessions or sync tasks running in the background, check **Minimize to tray when closing** early.

## Suggested first run

For a first pass through the app, try this order:

1. Open [Quick Start](./quick-start)
2. Create one **SSH** connection
3. Create one **Local Terminal** to experience the mixed workspace model
4. Open the file explorer and transfer queue in the SSH session
5. Try command history, quick commands, AI Assistant, recording, and terminal search
6. On Windows, also try dragging local files or folders into the file explorer for upload
