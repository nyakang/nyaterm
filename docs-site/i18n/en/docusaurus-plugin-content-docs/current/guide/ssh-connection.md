# SSH Connection Management

SSH is still NyaTerm's most complete session type. Beyond a basic login, an SSH connection can also be tied to:

- SFTP file explorer
- Remote resource, GPU, process, and Docker monitoring
- Proxy
- Jump host
- OTP binding and auto-fill
- Port tunnels
- SSH algorithm preferences

If you are new to NyaTerm, it usually makes sense to configure SSH first, then expand into file workflows, terminal enhancements, and network features.

## Create an SSH connection

In the **New Session** window, switch to the **SSH** tab and fill in these fields.

### Basic information

| Field | Description |
|------|------|
| Connection Name | Display name in the saved-connections list |
| Host | Server IP or domain |
| Port | Defaults to `22` |
| Username | Login user |
| Icon | Helps distinguish services or environments |
| Group | Organizes connections into folders |
| Description | Notes about the environment or purpose |

### Authentication methods

NyaTerm supports four SSH authentication methods:

- **Password**
- **Private key**
- **No authentication (none)**
- **SSH Agent**

You can select saved passwords or saved keys instead of re-entering them every time.

**No authentication (none)** is for hosts that complete authentication by other means (for example some jump-host or gateway flows, or servers that accept an empty auth). Only use it when the target host genuinely requires no SSH-level credential.

Password authentication covers temporary test hosts, environments that have not issued private keys yet, and accounts combined with OTP. Private key authentication covers daily operations work, reusing one identity across many hosts, and workflows involving jump hosts or automation.

Both passwords and keys are managed centrally in **Security/Auth**.

#### SSH Agent authentication

SSH Agent mode only uses signing provided by the local Agent; private keys and hardware-key material are never imported into NyaTerm. The authentication section selects one Agent endpoint. Endpoint options are filtered for the current device: macOS/Linux provide automatic discovery, an environment variable, and a Unix domain socket, while Windows provides automatic discovery, Pageant, and the Windows OpenSSH Agent. `Auto` uses the platform default Agent. Connections fail with a clear error when the Agent is unavailable or has no usable identity.

The Agent endpoint and forwarding switch are device-local connection settings. Cross-device sync does not overwrite these values on the destination device, so a macOS Unix socket path is never applied to Windows.

When the Agent is waiting for a hardware touch, PIN, or desktop approval, NyaTerm shows a confirmation dialog. If the Agent times out or authentication fails, **Retry** discards the current attempt and rebuilds the complete SSH/jump-host chain. **Cancel** terminates the connection attempt.

### Interactive authentication requests

When a server asks for additional keyboard-interactive input, OTP, or a restarted authentication step, NyaTerm collects the information through a dedicated SSH authentication request window instead of mixing every prompt into terminal output. The window distinguishes:

- Normal password / private-key authentication
- Keyboard-interactive authentication
- OTP or second-factor prompts
- Authentication flows that need to restart

If an authentication request comes from an unexpected host or session, verify the connection details before entering sensitive information.

### SSH profile and terminal type

The SSH form includes **Profile** and **Terminal Type** settings.

**Standard server** is for regular Linux / Unix shells. It keeps SFTP browsing, directory tracking, shell detection, shell integration, remote stats, and automatic icon detection available.

**Network device** is for switches, routers, and other device CLIs that are not Linux shells. At runtime, NyaTerm disables SFTP browsing, directory tracking, shell detection, shell integration, remote stats, and automatic icon detection so the device CLI is not probed like a full shell. This does not rewrite your saved SFTP choices.

Terminal Type controls the `$TERM` value declared to the remote SSH session. You can choose `xterm-256color`, `xterm`, `vt100`, `vt220`, `ansi`, or `linux`. If an older device renders strangely, try a more conservative terminal type.

## Advanced configuration

The advanced section is where an SSH connection goes from "can connect" to "fits a real daily workflow."

### Proxy

If the connection must go through a proxy, you can select a saved proxy profile.

Supported proxy types:

- **SOCKS5**
- **HTTP**
- **ProxyCommand**

A proxy record can store:

- Name
- Protocol
- Host
- Port
- Username / password

### SSH Agent forwarding

The **SSH Agent** tab in advanced configuration controls forwarding independently. When it is disabled, NyaTerm does not create a local Agent connection for forwarding and does not send an agent-forwarding request to the server. An SSH Agent authentication connection, when selected as the authentication method, still uses the Agent for authentication. When forwarding is enabled, only interactive terminal sessions request it; SFTP, tunnels, and jump-host transport connections do not implicitly enable local Agent forwarding.

Forwarding endpoints are independent from the login authentication endpoint. You can add multiple external SSH Agent endpoints in order, for example a primary SSH Agent and a gpg-agent SSH-compatible socket. The login authentication endpoint is never added to forwarding automatically. Forwarding sources are controlled only by the external SSH Agent list and the NyaTerm stored-key switch, and both source types use the same fingerprint allowlist or AllowAll policy.

The default policy is a fingerprint allowlist; an empty allowlist exposes no identities. Switching to AllowAll requires an explicit risk confirmation and exposes current and future identities from the enabled sources. If one endpoint is unavailable, the identity picker reports a local endpoint error while preserving successful results from other endpoints. Identities are merged in endpoint order and bounded by the SSH Agent protocol response limits of 1,024 identities and 256 KiB; when a limit is reached, the picker explicitly reports that only the deterministic prefix is shown and forwarded.

Established stored-key forwarding channels are invalidated after a saved key is successfully added, replaced, or deleted; a new channel reads the current key set. Broker and legacy raw-relay channels share a bounded local channel quota, and Broker channels have first-frame and idle timeouts. When a Backup is restored across operating systems, unsupported device-specific Agent endpoints are removed while malformed values still fail validation.

:::warning
Agent forwarding allows remote processes to use the signing capability of selected external Agents or NyaTerm stored keys through SSH. Enable it only for trusted servers and keep it disabled when it is not needed. The Agent endpoint and forwarding policy are device-local connection settings; external hardware keys are never imported, while NyaTerm stored-key synchronization continues to follow the application's existing encrypted snapshot/sync policy.
:::

### Jump host

If the target host is not directly reachable, you can pick another saved SSH connection as the **jump host**.

Typical cases include:

- Connecting through a bastion host
- Reaching internal production hosts
- Multi-hop SSH login chains

NyaTerm validates jump-host chains and rejects missing or cyclic references, so a saved connection cannot recurse forever through its own proxy path.

### Post-login command

You can configure a command that NyaTerm runs automatically once the SSH session is established and the shell is ready.

Common uses:

- Switching to a fixed working directory (`cd`)
- Activating an environment (for example tmux/screen, conda)
- Running a fixed setup command on connect

The command is sent as terminal input after login, so it behaves exactly like typing the command yourself.

### X11 forwarding

X11 Forwarding allows remote graphical applications to display on your local machine through SSH.

NyaTerm does not include an X server. You need to install and start one:

- Windows: VcXsrv or Xming
- macOS: XQuartz
- Linux: Xorg or Xwayland

Remote server requirements:

- `sshd_config`: `X11Forwarding yes`
- `xauth` installed

If you need to override the local DISPLAY value, set **Settings → Terminal → Local X11 DISPLAY**. Common values are `localhost:0` on Windows and `:0` on Linux/macOS.

### SSH algorithm preferences

You can control, per connection, which encryption algorithms are negotiated during the SSH handshake. This lives in the **SSH Algorithms** card under Advanced Configuration and offers three modes:

- **Compatible (default)**: uses a broad algorithm list including some legacy entries, so connections to older servers or network gear still succeed
- **Secure**: uses only modern algorithms and excludes legacy fallbacks
- **Custom**: pick and reorder algorithms across four categories — **key exchange / ciphers / MACs / host keys**

Each algorithm carries a risk label:

- **Modern** (green)
- **Legacy** (amber, e.g. `ssh-rsa`, `*-cbc`)
- **Insecure** (red, e.g. `3des-cbc`, `hmac-sha1`, `ssh-dss`)

When you save a custom configuration, NyaTerm validates that each category is non-empty and rejects unknown algorithm names. If you are unsure what to pick, keep **Compatible** mode; for security-sensitive environments, use **Secure** or a custom, tightened list.

### Multiplexed SSH sessions

NyaTerm can multiplex multiple terminal sessions over a single SSH connection. Opening additional terminals to the same host reuses the existing authenticated connection instead of re-authenticating each time.

Benefits:

- New terminals open faster
- Fewer auth prompts / OTP entries
- Lighter load on the server

### OTP binding

If the environment requires a second-factor code, you can bind an OTP entry to the SSH connection.

After binding, you can either:

- Quickly inspect the code during login
- Enable **auto-fill OTP** for compatible interactive prompts

This works well together with [OTP & Authentication](./otp-and-auth).

## Manage saved connections

After saving, the connection appears in the **Saved Connections** panel on the right.

Common operations include:

- Double-click to connect
- Organize by group
- Edit an existing connection
- Duplicate a connection as a template
- Reconnect from an existing saved source

If you manage many hosts, using groups, icons, and descriptions helps separate environments, projects, and roles.

## Temporary SSH links

If you just want a one-off connection without saving it, use a **temporary SSH link**. Open the dialog from the Saved Connections panel (also reachable via a global keyboard shortcut) and paste an address to start a throwaway session.

Two input forms are supported:

- `ssh://user@host:port` URLs
- `ssh [-p port] [-l user] user@host` command strings

Conventions and limits:

- Default username is `root` and default port is `22`
- Uses password authentication, with no proxy, jump host, post-login command, or X11
- For safety, inline passwords (`user:pass@host`) are rejected in every URL and command form; unsupported options such as `-J`, `-L/-R/-D`, `-i`, and `-o ProxyJump/ProxyCommand` are also rejected

A temporary session never becomes a saved connection: NyaTerm strips the connection ID, proxy, jump host, post-login command, X11, and algorithm preferences, so it stays a one-off session.

## External invocation and protocol links

Open SSH, Telnet, or local terminals from browsers, scripts, and launchers. External connections reuse the most recently used workspace, creating a window when none is available. Launching without arguments opens a new window; use `--reuse-window` to explicitly reuse one.

```powershell
NyaTerm.exe ssh://root@example.com:22
NyaTerm.exe --reuse-window
NyaTerm.exe --local --cwd "C:\中文 目录"
NyaTerm.exe --local "--cwd=C:\My Projects"
```

Stable installers declare the `nyaterm`, `ssh`, and `telnet` protocols. Preview installers declare only `nyaterm-preview`. Your OS may ask you to choose a default handler. Uninstall removes a registration only while it still points to that installation. Portable packages do not register protocols automatically.

Supported examples (replace the custom prefix with `nyaterm-preview://` for Preview):

- `ssh://user@host:port?mode=terminal`
- `ssh://user:password@host:port?mode=sftp`
- `telnet://host:port`
- `nyaterm://connect/ssh?host=host&port=22&username=user&mode=standard`
- `nyaterm://connect/telnet?host=host&port=23`
- `nyaterm://connect/local?cwd=C%3A%5CMy%20Projects`

SSH defaults to username `root` and port `22`; Telnet defaults to port `23`. IPv6 is supported, for example `ssh://user@[2001:db8::1]:2222`. Scheme names are case insensitive. Percent-encode usernames, passwords, and query values as UTF-8; they are decoded exactly once (for example `user%40realm`, `p%40ssword`). Both `--cwd PATH` and `--cwd=PATH` require `--local`. Directories must be nonempty; the background session worker checks existence and accessibility and never treats the path as a startup command.

SSH supports these optional modes:

| Mode | Behavior |
| --- | --- |
| `standard` | Ordinary SSH, falling back to a file session when the server rejects a shell but supports SFTP. |
| `terminal` | Terminal only, without SFTP, directory following, or remote monitoring. Shell rejection fails the connection. |
| `sftp` | Authenticate and verify SFTP, then show the file browser. No PTY/shell, post-login command, terminal AI execution, X11, agent forwarding, or remote monitoring. Saved connections with SFTP disabled report an error. |

Omitting the mode uses ordinary SSH. Overrides apply only to the current session. Reconnect, duplication, moves between windows, and saved-connection startup restoration preserve the mode without changing the saved connection. Old restore data defaults to ordinary SSH; unknown modes are retained and skipped during restoration.

Links without passwords first match saved connections by protocol, normalized host, and port. Explicit SSH usernames must match exactly. A unique match opens directly; multiple matches show a picker for a saved connection, temporary connection, or cancellation. Enabled saved post-login commands still require confirmation; SFTP mode does not execute them. Cancellation ends only the current request.

One-time passwords are allowed only in `ssh://` user information. Such links always create temporary sessions and never match or change saved credentials. The password remains in memory for reconnects in the current process and is excluded from restoration, backups, and sync. Authenticate again after restarting the app. Calling tools and browsers may retain links in history, so treat password-bearing links as sensitive.

Links cannot supply startup commands, proxies, jump hosts, private keys, or forwarding configuration. Unknown/duplicate parameters, invalid ports, and malformed encoding are rejected. Batches are processed independently so invalid entries do not block valid ones, with at most 16 connection intents and 4 KiB per argument. Duplicate OS deliveries are suppressed within 500ms. Requests wait for startup, restoration, unlocking, and existing dialogs; closing a window cancels its remaining requests.

## Session input synchronization

When you need to run the same operation on several hosts at once, use a **synchronized input group** to broadcast what you type in one terminal to multiple sessions. It is not SSH-only — serial sessions can join too.

Configuration, filtering, broadcast control, and the send command panel are documented in [Layout & Workspace → Synchronized input groups](./layout-and-workspace#synchronized-input-groups).

## Import sessions from other clients

NyaTerm can import session definitions from other terminal clients. Current supported imports are:

- **Xshell** (`.xts`)
- **MobaXterm** (`.mxtsessions`)
- **WindTerm** (`.sessions`)
- **SecureCRT** (`.xml`)
- **FinalShell** (`conn` directory)
- **Termius** (local IndexedDB)
- **NyaTerm JSON** (`.json`)

### Import from Termius

Choose **Termius** to read the local Termius IndexedDB and decrypt session data with `Termius/localKey` from the system Credential Manager, Keychain, or Secret Service.

The importer can bring over SSH hosts, groups, usernames, passwords, and SSH private keys. To avoid partially trusted configuration, any Termius encrypted field that cannot be decrypted fails the whole import before anything is written.

If the default path is not found, select the Termius `file__0.indexeddb.leveldb` directory manually.

### Import from NyaTerm JSON

Choose **NyaTerm JSON** to import a `.json` file, for session data generated from scripts, asset inventories, or other systems.

Sample file: [session-import-sample.json](/examples/session-import-sample.json)

Top-level JSON fields:

- `version`: currently `1`
- `groups`: session groups to create in advance, using `path` to represent nesting
- `passwords`: reusable saved-password entries, referenced within this file by `ref`
- `ssh_keys`: reusable saved-key entries, referenced within this file by `ref`
- `sessions`: session definitions to import

Supported session types:

- `ssh`
- `local_terminal`
- `telnet`
- `serial`
- `rdp`

Supported SSH authentication forms:

- Direct password: `"auth": { "mode": "password", "password": "replace-me" }`
- Saved password: `"auth": { "mode": "password", "password_ref": "prod-root-password" }`
- Saved key: `"auth": { "mode": "key", "key_ref": "ops-ed25519" }`
- SSH Agent: `"auth": { "mode": "agent" }`
- No authentication: `"auth": { "mode": "none" }`

Use either `password` or `password_ref`, but not both. `key` mode must provide `key_ref`. `agent` mode does not import private keys; it uses the current device's available SSH Agent when connecting. A `ref` is only valid inside the current JSON file; after import, NyaTerm generates real local IDs.

:::warning
Passwords and private keys in the JSON file are plaintext. Delete the file after importing, or at least treat it as a sensitive file.
:::

After importing, it is a good idea to verify:

- Host and port
- Username
- Whether proxy / jump host / OTP binding still needs to be added
- Whether saved passwords or keys are already matched correctly

## Host key policy

NyaTerm maintains known-host records and offers three SSH host key policies:

| Policy | Behavior |
|------|------|
| Prompt | Ask whether to trust an unknown host key on first connect (default) |
| Accept | Automatically accept and record new host keys |
| Strict | Reject all unknown host keys |

Known host records are stored in the local redb `known_hosts` document. Legacy `known_hosts` content from the old redb text-document path can be imported through the compatibility reader; NyaTerm does not promise to scan arbitrary filesystem locations for an old file.

If you operate in a stricter environment, verify the host key source before accepting it.

## When to use SSH

The file explorer / SFTP, OTP, jump hosts, proxies, tunnels, and remote host monitoring all require SSH — those capabilities exist only on SSH sessions. For a plain local shell, use [Local Terminal](./session-types#local-terminal).
