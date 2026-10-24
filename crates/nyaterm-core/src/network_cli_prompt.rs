//! Lightweight CLI prompt hints. Explicit session configuration takes precedence.

use crate::AiExecutionProfile;

/// Only Auto may be overridden by SSH configuration or a passive prompt hint.
pub fn resolve_ssh_execution_profile(
    configured: AiExecutionProfile,
    network_device: bool,
    output: &str,
) -> AiExecutionProfile {
    if configured != AiExecutionProfile::Auto {
        configured
    } else if network_device
        || looks_like_network_cli_prompt(output)
        || looks_like_sql_prompt(output)
    {
        AiExecutionProfile::SendOnly
    } else {
        AiExecutionProfile::Posix
    }
}

/// A forced shell profile does not make extra channels safe for a known device.
pub fn ssh_allows_background_execution(
    configured: AiExecutionProfile,
    network_device: bool,
    output: &str,
) -> bool {
    !network_device
        && !looks_like_network_cli_prompt(output)
        && !looks_like_sql_prompt(output)
        && !matches!(
            configured,
            AiExecutionProfile::SendOnly | AiExecutionProfile::Disabled
        )
}

/// Returns true when the last non-empty line of `output` looks like an
/// interactive network-device CLI prompt.
pub fn looks_like_network_cli_prompt(output: &str) -> bool {
    // Only the tail matters and the session buffer can be megabytes long.
    let tail = tail_chars(output, 4096);
    let mut line = PromptLine::default();
    line.feed(tail);
    looks_like_network_cli_prompt_line(line.text().trim())
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

pub(super) fn looks_like_network_cli_prompt_line(line: &str) -> bool {
    // Shell fingerprints: `user@host` prompts and `$`-terminated prompts.
    if line.contains('@') || line.ends_with('$') || looks_like_cli_input_wait(line) {
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
    let body = line.trim_end_matches(['#', '*', '>']);
    if body == line {
        // No CLI terminator: only the bracket forms above count.
        return false;
    }
    let body = body.trim_end();
    let shell = body.to_ascii_lowercase();
    if ["root", "sh", "bash", "zsh", "fish", "dash", "ksh", "ash"]
        .iter()
        .any(|name| shell == *name || shell.starts_with(&format!("{name}-")))
    {
        return false;
    }
    // Whitespace is only part of the controller form `(Name) #`.
    is_hostname_token(body)
        || (body.starts_with('(')
            && body.ends_with(')')
            && is_hostname_token(&body[1..body.len() - 1]))
        || body.split_once('(').is_some_and(|(host, context)| {
            is_hostname_token(host)
                && context.ends_with(')')
                && is_hostname_token(&context[..context.len() - 1])
        })
}

/// Returns true when the last non-empty line of `output` looks like an
/// interactive SQL client prompt. These clients never run the POSIX marker
/// wrapper, so the agent must send the bare command instead of waiting for
/// markers that never appear (issue #866).
pub fn looks_like_sql_prompt(output: &str) -> bool {
    let tail = tail_chars(output, 4096);
    let mut line = PromptLine::default();
    line.feed(tail);
    looks_like_sql_prompt_line(line.text().trim())
}

/// Oracle SQL*Plus / RMAN, MySQL / MariaDB, PostgreSQL, SQLite.
pub(super) fn looks_like_sql_prompt_line(line: &str) -> bool {
    if line.is_empty() {
        return false;
    }
    let lower = line.to_ascii_lowercase();
    const PROMPTS: &[&str] = &[
        "sqlplus>",
        "sql>",
        "rman>",
        "mysql>",
        "mariadb>",
        "postgres=#",
        "sqlite>",
    ];
    PROMPTS.iter().any(|prompt| lower.ends_with(prompt))
}

pub(super) fn looks_like_cli_input_wait(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    [
        "--more--",
        "-- more --",
        "---- more ----",
        "[y/n]",
        "[yes/no]",
        "(y/n)",
        "[confirm]",
        "password:",
        "press any key",
        "continue?",
        "are you sure",
    ]
    .iter()
    .any(|hint| lower.contains(hint))
}

/// Bounded, incremental last-line observer, including split ANSI sequences and
/// CR overwrites. This is a prompt hint, not a second terminal emulator.
#[derive(Debug, Default)]
pub(super) struct PromptLine {
    line: Vec<char>,
    previous: String,
    cursor: usize,
    escape: u8,
    csi: String,
    overflow: bool,
}

impl PromptLine {
    pub(super) fn feed(&mut self, text: &str) {
        for ch in text.chars() {
            match self.escape {
                1 => {
                    self.escape = match ch {
                        '[' => 2,
                        ']' => 3,
                        _ => 0,
                    };
                    self.csi.clear();
                }
                2 => {
                    if ('\u{40}'..='\u{7e}').contains(&ch) {
                        if ch == 'K' {
                            if self.csi == "2" {
                                self.line.clear();
                                self.cursor = 0;
                            } else {
                                self.line.truncate(self.cursor);
                            }
                        } else if ch == 'G' && (self.csi.is_empty() || self.csi == "1") {
                            self.cursor = 0;
                        }
                        self.escape = 0;
                    } else if self.csi.len() < 32 {
                        self.csi.push(ch);
                    }
                }
                3 => match ch {
                    '\u{7}' => self.escape = 0,
                    '\u{1b}' => self.escape = 4,
                    _ => {}
                },
                4 => self.escape = if ch == '\\' { 0 } else { 3 },
                _ => match ch {
                    '\u{1b}' => self.escape = 1,
                    '\r' => self.cursor = 0,
                    '\n' => {
                        if self.overflow {
                            self.previous.clear();
                        } else if !self.line.is_empty() {
                            self.previous = self.line.iter().collect();
                        }
                        self.line.clear();
                        self.cursor = 0;
                        self.overflow = false;
                    }
                    '\u{8}' => self.cursor = self.cursor.saturating_sub(1),
                    c if !c.is_control() => {
                        if self.cursor < 512 {
                            if self.cursor < self.line.len() {
                                self.line[self.cursor] = c;
                            } else {
                                self.line.push(c);
                            }
                            self.cursor += 1;
                        } else {
                            self.overflow = true;
                        }
                    }
                    _ => {}
                },
            }
        }
    }

    pub(super) fn text(&self) -> String {
        if self.overflow || self.escape != 0 || self.cursor < self.line.len() {
            return String::new();
        }
        if self.line.is_empty() {
            self.previous.clone()
        } else {
            self.line.iter().collect()
        }
    }
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
    use super::{
        looks_like_network_cli_prompt, looks_like_sql_prompt, resolve_ssh_execution_profile,
        ssh_allows_background_execution,
    };
    use crate::{AiExecutionProfile, build_agent_capture_command};

    #[test]
    fn ssh_profile_priority_preserves_explicit_choices_and_shell_markers() {
        let auto = AiExecutionProfile::Auto;
        let network = resolve_ssh_execution_profile(auto, true, "unrecognised prompt");
        assert_eq!(network, AiExecutionProfile::SendOnly);
        assert!(build_agent_capture_command(network, "id", "display version").is_none());
        let shell = resolve_ssh_execution_profile(auto, false, "root#");
        assert_eq!(shell, AiExecutionProfile::Posix);
        assert!(build_agent_capture_command(shell, "id", "pwd").is_some());
        for explicit in [
            AiExecutionProfile::Posix,
            AiExecutionProfile::Powershell,
            AiExecutionProfile::Cmd,
            AiExecutionProfile::SendOnly,
            AiExecutionProfile::Disabled,
        ] {
            assert_eq!(
                resolve_ssh_execution_profile(explicit, true, "<Switch>"),
                explicit
            );
        }
    }

    #[test]
    fn background_never_opens_exec_for_known_devices_raw_or_disabled_profiles() {
        assert!(ssh_allows_background_execution(
            AiExecutionProfile::Auto,
            false,
            "root#"
        ));
        assert!(!ssh_allows_background_execution(
            AiExecutionProfile::Auto,
            true,
            "unknown"
        ));
        assert!(!ssh_allows_background_execution(
            AiExecutionProfile::Posix,
            true,
            "unknown"
        ));
        assert!(!ssh_allows_background_execution(
            AiExecutionProfile::Posix,
            false,
            "Router#"
        ));
        for profile in [AiExecutionProfile::Disabled, AiExecutionProfile::SendOnly] {
            assert!(!ssh_allows_background_execution(profile, false, "root#"));
        }
    }

    #[test]
    fn handles_ansi_and_cr_redraws_without_matching_stale_prompts() {
        assert!(looks_like_network_cli_prompt("\u{1b}[32mRouter>\u{1b}[0m"));
        assert!(looks_like_network_cli_prompt(
            "progress 100%\r\u{1b}[2K<Switch>"
        ));
        assert!(!looks_like_network_cli_prompt("<Switch>\rworking"));
    }

    #[test]
    fn recognizes_interactive_sql_prompts() {
        for prompt in [
            "sqlplus>",
            "SQL>",
            "RMAN>",
            "mysql>",
            "MariaDB>",
            "postgres=#",
            "sqlite>",
        ] {
            assert!(looks_like_sql_prompt(prompt), "{prompt}");
            assert_eq!(
                resolve_ssh_execution_profile(AiExecutionProfile::Auto, false, prompt),
                AiExecutionProfile::SendOnly,
                "{prompt}"
            );
        }
        assert!(looks_like_sql_prompt("select 1 from dual;\nSQL>"));
        assert!(!ssh_allows_background_execution(
            AiExecutionProfile::Auto,
            false,
            "SQL>"
        ));
    }

    #[test]
    fn sql_prompt_detection_rejects_shell_and_network_prompts() {
        for line in ["user@host:~$", "bash-5.2$", "<Switch>", "[H3C]", "Router#"] {
            assert!(!looks_like_sql_prompt(line), "{line}");
        }
    }

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
        for prompt in ["root#", "bash-5.2#", "sh-5.2#", "zsh#", "root>"] {
            assert!(!looks_like_network_cli_prompt(prompt), "{prompt}");
        }
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
