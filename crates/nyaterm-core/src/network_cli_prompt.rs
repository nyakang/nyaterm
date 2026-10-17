//! Detection of interactive network-device CLI prompts (H3C, Huawei, Ruijie,
//! Cisco, Aruba, ...).
//!
//! The AI agent wraps commands in shell markers to detect completion. Network
//! devices do not run a shell, so those markers never appear and the command
//! times out (issue #910). For those sessions the agent should send the bare
//! command and treat quiet output as completion instead -- but only when the
//! prompt tells us the remote end is a device CLI rather than a shell.
//!
//! The check is deliberately conservative: it recognises the explicit prompt
//! shapes the vendors use and rejects anything carrying shell fingerprints
//! (`user@host`, a trailing `$`). A false positive degrades to quiet-output
//! completion, which still returns the command's output; a false negative keeps
//! the current shell behaviour.

/// Returns true when the last non-empty line of `output` looks like an
/// interactive network-device CLI prompt.
pub fn looks_like_network_cli_prompt(output: &str) -> bool {
    // Only the tail matters and the session buffer can be megabytes long.
    let tail = tail_chars(output, 4096);
    let Some(line) = tail.lines().rev().find(|line| !line.trim().is_empty()) else {
        return false;
    };
    looks_like_network_cli_prompt_line(line.trim())
}

/// The last `max` bytes of `s`, moved forward to a char boundary.
fn tail_chars(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut start = s.len() - max;
    while start < s.len() && !s.is_char_boundary(start) {
        start += 1;
    }
    &s[start..]
}

fn looks_like_network_cli_prompt_line(line: &str) -> bool {
    // Shell fingerprints: `user@host` prompts and `$`-terminated prompts.
    if line.contains('@') || line.ends_with('$') {
        return false;
    }
    // H3C / Huawei system view: `<HOSTNAME>`.
    if let Some(inner) = line
        .strip_prefix('<')
        .and_then(|rest| rest.strip_suffix('>'))
    {
        return is_hostname_token(inner);
    }
    // H3C / Huawei sub-views: `[HOSTNAME]`, `[HOSTNAME-interface]`.
    if let Some(inner) = line
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
    {
        return is_device_context_token(inner);
    }
    // Cisco / Ruijie / Aruba: `Hostname#`, `Hostname(config)#`, `(Name) #`.
    let body = line.trim_end_matches(['#', '*']);
    if body == line {
        // No `#`/`*` terminator: only the bracket forms above count.
        return false;
    }
    is_device_context_token(body.trim_end())
}

/// Hostnames: letters, digits, `.`, `_`, `-`.
fn is_hostname_token(token: &str) -> bool {
    !token.is_empty()
        && token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// Hostnames plus sub-view suffixes such as `Hostname(config)` or
/// `HOST-1/GigabitEthernet0/0/1`.
fn is_device_context_token(token: &str) -> bool {
    !token.is_empty()
        && token.chars().all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/' | '(' | ')' | ' ')
        })
}

#[cfg(test)]
mod tests {
    use super::looks_like_network_cli_prompt;

    #[test]
    fn recognizes_h3c_and_huawei_prompts() {
        assert!(looks_like_network_cli_prompt("<Switch>"));
        assert!(looks_like_network_cli_prompt("<HUAWEI>"));
        assert!(looks_like_network_cli_prompt("[H3C]"));
        assert!(looks_like_network_cli_prompt("[H3C-GigabitEthernet1/0/1]"));
        assert!(looks_like_network_cli_prompt("display version\n<Switch>"));
    }

    #[test]
    fn recognizes_cisco_ruijie_and_aruba_prompts() {
        assert!(looks_like_network_cli_prompt("Switch#"));
        assert!(looks_like_network_cli_prompt("Switch(config)#"));
        assert!(looks_like_network_cli_prompt("Switch(config-if)#"));
        assert!(looks_like_network_cli_prompt("(Controller) #"));
        assert!(looks_like_network_cli_prompt("(Controller) *"));
    }

    #[test]
    fn rejects_shell_prompts() {
        assert!(!looks_like_network_cli_prompt("user@host:~$"));
        assert!(!looks_like_network_cli_prompt("[root@host ~]#"));
        assert!(!looks_like_network_cli_prompt("root@ubuntu:/var/log#"));
        assert!(!looks_like_network_cli_prompt("PS C:\\Users\\admin>"));
    }

    #[test]
    fn rejects_banners_and_plain_output() {
        assert!(!looks_like_network_cli_prompt(""));
        assert!(!looks_like_network_cli_prompt(
            "Welcome to Ubuntu 24.04 LTS"
        ));
        assert!(!looks_like_network_cli_prompt(
            "Last login: Fri Oct 16 09:00"
        ));
        assert!(!looks_like_network_cli_prompt("total 24"));
    }

    #[test]
    fn uses_the_last_non_empty_line() {
        assert!(looks_like_network_cli_prompt("<Switch>\n\n"));
        assert!(looks_like_network_cli_prompt(
            "Welcome to the device\n<Switch>"
        ));
        // A banner after the prompt means the prompt is not what the session
        // is sitting at any more.
        assert!(!looks_like_network_cli_prompt(
            "<Switch>\nCopyright (c) 2020"
        ));
    }
}
