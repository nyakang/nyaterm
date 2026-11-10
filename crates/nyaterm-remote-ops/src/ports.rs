//! Listening sockets and the processes that own them (`ss -tulpn`, but for every
//! platform and for SSH hosts).
//!
//! One batched script per refresh. The script prints a format marker on its own
//! line, then the tool's raw output, so the parser never has to guess which
//! dialect it was given. Local Windows uses the native socket tables instead of
//! spawning PowerShell.

use crate::host::{Elevation, HostOs, HostTarget};
use std::collections::HashMap;
use std::time::Duration;

const PORTS_TIMEOUT: Duration = Duration::from_secs(15);
pub const PORTS_UNSUPPORTED_MARKER: &str = "NYATERM_PORTS_UNSUPPORTED";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PortProtocol {
    Tcp,
    Udp,
}

impl PortProtocol {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Tcp => "TCP",
            Self::Udp => "UDP",
        }
    }
}

/// How far a socket is reachable, derived from its bind address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PortExposure {
    /// `127.0.0.0/8` or `::1`.
    Loopback,
    /// `0.0.0.0`, `::` or `*`: reachable on every interface.
    AllInterfaces,
    /// A specific interface address.
    Interface,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PortEntry {
    pub protocol: PortProtocol,
    pub address: String,
    pub port: u16,
    pub pid: Option<u32>,
    pub process: Option<String>,
    pub user: Option<String>,
}

impl PortEntry {
    pub fn exposure(&self) -> PortExposure {
        classify_address(&self.address)
    }

    /// Stable identity for selection across refreshes.
    pub fn key(&self) -> String {
        format!(
            "{}|{}|{}|{}",
            self.protocol.label(),
            self.address,
            self.port,
            self.pid.unwrap_or(0)
        )
    }
}

pub fn classify_address(address: &str) -> PortExposure {
    let address = address.split('%').next().unwrap_or(address);
    match address {
        "*" | "0.0.0.0" | "::" | "" => PortExposure::AllInterfaces,
        "::1" | "localhost" => PortExposure::Loopback,
        other if other.starts_with("127.") => PortExposure::Loopback,
        _ => PortExposure::Interface,
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PortsSnapshot {
    pub entries: Vec<PortEntry>,
    /// Some sockets have no visible owner: the tool lacked the privilege. The UI
    /// uses this to offer an elevated refresh instead of showing blanks silently.
    pub owners_hidden: bool,
}

/// Prints `@@FORMAT <name>` and the tool output of the best available backend.
pub const PORTS_SCRIPT: &str = r#"
unsupported() { echo "NYATERM_PORTS_UNSUPPORTED"; exit 42; }
if [ "$(uname -s 2>/dev/null)" = "Darwin" ]; then
  echo "@@FORMAT macos"
  tcp=$(netstat -anv -p tcp 2>/dev/null)
  printf '%s\n' "$tcp"
  netstat -anv -p udp 2>/dev/null
  # macOS 26+ prints `name:pid` itself. Older releases print only the numeric
  # pid, so names and users come from one `ps` (paid only on those hosts).
  case "$tcp" in
    *process:pid*) ;;
    *) echo "@@USERS"; ps -axo pid=,user=,comm= 2>/dev/null ;;
  esac
elif command -v ss >/dev/null 2>&1; then
  echo "@@FORMAT ss"
  ss -tulnpe 2>/dev/null
  echo "@@PASSWD"
  cut -d: -f1,3 /etc/passwd 2>/dev/null
elif command -v sockstat >/dev/null 2>&1; then
  echo "@@FORMAT sockstat"
  sockstat -46l 2>/dev/null
elif command -v netstat >/dev/null 2>&1; then
  echo "@@FORMAT netstat"
  netstat -tulnp 2>/dev/null || netstat -lnp 2>/dev/null || netstat -an 2>/dev/null
else
  unsupported
fi
"#;

pub fn list_ports(
    target: &HostTarget,
    os: HostOs,
    elevation: Elevation,
) -> anyhow::Result<PortsSnapshot> {
    target.ensure_probes_allowed()?;
    #[cfg(windows)]
    if target.is_local() && matches!(os, HostOs::Windows) {
        return native::list_windows();
    }
    if matches!(os, HostOs::Windows) {
        anyhow::bail!("port listing is unsupported on this host");
    }
    let output = target.run_sh(PORTS_SCRIPT, elevation, PORTS_TIMEOUT)?;
    if output.stdout.contains(PORTS_UNSUPPORTED_MARKER) {
        anyhow::bail!("no supported port inspection tool found on this host");
    }
    if output.stdout.trim().is_empty() {
        let detail = output.stderr.trim();
        anyhow::bail!(
            "{}",
            if detail.is_empty() {
                "port listing returned no data"
            } else {
                detail
            }
        );
    }
    Ok(parse_ports_output(&output.stdout))
}

/// What the list cannot afford to collect for every socket: the owner's user
/// name and full command line. Fetched on demand for the selected row only
/// (one `ps` costs ~30 ms on macOS, which would triple the whole refresh).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProcessDetails {
    pub user: Option<String>,
    pub command: Option<String>,
}

pub fn parse_process_details(output: &str) -> ProcessDetails {
    let line = output.lines().map(str::trim).find(|l| !l.is_empty());
    let Some(line) = line else {
        return ProcessDetails::default();
    };
    let (user, command) = match line.split_once(char::is_whitespace) {
        Some((user, command)) => (user, command.trim()),
        None => (line, ""),
    };
    ProcessDetails {
        user: Some(user.to_string()).filter(|u| !u.is_empty()),
        command: Some(command.to_string()).filter(|c| !c.is_empty()),
    }
}

pub fn process_details(
    target: &HostTarget,
    os: HostOs,
    pid: u32,
    elevation: Elevation,
) -> anyhow::Result<ProcessDetails> {
    target.ensure_probes_allowed()?;
    if matches!(os, HostOs::Windows) {
        anyhow::ensure!(target.is_local(), "remote Windows hosts are not supported");
        let found = crate::local_process::list_local_processes()
            .into_iter()
            .find(|process| process.pid == pid)
            .ok_or_else(|| anyhow::anyhow!("process {pid} no longer exists"))?;
        return Ok(ProcessDetails {
            user: Some(found.user).filter(|u| u != "-"),
            command: Some(found.command_line).filter(|c| !c.is_empty()),
        });
    }
    let output = target.run_sh(
        &format!("ps -o user=,args= -p {pid}"),
        elevation,
        PORTS_TIMEOUT,
    )?;
    let output = crate::host::ensure_host_command_success(output, "process details unavailable")?;
    Ok(parse_process_details(&output.stdout))
}

pub fn parse_ports_output(output: &str) -> PortsSnapshot {
    let mut format = "";
    let mut main = Vec::new();
    let mut aux = Vec::new();
    let mut in_aux = false;
    for line in output.lines() {
        if let Some(name) = line.strip_prefix("@@FORMAT ") {
            format = name.trim();
        } else if line.starts_with("@@USERS") || line.starts_with("@@PASSWD") {
            in_aux = true;
        } else if in_aux {
            aux.push(line);
        } else {
            main.push(line);
        }
    }
    // The modern format has a `process:pid` column; older macOS only has a pid.
    let legacy_macos = !main.iter().any(|line| line.contains("process:pid"));
    let mut entries = match format {
        "macos" if legacy_macos => main
            .iter()
            .filter_map(|l| parse_macos_legacy_line(l))
            .collect(),
        "macos" => main.iter().filter_map(|l| parse_macos_line(l)).collect(),
        "ss" => main.iter().filter_map(|l| parse_ss_line(l)).collect(),
        "sockstat" => main.iter().filter_map(|l| parse_sockstat_line(l)).collect(),
        "netstat" => main.iter().filter_map(|l| parse_netstat_line(l)).collect(),
        _ => Vec::new(),
    };
    match format {
        "macos" => {
            // `pid user [command...]`; the command is a full path, possibly with spaces.
            let known: HashMap<u32, (&str, &str)> = aux
                .iter()
                .filter_map(|line| {
                    let line = line.trim_start();
                    let (pid, rest) = line.split_once(char::is_whitespace)?;
                    let rest = rest.trim_start();
                    let (user, command) = rest
                        .split_once(char::is_whitespace)
                        .map_or((rest, ""), |(user, command)| (user, command.trim()));
                    Some((pid.parse().ok()?, (user, command)))
                })
                .collect();
            for entry in &mut entries {
                let Some((user, command)) = entry.pid.and_then(|pid| known.get(&pid)) else {
                    continue;
                };
                entry.user = Some((*user).to_string()).filter(|u| !u.is_empty());
                if entry.process.is_none() {
                    entry.process = command
                        .rsplit('/')
                        .next()
                        .map(str::to_string)
                        .filter(|name| !name.is_empty());
                }
            }
        }
        "ss" => {
            // `ss -e` reports a numeric uid; resolve it through the host's passwd.
            let names: HashMap<&str, &str> = aux
                .iter()
                .filter_map(|line| line.split_once(':').map(|(name, uid)| (uid, name)))
                .collect();
            for entry in &mut entries {
                if let Some(uid) = entry.user.take() {
                    entry.user = Some(names.get(uid.as_str()).copied().unwrap_or(&uid).to_string());
                }
            }
        }
        _ => {}
    }
    entries.sort_by(|a: &PortEntry, b| {
        (a.port, a.protocol, &a.address).cmp(&(b.port, b.protocol, &b.address))
    });
    entries.dedup();
    let owners_hidden = entries.iter().any(|entry| entry.pid.is_none());
    PortsSnapshot {
        entries,
        owners_hidden,
    }
}

/// Split `host:port` where the host may be bracketed IPv6 or contain colons.
fn split_colon_port(value: &str) -> Option<(String, u16)> {
    let (host, port) = value.rsplit_once(':')?;
    let port = port.parse().ok()?;
    let host = host.trim_start_matches('[').trim_end_matches(']');
    Some((host.to_string(), port))
}

/// Split `host.port` (BSD/macOS netstat).
fn split_dot_port(value: &str) -> Option<(String, u16)> {
    let (host, port) = value.rsplit_once('.')?;
    let port = port.parse().ok()?;
    Some((host.to_string(), port))
}

fn parse_ss_line(line: &str) -> Option<PortEntry> {
    let mut parts = line.split_whitespace();
    let netid = parts.next()?;
    let protocol = match netid {
        "tcp" => PortProtocol::Tcp,
        "udp" => PortProtocol::Udp,
        _ => return None,
    };
    let _state = parts.next()?;
    let _recv = parts.next()?;
    let _send = parts.next()?;
    let (address, port) = split_colon_port(parts.next()?)?;
    let _peer = parts.next()?;
    let rest = line.split_once(" users:").map(|(_, tail)| tail);
    let (process, pid) = rest.map(parse_ss_users).unwrap_or((None, None));
    let uid = line
        .split_whitespace()
        .find_map(|token| token.strip_prefix("uid:"))
        .map(str::to_string);
    Some(PortEntry {
        protocol,
        address,
        port,
        pid,
        process,
        user: uid,
    })
}

/// `(("sshd",pid=812,fd=3),("sshd",pid=900,fd=4))` -> first owner.
fn parse_ss_users(tail: &str) -> (Option<String>, Option<u32>) {
    let Some(start) = tail.find('"') else {
        return (None, None);
    };
    let after = &tail[start + 1..];
    let Some(end) = after.find('"') else {
        return (None, None);
    };
    let name = after[..end].to_string();
    let pid = after[end..].split("pid=").nth(1).and_then(|value| {
        value
            .chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>()
            .parse()
            .ok()
    });
    (Some(name), pid)
}

fn parse_sockstat_line(line: &str) -> Option<PortEntry> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 7 || parts[0] == "USER" {
        return None;
    }
    let protocol = if parts[4].starts_with("tcp") {
        PortProtocol::Tcp
    } else if parts[4].starts_with("udp") {
        PortProtocol::Udp
    } else {
        return None;
    };
    let (address, port) = split_colon_port(parts[5])?;
    Some(PortEntry {
        protocol,
        address,
        port,
        pid: parts[2].parse().ok(),
        process: Some(parts[1].to_string()).filter(|name| name != "?"),
        user: Some(parts[0].to_string()).filter(|user| user != "?"),
    })
}

fn parse_netstat_line(line: &str) -> Option<PortEntry> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    let proto = parts.first()?;
    let protocol = if proto.starts_with("tcp") {
        PortProtocol::Tcp
    } else if proto.starts_with("udp") {
        PortProtocol::Udp
    } else {
        return None;
    };
    if protocol == PortProtocol::Tcp && !parts.contains(&"LISTEN") {
        return None;
    }
    let (address, port) = split_colon_port(parts.get(3)?)?;
    let owner = parts.last().filter(|token| token.contains('/'));
    let (pid, process) = owner
        .and_then(|token| token.split_once('/'))
        .map(|(pid, name)| (pid.parse().ok(), Some(name.to_string())))
        .unwrap_or((None, None));
    Some(PortEntry {
        protocol,
        address,
        port,
        pid,
        process,
        user: None,
    })
}

/// macOS `netstat -anv`. The owner column is `name:pid` and the name may contain
/// spaces, so it is located by counting from both ends of the row: everything
/// after it is eight fixed hex/counter columns.
fn parse_macos_line(line: &str) -> Option<PortEntry> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    let proto = parts.first()?;
    let protocol = if proto.starts_with("tcp") {
        PortProtocol::Tcp
    } else if proto.starts_with("udp") {
        PortProtocol::Udp
    } else {
        return None;
    };
    let (prefix, require_listen) = match protocol {
        // proto recvq sendq local foreign state rxbytes txbytes rhiwat shiwat
        PortProtocol::Tcp => (10, true),
        // proto recvq sendq local foreign rxbytes txbytes rhiwat shiwat
        PortProtocol::Udp => (9, false),
    };
    if parts.len() < prefix + 1 + 8 {
        return None;
    }
    if require_listen && parts[5] != "LISTEN" {
        return None;
    }
    let (address, port) = split_dot_port(parts[3])?;
    let owner = parts[prefix..parts.len() - 8].join(" ");
    let (name, pid) = owner.rsplit_once(':')?;
    Some(PortEntry {
        protocol,
        address: address.to_string(),
        port,
        pid: pid.parse().ok(),
        process: Some(name.to_string()).filter(|name| !name.is_empty()),
        user: None,
    })
}

/// macOS before the `process:pid` column: after the addresses come
/// `rhiwat shiwat pid epid state options` (TCP has a `(state)` column first).
fn parse_macos_legacy_line(line: &str) -> Option<PortEntry> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    let proto = parts.first()?;
    let (protocol, state_columns) = if proto.starts_with("tcp") {
        (PortProtocol::Tcp, 1)
    } else if proto.starts_with("udp") {
        (PortProtocol::Udp, 0)
    } else {
        return None;
    };
    // proto recvq sendq local foreign [state] rhiwat shiwat pid epid state options
    let pid_at = 5 + state_columns + 2;
    if parts.len() <= pid_at + 1 {
        return None;
    }
    if protocol == PortProtocol::Tcp && parts[5] != "LISTEN" {
        return None;
    }
    let (address, port) = split_dot_port(parts[3])?;
    Some(PortEntry {
        protocol,
        address: address.to_string(),
        port,
        pid: parts[pid_at].parse().ok().filter(|pid| *pid > 0),
        process: None,
        user: None,
    })
}

#[cfg(windows)]
mod native {
    use super::{PortEntry, PortProtocol, PortsSnapshot};

    pub fn list_windows() -> anyhow::Result<PortsSnapshot> {
        let listeners = listeners::get_all()
            .map_err(|error| anyhow::anyhow!("port listing failed: {error}"))?;
        let mut entries: Vec<PortEntry> = listeners
            .into_iter()
            .map(|listener| PortEntry {
                protocol: match listener.protocol {
                    listeners::Protocol::TCP => PortProtocol::Tcp,
                    listeners::Protocol::UDP => PortProtocol::Udp,
                },
                address: listener.socket.ip().to_string(),
                port: listener.socket.port(),
                pid: Some(listener.process.pid),
                process: Some(listener.process.name),
                user: None,
            })
            .collect();
        entries.sort_by(|a, b| {
            (a.port, a.protocol, &a.address).cmp(&(b.port, b.protocol, &b.address))
        });
        Ok(PortsSnapshot {
            entries,
            owners_hidden: false,
        })
    }
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "macos")]
    use super::list_ports;
    use super::{
        PORTS_SCRIPT, PortExposure, PortProtocol, ProcessDetails, parse_ports_output,
        parse_process_details,
    };
    #[cfg(target_os = "macos")]
    use crate::host::{Elevation, HostOs, HostTarget};

    const MACOS: &str = "@@FORMAT macos
Active Internet connections (including servers)
Proto Recv-Q Send-Q  Local Address          Foreign Address        (state)          rxbytes      txbytes  rhiwat  shiwat          process:pid    state  options           gencnt    flags   flags1 usecnt rtncnt fltrs
tcp4       0      0  127.0.0.1.49156        *.*                    LISTEN                 0            0  131072  131072 devserver:3530   00100 00000006 0000000000004580 00000001 00002800      1      0 000000
tcp46      0      0  *.5000                 *.*                    LISTEN                 0            0  131072  131072 Code Helper (Plu:951    00100 00000106 0000000000001a66 00000001 00000800      1      0 000000
tcp4       0      0  127.0.0.1.55000        10.0.0.2.443           ESTABLISHED         9000         1000  131072  131072             curl:77     00100 00000106 0000000000001a66 00000001 00000800      1      0 000000
Active Internet connections (including servers)
Proto Recv-Q Send-Q  Local Address          Foreign Address                                    rxbytes      txbytes  rhiwat  shiwat          process:pid    state  options           gencnt    flags   flags1 usecnt rtncnt fltrs
udp4       0      0  *.63035                *.*                                           0            0  786896    9216            homed:644    00000 00000224 0000000000001579 00000001 00000800      1      0 000000
@@USERS
  644 root
 3530 jakub
  951 jakub
";

    #[test]
    fn macos_netstat_keeps_listeners_names_with_spaces_and_resolves_users() {
        let snapshot = parse_ports_output(MACOS);
        assert_eq!(
            snapshot.entries.len(),
            3,
            "ESTABLISHED rows are not listeners"
        );
        let code = snapshot.entries.iter().find(|e| e.port == 5000).unwrap();
        assert_eq!(code.process.as_deref(), Some("Code Helper (Plu"));
        assert_eq!(code.pid, Some(951));
        assert_eq!(code.user.as_deref(), Some("jakub"));
        assert_eq!(code.exposure(), PortExposure::AllInterfaces);
        let udp = snapshot.entries.iter().find(|e| e.port == 63035).unwrap();
        assert_eq!(udp.protocol, PortProtocol::Udp);
        assert_eq!(udp.user.as_deref(), Some("root"));
        let loopback = snapshot.entries.iter().find(|e| e.port == 49156).unwrap();
        assert_eq!(loopback.exposure(), PortExposure::Loopback);
    }

    #[test]
    fn ss_output_resolves_process_and_uid_to_names() {
        let output = "@@FORMAT ss
Netid State  Recv-Q Send-Q Local Address:Port Peer Address:Port Process
tcp   LISTEN 0      128          0.0.0.0:22        0.0.0.0:*     users:((\"sshd\",pid=812,fd=3)) uid:0 ino:123 sk:1
tcp   LISTEN 0      4096           [::1]:5432           [::]:*     users:((\"postgres\",pid=900,fd=5),(\"x\",pid=901,fd=5)) uid:110 ino:5 sk:2
udp   UNCONN 0      0      127.0.0.53%lo:53         0.0.0.0:*     users:((\"systemd-resolve\",pid=600,fd=13)) uid:101 ino:9 sk:3
tcp   LISTEN 0      128             [::]:22           [::]:*
@@PASSWD
root:0
postgres:110
systemd-resolve:101
";
        let snapshot = parse_ports_output(output);
        assert_eq!(snapshot.entries.len(), 4);
        let ssh = &snapshot.entries[0];
        assert_eq!((ssh.port, ssh.pid), (22, Some(812)));
        assert_eq!(ssh.user.as_deref(), Some("root"));
        let pg = snapshot.entries.iter().find(|e| e.port == 5432).unwrap();
        assert_eq!(pg.process.as_deref(), Some("postgres"));
        assert_eq!(pg.address, "::1");
        assert_eq!(pg.exposure(), PortExposure::Loopback);
        let dns = snapshot.entries.iter().find(|e| e.port == 53).unwrap();
        assert_eq!(dns.address, "127.0.0.53%lo");
        assert_eq!(dns.exposure(), PortExposure::Loopback);
        assert!(snapshot.owners_hidden, "the last row has no visible owner");
    }

    #[test]
    fn sockstat_output_is_parsed_for_freebsd() {
        let output = "@@FORMAT sockstat
USER     COMMAND    PID   FD PROTO  LOCAL ADDRESS         FOREIGN ADDRESS
root     sshd       812   4  tcp4   *:22                  *:*
root     sshd       812   5  tcp6   *:22                  *:*
?        ?          ?     ?  udp4   *:514                 *:*
";
        let snapshot = parse_ports_output(output);
        // The v4 and v6 wildcard rows of one daemon collapse into one entry.
        assert_eq!(snapshot.entries.len(), 2);
        assert_eq!(snapshot.entries[0].pid, Some(812));
        assert!(
            snapshot
                .entries
                .iter()
                .any(|e| e.port == 22 && e.pid == Some(812))
        );
    }

    #[test]
    fn netstat_fallback_only_keeps_tcp_listeners() {
        let output = "@@FORMAT netstat
tcp        0      0 0.0.0.0:22              0.0.0.0:*               LISTEN      812/sshd
tcp        0      0 10.0.0.2:22             10.0.0.9:5555           ESTABLISHED 900/sshd
tcp6       0      0 :::80                   :::*                    LISTEN      -
udp        0      0 0.0.0.0:68              0.0.0.0:*                           456/dhclient
";
        let snapshot = parse_ports_output(output);
        assert_eq!(snapshot.entries.len(), 3);
        let web = snapshot.entries.iter().find(|e| e.port == 80).unwrap();
        assert_eq!(web.address, "::");
        assert_eq!(web.pid, None);
    }

    const MACOS_LEGACY: &str = "@@FORMAT macos
Active Internet connections (including servers)
Proto Recv-Q Send-Q  Local Address          Foreign Address        (state)      rhiwat  shiwat    pid   epid state    options
tcp4       0      0  127.0.0.1.49156        *.*                    LISTEN       131072  131072   1234      0 0x0100 0x00000006
tcp46      0      0  *.5000                 *.*                    LISTEN       131072  131072    605      0 0x0100 0x00000006
tcp4       0      0  127.0.0.1.55000        10.0.0.2.443           ESTABLISHED  131072  131072     77      0 0x0102 0x00000006
Active Internet connections (including servers)
Proto Recv-Q Send-Q  Local Address          Foreign Address                    rhiwat  shiwat    pid   epid state    options
udp4       0      0  *.5353                 *.*                                786896    9216    482      0 0x0000 0x00000004
@@USERS
 1234 me /Applications/Some App.app/Contents/MacOS/Some App
  605 jakub /System/Library/CoreServices/ControlCenter.app/Contents/MacOS/ControlCenter
  482 _mdnsresponder /usr/sbin/mDNSResponder
";

    #[test]
    fn macos_before_the_process_pid_column_still_lists_ports_with_names_from_ps() {
        let snapshot = parse_ports_output(MACOS_LEGACY);
        assert_eq!(
            snapshot.entries.len(),
            3,
            "ESTABLISHED rows are not listeners"
        );
        let app = snapshot.entries.iter().find(|e| e.port == 49156).unwrap();
        assert_eq!(app.pid, Some(1234));
        assert_eq!(app.process.as_deref(), Some("Some App"));
        assert_eq!(app.user.as_deref(), Some("me"));
        let udp = snapshot.entries.iter().find(|e| e.port == 5353).unwrap();
        assert_eq!(udp.protocol, PortProtocol::Udp);
        assert_eq!(udp.pid, Some(482));
        assert_eq!(udp.process.as_deref(), Some("mDNSResponder"));
        assert!(!snapshot.owners_hidden);
    }

    #[test]
    fn process_details_split_the_user_from_a_command_with_spaces() {
        let details = parse_process_details("  root   /usr/sbin/sshd -D -o \"A B\"\n");
        assert_eq!(details.user.as_deref(), Some("root"));
        assert_eq!(
            details.command.as_deref(),
            Some("/usr/sbin/sshd -D -o \"A B\"")
        );
        assert_eq!(parse_process_details("\n"), ProcessDetails::default());
        assert_eq!(parse_process_details("daemon").command, None);
    }

    #[test]
    fn ps_is_only_spawned_on_macos_releases_without_the_process_pid_column() {
        // The fast path must not pay for `ps`; it is reachable only through the
        // legacy branch that checks for the modern column first.
        let ps_at = PORTS_SCRIPT.find("ps -axo").expect("legacy branch");
        let guard_at = PORTS_SCRIPT.find("*process:pid*)").expect("column check");
        assert!(guard_at < ps_at);
        assert_eq!(PORTS_SCRIPT.matches("ps -axo").count(), 1);
    }

    #[test]
    fn unknown_format_yields_no_entries_instead_of_garbage() {
        assert!(parse_ports_output("hello\nworld").entries.is_empty());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn local_macos_listing_runs_and_finds_listeners() {
        let snapshot =
            list_ports(&HostTarget::Local, HostOs::local(), Elevation::None).expect("netstat");
        // Show the raw tool output when parsing finds nothing: the format differs
        // between macOS releases, and a bare assertion would hide that.
        let raw = || {
            HostTarget::Local
                .run_sh(
                    PORTS_SCRIPT,
                    Elevation::None,
                    std::time::Duration::from_secs(10),
                )
                .map(|out| out.stdout.lines().take(12).collect::<Vec<_>>().join("\n"))
                .unwrap_or_default()
        };
        assert!(
            snapshot.entries.iter().any(|e| e.pid.is_some()),
            "no listener with a pid parsed; raw output:\n{}",
            raw()
        );
    }
}
