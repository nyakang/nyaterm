# Installation

## System Requirements

NyaTerm supports the following operating systems:

- **Windows** 10/11 (x64 / ARM64)
- **macOS** 12+ (Intel / Apple Silicon)
- **Linux** (x64 / ARM64; Ubuntu 20.04+, Fedora 36+, Arch Linux, and similar distributions)

NyaTerm renders natively on the GPU through GPUI, so it has graphics requirements:

- **Linux**: a working Vulkan driver (for example `libvulkan1` and `mesa-vulkan-drivers`) plus an X11 or Wayland session. The application will not start without a Vulkan driver
- **macOS**: uses Metal, which ships with the system
- **Windows**: uses the system graphics driver; usually nothing extra is needed

NyaTerm is a desktop client, not a terminal multiplexer, so it is not applicable on a headless SSH-only server.

## Download and install

### From releases

Visit the [Releases](https://github.com/nyakang/nyaterm/releases) page and download the installer for your OS:

| Platform | Format |
|----------|--------|
| Windows | installer `-setup.exe` / portable `.zip` |
| macOS | `.dmg` / `.app.tar.gz` |
| Linux | `.deb` / `.AppImage` / `.rpm` |

For the Windows portable edition, extract the ZIP and run `NyaTerm.exe`; data lives in the adjacent `data/` directory. GPUI **Help → Check Updates** uses signed manifests and supports downloading and installing updates for supported installation types. Homebrew and deb/rpm installations use their package manager.

**Migrating from 1.x: Windows portable and old MSI installations require manual upgrades.** Quit and back up all data and keys first. Replace the complete portable program payload while preserving `data/`; for MSI, back up, uninstall the old version, and install NSIS. Replacing only the main executable is insufficient, and automatic migration is not promised for all legacy installers. See the [2.0 migration guide](./migration-v2).

### Install native components on demand

The main package includes the application and MCP helper. Install SPICE, RDP Builtin, VNC, and Android from the Plugins panel as needed; the Android component includes ADB, the screen helper, and FFmpeg. The first connection asks before downloading, then resumes if the connection request is still valid. Windows Native RDP uses the system component and does not require RDP Builtin.

The Plugins panel supports download progress, cancellation, enabling/disabling, uninstalling, and importing officially signed component ZIPs. For offline installation, enter the ZIP path in the package path field and click the component's import button. Components must support the application's version and target platform. Compatible installed versions remain available after application upgrades; component updates are manual.

When migrating an old portable package to the slim layout, quit the application and back up the complete `data/` directory. Extract the slim package into a new directory and copy the complete `data/` directory into it, leaving old helpers and runtime libraries behind. Reinstall components when first needed. Later slim-package upgrades preserve components and user data.

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

### Build from source

If you prefer to build NyaTerm yourself, see [Development Setup](../development/setup).

## Migrating an existing environment

If you maintained sessions in another client, you can import from Xshell, MobaXterm, WindTerm, SecureCRT, FinalShell, Termius, or NyaTerm / Electerm JSON after installing. The full format list and import caveats are in [SSH Connections → Importing sessions from other clients](../guide/ssh-connection#import-sessions-from-other-clients).

To restore a complete NyaTerm environment, use a `.nya` encrypted configuration backup rather than session import — it restores more than the connection list. `.nya` import and export require a master password set in **Settings → Security** first, and importing usually needs an application restart.

## Next steps

Continue with [Quick Start](./quick-start), which walks through creating your first connection, understanding the workspace, and which settings are worth reviewing early.
