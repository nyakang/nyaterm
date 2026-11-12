# VNC Remote Desktop

A VNC session connects to virtual machine consoles, lab environments, or lightweight graphical desktops serving RFB. It shares the remote desktop pane with RDP, along with saved connections, recents, tabs, and split panes.

VNC protocol decoding runs in a separate helper process. When running from source you must build the helper first, or connections fail with `HelperMissing` — see [Development Setup](../development/setup#rdp--vnc-need-their-helpers-built-first).

## Connection configuration

### Basics

| Field | Description |
|-------|-------------|
| Host / Port | RFB service address; default port `5900` |
| VNC password | Password used by classic VNC Authentication |

### Security mode

| Mode | Behavior |
|------|----------|
| Auto | Negotiates from the security types the server advertises (default) |
| None | No authentication |
| VNC password | Classic VNC Authentication |

Classic VNC Authentication only uses the **first 8 bytes** of the password. NyaTerm rejects passwords longer than 8 bytes.

### Display

**Scale** is **Fit** (default), **Actual size**, or **Stretch**.

### Clipboard

**Clipboard** is on by default and syncs Latin-1 text. The limits come from the RFB protocol itself:

- Latin-1 text only; non-Latin-1 characters are rejected
- At most 1 MiB per transfer

This limit is deliberate — it keeps binary or oversized content out of the VNC protocol path.

### Session behavior

- **Shared session** — lets other viewers stay connected instead of disconnecting them
- **View only** — disables keyboard and pointer input

Both gates are enforced in the helper process, not merely hidden in the UI.

### Reconnect

**Reconnect** is on by default and retries a limited number of times on transient transport failures, defaulting to 5. The connect timeout is 15 seconds and the handshake timeout is 30 seconds.

## Transport and encoding support

Current boundaries:

- Transport is direct TCP only — **no TLS / VeNCrypt** — and cannot be carried over a proxy or SSH tunnel
- Encodings are advertised in the order `DesktopSizePseudo`, ZRLE, Tight, Raw. Tight JPEG is decoded in the helper into a uniform RGBA framebuffer, and Raw remains the stable fallback
- CopyRect, cursor pseudo-encodings, and remote resize are not supported

## Interoperability status

| Scenario | Security mode | Encodings | Status |
| --- | --- | --- | --- |
| Scripted RFB 3.8 fixture | None | ZRLE / Tight / Tight JPEG → RGBA | Covered by automated tests |
| Scripted RFB 3.8 fixture | VNC password | ZRLE / Tight / Tight JPEG → RGBA | Covered by automated tests |
| TigerVNC | None / VNC password | Raw / ZRLE / Tight / JPEG | Pending real-server validation |
| TightVNC | None / VNC password | Raw / Tight / JPEG | Pending real-server validation |
| x11vnc / LibVNCServer | None / VNC password | Raw / ZRLE / Tight / JPEG | Pending real-server validation |
| QEMU / KVM VNC | None / VNC password | Raw / ZRLE / Tight / JPEG | Pending real-server validation |

Automated coverage is against a scripted RFB 3.8 fixture. Real-server interoperability is still being validated; when reporting a problem, include the server implementation and version.

## Capability boundaries

Like RDP, VNC sessions do not offer terminal command history, the SFTP file explorer, SSH proxy / jump hosts, or remote host monitoring.

## Error kinds

| Kind | Common cause |
|------|--------------|
| `Authentication` | Wrong password, or VNC password mode selected with no password set |
| `Protocol` | The server's RFB version or security type is unsupported |
| `Encoding` | An undecodable encoding arrived, such as an unnegotiated pseudo-encoding |
| `Transport` | Port unreachable, connection refused, or the link dropped |
| `Clipboard` | Clipboard content exceeded the Latin-1 or 1 MiB limit |
| `HelperMissing` | The helper executable is not beside the application |
| `HelperCrashed` | The helper process exited during a live session |
