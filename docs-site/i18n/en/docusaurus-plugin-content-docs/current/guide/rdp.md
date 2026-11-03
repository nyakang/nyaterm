# RDP Remote Desktop

An RDP session connects to a Windows host or any other remote desktop serving RDP. It shares tabs, split panes, and the saved-connection system with terminal sessions, but it carries a graphical framebuffer rather than a text terminal.

RDP protocol decoding runs in a separate helper process. When running from source you must build the helper first, or connections fail with `HelperMissing` — see [Development Setup](../development/setup#rdp--vnc-need-their-helpers-built-first).

## Connection configuration

### Basics

| Field | Description |
|-------|-------------|
| Host / Port | RDP service address; default port `3389` |
| Username | Login user |
| Domain | Active Directory domain; leave empty for standalone hosts |
| Password | Login password |

### Security

**Network Level Authentication** (NLA / CredSSP) is used when the remote host supports it.

**Certificate policy** decides what happens on first contact with an unknown certificate:

| Policy | Behavior |
|--------|----------|
| Prompt on unknown certificate | Opens a verification dialog and lets you decide (default) |
| Reject untrusted certificates | Refuses without asking |
| Accept for this session only | Accepts without recording trust, so the next connection asks again |

With **Prompt on unknown certificate**, the dialog can accept just this connection or accept and remember the certificate. If a remembered certificate later changes, you are prompted again before connecting.

### Display

**Display mode** is either **Fit window** (default) or **Fixed size**. Fixed size requires explicit dimensions:

- Width from **640 to 7680**
- Height from **480 to 4320**

The default resolution is 1920×1080 at 32-bit color depth.

### Clipboard

Clipboard sharing is always enabled and has no setting. Builtin supports bidirectional text and file/directory copies; image and rich-text channels are not implemented. Windows Native uses Microsoft's full clipboard redirection, with formats and file transfers subject to the installed control and server policy.

Older connections, backups, and restored sessions still accept saved disabled/text-only values, but those values no longer restrict clipboard sharing at runtime. Clipboard sharing does not enable drive, printer, or device redirection.

### Reconnect

**Auto reconnect** is on by default and retries a limited number of times on transient network failures. **Retry attempts** accepts **0 to 20**, defaulting to 5.

## Capability boundaries

RDP sessions do not offer these SSH-only capabilities:

- Terminal command history and command suggestions
- The SFTP file explorer
- SSH proxy, jump hosts, and tunnels
- Remote host monitoring panels

Use SSH, Local Terminal, Telnet, or Serial sessions when you need command-line capabilities.

## Error kinds

The error kind on a failed connection narrows down where to look:

| Kind | Common cause |
|------|--------------|
| `Authentication` | Wrong username, password, or domain; NLA requirements unmet |
| `CertificateRejected` | The certificate policy refused the server certificate |
| `ConnectionRefused` / `Timeout` | Port unreachable, firewall block, or RDP not enabled on the host |
| `Tls` / `Negotiation` | TLS handshake or protocol negotiation failed |
| `HelperMissing` | The helper executable is not beside the application |
| `HelperCrashed` | The helper process exited during a live session |
