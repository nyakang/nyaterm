# Test Connection: environments still needed for live verification

The connection editor's **Test Connection** action exercises the real connection
path without saving the connection or creating a tab. This page lists the
authentication paths that are implemented but still need a live environment to
verify end to end.

> Do not put real passwords, private keys, OTP secrets, or tokens in this file
> or in a commit. Provide them out of band: a local test instance, a CI secret,
> or a temporary test server.

## Already verified automatically or in this workspace

| Area | Coverage |
|---|---|
| SSH password | Live against `test.rebex.net` |
| SSH private key | Live against a local asyncssh server (`PUBLICKEY ... ok=True`) |
| SSH host-key trust | Test-only acceptance, not persisted; prompt appears on every test |
| FTP | Live against `test.rebex.net` (plain FTP) |
| FTP anonymous | GUI test against a local pyftpdlib server |
| Telnet | Reachability against `telehack.com` |
| Telnet auto-login | Local fake Telnet server (unit/integration) |
| VNC None / VncAuth | Live against a local RFB server with an independent DES implementation |
| VNC helper path | Live against a local VncAuth server through `nyaterm-vnc-helper` |
| VNC routed | Probe supports the real network-route relay (not live-proxied) |
| RDP NLA failure | Live against the local RDP server (`STATUS_LOGON_FAILURE`) |
| RDP routed | Probe supports the real network-route relay (not live-proxied) |
| Serial | Open/close probe (no serial port available here) |

## 1. SSH Agent

**Need:** an SSH server plus an agent endpoint the test process can reach.

- Windows: enable the OpenSSH Authentication Agent service and add a key:
  ```powershell
  Set-Service ssh-agent -StartupType Manual
  Start-Service ssh-agent
  ssh-add $env:USERPROFILE\.ssh\id_rsa
  ```
- Linux/macOS: export `SSH_AUTH_SOCK` with at least one key loaded.
- Pageant: run Pageant with a key loaded.

**Provide:** host, port, username, and the agent endpoint type
(`WindowsOpenSsh`, `Environment { variable }`, `Pageant`, or a Unix socket path).

**Verify:** SSH → Authentication = Agent → Test Connection → “SSH 认证成功”.

## 2. SSH encrypted private key / passphrase

**Need:** an SSH server that accepts the key, plus an encrypted private key.

**Provide:** host, port, username, the key (or a test key registered on the
server), and whether the passphrase is stored with the key or should be prompted.

**Verify:** SSH → Authentication = Private key → Test Connection; enter the
passphrase if prompted.

## 3. SSH keyboard-interactive / OTP / MFA

**Need:** an SSH server that requires keyboard-interactive, TOTP, or a second
factor.

**Provide:** host, port, username, password, and either a TOTP secret enrolled
in the connection or an account that accepts a one-time code.

**Verify:** SSH → Authentication = Password → Test Connection; complete the
keyboard-interactive prompt.

## 4. SSH certificate authentication

**Need:** an SSH server that trusts a user certificate, plus the private key and
certificate pair.

**Provide:** host, port, username, private key, and certificate.

**Verify:** SSH → Private key (with certificate) → Test Connection.

## 5. FTPS (explicit / implicit TLS)

**Need:** an FTPS server with a certificate the test machine can validate, or a
documented self-signed setup.

**Provide:** host, ports, security mode (`explicit` or `implicit`), username,
password, and whether a custom CA is required.

**Verify:** FTP → security mode → Test Connection.

## 6. Telnet auto-login on a real device

**Need:** a real Telnet device or server with username/password prompts and a
recognizable success prompt (router, switch, lab appliance, or a persistent
public Telnet service with credentials).

**Provide:** host, port, username, password, and the prompt patterns if they are
non-standard.

**Verify:** Telnet → enable Auto Login → Test Connection → “Telnet 认证成功”.

## 7. RDP

**Need:** an RDP server with valid credentials. Separate environments are useful
for:

- NLA / CredSSP success
- NLA disabled (standard RDP security)
- `Strict` / `RejectChanged` certificate policy
- Windows Native backend

**Provide:** host, port, domain, username, password, NLA on/off, certificate
policy, and backend (`Builtin` or Windows Native).

**Verify:** RDP → Test Connection.

## 8. VNC RA2 / VeNCrypt / Tight

**Need:** a VNC server configured for one of these security types:

- Apple Remote Desktop (RA2, security type 30)
- VeNCrypt (TLS + username/password)
- TightVNC security

**Provide:** host, port, security mode, username/password if applicable, and the
server key/fingerprint policy (for RA2).

**Verify:** VNC → Test Connection. The lightweight probe falls back to the
isolated VNC helper for these security types.

## 9. SPICE

**Need:** a SPICE server (for example a QEMU VM with a SPICE display) with
ticket authentication, and optionally TLS. The helper is a thin wrapper around a
native SPICE client runtime; this workspace currently lacks that native
component, so the probe stops with `RuntimeMissing`.

**Provide:** host, plaintext port, TLS port if used, username, password, TLS
policy, whether the server uses a self-signed certificate, and the native SPICE
component/runtime package for this platform.

**Verify:** SPICE → Test Connection → “SPICE 认证成功”.

## 10. ADB device authorization

**Need:** an Android device or emulator over USB or wireless ADB, either already
authorized or ready to show the RSA authorization prompt.

**Provide:** device connection details and whether the device should be
authorized, unauthorized, or offline.

**Verify:** currently the ADB test action is disabled; this needs a separate
design decision for what “test” means for an ADB connection.

## 11. Proxy / jump routes

**Need:** an HTTP CONNECT or SOCKS5 proxy, and an SSH jump host, ideally with
authentication enabled.

**Provide:** proxy protocol/host/port/credentials, jump host credentials, and a
target reachable only through that route.

**Verify:** configure Network & Proxy on an SSH/Telnet/VNC/RDP connection and
run Test Connection. The test must fail when the proxy/jump credentials are
wrong.

## 12. SFTP subsystem

**Need:** an SSH server that authenticates but denies the SFTP subsystem.

**Provide:** host, port, username, password/key, and confirmation that the SFTP
subsystem is disabled.

**Verify:** SSH Test Connection currently reports authentication success; it
does not open the SFTP subsystem. This is a known limitation.
