# Session Types

NyaTerm supports six terminal and remote-desktop session types in one workspace:

- **SSH**
- **Local Terminal**
- **Telnet**
- **Serial**
- **RDP**
- **VNC**

## At a glance

| Session Type | Typical scenario | Key capabilities |
|--------------|------------------|------------------|
| SSH | Remote Linux / Unix administration | SFTP, OTP, resource / NVIDIA GPU / Ascend NPU / process / Docker monitoring, proxy, jump host, tunnels, algorithm preferences |
| Local Terminal | Local shell work, scripts, builds | Shared terminal UI, command history, split panes |
| Telnet | Legacy devices, lab environments, compatibility troubleshooting | Terminal workspace features with `Backspace Mode`, but not SSH-only features |
| Serial | Routers, switches, boards, embedded debug ports | Serial port settings, `Backspace Mode`, and terminal workspace features |
| RDP | Windows Remote Desktop or graphical administration entry points | Remote desktop display, NLA/CredSSP, certificate verification, text clipboard, window fitting, and reconnects |
| VNC | Raw TCP VNC services, VM consoles, lightweight graphical remote desktops | Raw / ZRLE / Tight / Tight JPEG display, None / VNC Auth, window scaling, text clipboard, and reconnects |

## SSH

SSH is the most capable session type in NyaTerm. It is the best fit when you need to:

- Log in to remote Linux / Unix hosts
- Browse and transfer remote files
- Use OTP, jump hosts, or proxies
- Watch remote resource, NVIDIA GPU, Ascend NPU, process, and Docker monitoring
- Configure port tunnels
- Fine-tune negotiated SSH algorithms

## Local Terminal

Local Terminal puts your local shell workflow inside the same NyaTerm workspace, for example:

- Running frontend or Rust builds locally
- Running scripts, reading logs, or using Git
- Comparing local and remote output side by side

Local Terminal supports these workspace features:

- Tabs
- Split panes
- Terminal search
- Command history and suggestions
- Optional line numbers, timestamps, and highlighting

When creating a local terminal, you can also choose:

- The shell path, such as `powershell.exe`, `cmd.exe`, `bash`, or `wsl.exe`
- The working directory

## Telnet

Telnet sessions are for:

- Maintaining older equipment
- Lab environments
- Compatibility scenarios where SSH is not available

For devices that expect specific erase behavior, Telnet also exposes `Backspace Mode` so you can choose `Ctrl+H (BS)` or `DEL (0x7F)`.

## Serial

Serial sessions connect to:

- Network device console ports
- Routers and switches
- Development boards, embedded devices, and debug ports

When creating a serial session, you can configure:

- Port
- Baud rate
- Data bits
- Parity
- Stop bits
- `Backspace Mode`

Serial sessions still live inside NyaTerm's tabbed and split workspace, so you can watch serial output in one pane while running commands in an SSH or local terminal pane.

## RDP

An RDP session connects to a Windows host or another remote desktop serving RDP. Remote desktops support tabs, split panes, and saved connections.

Configuration, certificate policies, and error kinds are documented in [RDP Remote Desktop](./rdp).

## VNC

A VNC session connects to virtual machine consoles, lab environments, or lightweight graphical desktops serving RFB, sharing the remote desktop pane and the same capability boundaries as RDP.

Transport is currently direct TCP only, with no TLS / VeNCrypt. Configuration, encoding support, and interoperability status are documented in [VNC Remote Desktop](./vnc).

## How to choose

A simple rule of thumb:

- Need the full remote workflow? Use **SSH**
- Need a local shell? Use **Local Terminal**
- Need a traditional remote terminal? Use **Telnet**
- Need a device console or debug port? Use **Serial**
- Need a graphical Windows remote desktop? Use **RDP**
- Need a VNC / VM console graphical desktop? Use **VNC**

## Mix them in one workspace

One of NyaTerm's strengths is that you can mix these session types in the same workspace, for example:

- SSH on the left to watch remote logs
- Local Terminal on the right to run packaging or Git commands
- A Serial tab open to watch device boot output
- An RDP pane open to inspect a Windows remote desktop
- A VNC pane open to operate a VM console

That is why some features are documented as session-specific. The workspace is shared, but the capability boundary still depends on the underlying session type.
