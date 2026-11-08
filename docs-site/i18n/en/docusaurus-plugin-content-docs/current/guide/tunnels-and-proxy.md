# Tunnels, Proxy, and Jump Hosts

NyaTerm separates network-related features into three layers:

1. **Proxy** — how the app reaches the network
2. **Jump host** — which SSH host acts as the intermediate hop
3. **Tunnel** — where port traffic is mapped

These features often appear together in real environments, but they solve different problems.

## Proxy

A proxy helps NyaTerm establish outbound connectivity to the remote side.

Currently supported:

- **SOCKS5**
- **HTTP**
- **ProxyCommand**

Each proxy configuration can store:

- Name
- Protocol
- Host
- Port
- Username / password
- ProxyCommand command template

### ProxyCommand

ProxyCommand reuses an existing command-line proxy, gateway, or corporate network helper. NyaTerm replaces OpenSSH-style placeholders in the command template:

| Placeholder | Meaning |
|-------------|---------|
| `%h` | Target host |
| `%p` | Target port |
| `%r` | Login username |
| `%%` | Literal `%` |

Example:

```bash
nc -X connect -x jump.example.com:1080 %h %p
```

If your environment depends on a custom gateway, bastion command, or local proxy program, save it as a ProxyCommand profile and select it from the connection's network configuration page.

Proxies are managed centrally in the Network panel. SSH, Telnet / Raw TCP, RDP, and VNC connections can select them from their network configuration page.

Typical use cases:

- Corporate networks that require outbound proxy access
- Regions where direct access is restricted
- Teams that want a reusable set of outbound network profiles

## Jump hosts

A jump host uses another saved SSH connection as the intermediate entry point and can forward SSH, Telnet / Raw TCP, RDP, and VNC connections.

Typical use cases:

- Bastion hosts
- Internal hosts that are not directly reachable
- Multi-layer SSH network isolation

In the SSH connection advanced section, you can pick an existing saved SSH connection as the jump host. NyaTerm validates the jump-host chain:

- Missing referenced jump hosts are blocked or surfaced for correction
- Recursive relationships are rejected so connections cannot loop forever
- Multi-hop designs should stay as clear one-way chains, for example `local -> bastion -> internal host`

### Telnet / Raw TCP

Select a proxy or SSH jump host on the connection's network configuration page. Keep the target host and port set to the final device's address, for example `192.168.1.100:23`. Raw TCP mode uses the same network configuration.

When both a proxy and a jump host are selected, the jump host takes priority and this connection's proxy is not used. If the jump host also needs a proxy, configure it on that jump host's SSH connection. Multi-hop chains use each jump host connection's settings.

SSH encrypts the connection from the client to the jump host; the connection from the jump host to the Telnet / Raw TCP device remains unencrypted. ProxyCommand has no standard protocol for confirming the remote connection. Once the command and its pipes are established, startup is considered successful; a command exit or broken pipe ends the session.

## Tunnels

NyaTerm provides a dedicated tunnel management area in the Network panel, so port mappings can be saved and reused instead of retyped as one-off commands.

### Tunnel types

- **Local tunnel**
- **Remote tunnel**
- **Dynamic tunnel (SOCKS5)**

### Local tunnel

A local tunnel binds a local listening port and forwards traffic to a remote target. Use it to:

- Accessing internal databases
- Opening web consoles that are only reachable from the remote host
- Securely forwarding service ports through SSH

### Remote tunnel

A remote tunnel binds a port on the remote side and forwards it back to a local service. Use it to:

- Temporarily exposing a local service to the remote environment
- Reverse debugging or temporary integration work

### Dynamic tunnel

A dynamic tunnel creates a local SOCKS5 proxy port. Use it to:

- Pointing a browser or tool at an SSH-backed proxy temporarily
- Quickly building an outbound path through SSH

## Tunnel configuration

When creating a tunnel, you typically configure:

- Tunnel name
- Tunnel type
- Associated SSH connection
- Listen port
- Target host / target port for local and remote tunnels
- Whether to bind only to `127.0.0.1`
- Whether to auto-open the tunnel

If the port only needs to be used locally, keeping it bound to `127.0.0.1` is usually safer than listening on `0.0.0.0`.

## Daily operations in the Network panel

In the Network panel, you can:

- Create / edit / delete proxies
- Create / edit / delete tunnels
- Open or close a tunnel directly
- See whether a tunnel is currently active

This turns network setup into something reusable and visible instead of a collection of one-off shell commands.

## Common combinations

### Example 1: Proxy + SSH

- Save a SOCKS5 proxy in the Network panel
- Select that proxy in an SSH connection's advanced section
- Useful in corporate or cross-region network environments

### Example 2: Jump host + target host

- Save the bastion connection first
- Set that bastion as the jump host on the target host connection
- Useful for layered internal network access

### Example 3: SSH + local tunnel

- Create the SSH connection
- Save a local tunnel such as `localhost:15432 -> db.internal:5432`
- Then access the database locally via `127.0.0.1:15432`
