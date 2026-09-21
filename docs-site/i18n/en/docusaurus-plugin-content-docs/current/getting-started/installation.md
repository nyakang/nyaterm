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

For the Windows portable edition, extract the zip and run `NyaTerm.exe`; configuration lives in the adjacent `data/` directory.

**Help → Check Updates** only checks GitHub Releases and opens the Releases page — it does not download or replace program files. To update a portable build, close NyaTerm, replace the program files manually, and keep `data/`.

An installed Tauri release can use its existing signed updater once to migrate to the GPUI installer. The old Tauri portable updater only accepts one application executable and cannot carry the RDP/VNC helper processes required by GPUI. Download the complete GPUI portable zip, copy the old `data/` directory into the new directory, and do not replace only `NyaTerm.exe`.

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

### Nix / NixOS

You can run or install NyaTerm using Nix Flakes:

```bash
# Run directly
nix run github:nyakang/nyaterm

# Install into user profile
nix profile install github:nyakang/nyaterm
```

Or build from the local repository:

```bash
nix build
```

### Build from source

If you prefer to build NyaTerm yourself, see [Development Setup](../development/setup).

## Migrating an existing environment

If you maintained sessions in another client, you can import from Xshell, MobaXterm, WindTerm, SecureCRT, FinalShell, Termius, or NyaTerm / Electerm JSON after installing. The full format list and import caveats are in [SSH Connections → Importing sessions from other clients](../guide/ssh-connection#import-sessions-from-other-clients).

To restore a complete NyaTerm environment, use a `.nya` encrypted configuration backup rather than session import — it restores more than the connection list. `.nya` import and export require a master password set in **Settings → Security** first, and importing usually needs an application restart.

## Next steps

Continue with [Quick Start](./quick-start), which walks through creating your first connection, understanding the workspace, and which settings are worth reviewing early.
