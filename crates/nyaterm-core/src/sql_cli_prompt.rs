//! Passive SQL client hints for commands sent to the existing terminal.

use crate::models::connection::AiExecutionProfile;
use crate::network_cli_prompt::PromptLine;
use crate::terminal::input_tracker::known_terminal_prompt_prefix;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SqlClient {
    SqlPlus,
    Rman,
    MySql,
    MariaDb,
    Postgres,
    Sqlite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SqlPrompt {
    pub client: SqlClient,
    pub continuation: bool,
}

/// The configured dialect describes the outer shell, not a nested SQL client.
/// Never turn execution back on when the user has explicitly disabled it.
pub fn resolve_sql_execution_profile(
    configured: AiExecutionProfile,
    output: &str,
) -> AiExecutionProfile {
    if configured != AiExecutionProfile::Disabled && sql_prompt(output).is_some() {
        AiExecutionProfile::SendOnly
    } else {
        configured
    }
}

pub fn looks_like_sql_prompt(output: &str) -> bool {
    sql_prompt(output).is_some()
}

pub(crate) fn sql_prompt(output: &str) -> Option<SqlPrompt> {
    let mut start = output.len().saturating_sub(4096);
    while !output.is_char_boundary(start) {
        start += 1;
    }
    let mut observer = PromptLine::default();
    let mut context = None;
    let mut result = None;
    for part in output[start..].split_inclusive('\n') {
        observer.feed(part);
        let observed = observer.text();
        let line = observed.trim();
        result = classify_sql_prompt_line(line).or_else(|| {
            context
                .filter(|client: &SqlClient| {
                    !part.ends_with('\n') && client.is_continuation(&observed)
                })
                .map(|client| SqlPrompt {
                    client,
                    continuation: true,
                })
        });
        if let Some(prompt) = result {
            context = Some(prompt.client);
        } else if known_terminal_prompt_prefix(line).is_some() {
            // A shell prompt (including command echo) invalidates old SQL context.
            context = None;
        } else {
            // A command echo can establish context for otherwise ambiguous `->`
            // or SQL*Plus line numbers, but is never itself a returned prompt.
            for (end, ch) in line.char_indices() {
                if ch.is_whitespace()
                    && let Some(prompt) = classify_sql_prompt_line(&line[..end])
                {
                    context = Some(prompt.client);
                    break;
                }
            }
        }
    }
    result
}

pub(crate) fn classify_sql_prompt_line(line: &str) -> Option<SqlPrompt> {
    let lower = line.to_ascii_lowercase();
    let client = match lower.as_str() {
        "sqlplus>" | "sql>" => SqlClient::SqlPlus,
        "rman>" => SqlClient::Rman,
        "mysql>" => SqlClient::MySql,
        "mariadb>" => SqlClient::MariaDb,
        "sqlite>" => SqlClient::Sqlite,
        _ => {
            for (prefix, client) in [
                ("mariadb [", SqlClient::MariaDb),
                ("mysql [", SqlClient::MySql),
            ] {
                if lower
                    .strip_prefix(prefix)
                    .and_then(|s| s.strip_suffix("]>"))
                    .is_some_and(|db| !db.is_empty() && !db.contains(['[', ']']))
                {
                    return Some(SqlPrompt {
                        client,
                        continuation: false,
                    });
                }
            }
            // Default psql prompts: database + prompt status + transaction status
            // + privilege indicator. Database changes and failed transactions
            // must not prevent recognizing the next ready prompt.
            let body = line.strip_suffix(['#', '>'])?;
            let body = body
                .strip_suffix(['*', '!'])
                .filter(|body| body.ends_with(['=', '-', '\'', '"', '$', '(', '*']))
                .unwrap_or(body);
            let state = body.chars().next_back()?;
            if !matches!(state, '=' | '-' | '\'' | '"' | '$' | '(' | '*') {
                return None;
            }
            let database = &body[..body.len() - state.len_utf8()];
            if database.is_empty()
                || !database
                    .chars()
                    .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.'))
            {
                return None;
            }
            return Some(SqlPrompt {
                client: SqlClient::Postgres,
                continuation: state != '=',
            });
        }
    };
    Some(SqlPrompt {
        client,
        continuation: false,
    })
}

impl SqlClient {
    pub(crate) fn is_continuation(self, line: &str) -> bool {
        let trimmed = line.trim();
        match self {
            Self::SqlPlus | Self::Rman => {
                line.starts_with("  ")
                    && line.ends_with(' ')
                    && !trimmed.is_empty()
                    && trimmed.bytes().all(|c| c.is_ascii_digit())
            }
            Self::MySql | Self::MariaDb => matches!(trimmed, "->" | "'>" | "\">" | "`>" | "/*>"),
            Self::Sqlite => trimmed == "...>",
            Self::Postgres => classify_sql_prompt_line(trimmed)
                .is_some_and(|prompt| prompt.client == self && prompt.continuation),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::models::connection::AiExecutionProfile;
    use crate::network_cli_prompt::{
        resolve_ssh_execution_profile, ssh_allows_background_execution,
    };
    use crate::sql_cli_prompt::{looks_like_sql_prompt, resolve_sql_execution_profile};

    #[test]
    fn sql_clients_suspend_shell_wrappers_and_background_execution() {
        for prompt in [
            "sqlplus>",
            "SQL>",
            "RMAN>",
            "mysql>",
            "MariaDB>",
            "sqlite>",
            "postgres=#",
            "app_db=>",
            "app_db=*#",
            "app_db=!>",
            "应用库=>",
            "MariaDB [(none)]>",
            "MariaDB [app_db]>",
            "mysql [app_db]>",
            "app_db->",
            "app_db'!>",
            "app_db*>",
            "SQL> select\r\n  2  ",
            "mysql> select\r\n    -> ",
            "sqlite> select\r\n   ...> ",
        ] {
            for profile in [
                AiExecutionProfile::Auto,
                AiExecutionProfile::Posix,
                AiExecutionProfile::Powershell,
                AiExecutionProfile::Cmd,
                AiExecutionProfile::SendOnly,
            ] {
                assert_eq!(
                    resolve_sql_execution_profile(profile, prompt),
                    AiExecutionProfile::SendOnly,
                    "{prompt}"
                );
                assert_eq!(
                    resolve_ssh_execution_profile(profile, false, prompt),
                    AiExecutionProfile::SendOnly,
                    "{prompt}"
                );
                assert!(
                    !ssh_allows_background_execution(profile, false, prompt),
                    "{prompt}"
                );
            }
            assert_eq!(
                resolve_sql_execution_profile(AiExecutionProfile::Disabled, prompt),
                AiExecutionProfile::Disabled
            );
        }
    }

    #[test]
    fn shell_return_and_output_do_not_reuse_stale_sql_prompts() {
        for output in [
            "user@host:~$",
            "PS C:\\Users>",
            "C:\\Users>",
            "Router#",
            "[H3C]",
            "echo SQL>",
            "SQL> select 1;",
            "mysql> select",
            "postgres=# select",
            "prefix postgres=#",
            "SQL>\r\nquery is running",
            "mysql>\r\nuser@host:~$",
            "mysql>\r\nuser@host:~$ cat\r\n->",
            "->",
            "  2  ",
            "...>",
            "SQL> select 1 from dual;\r\n  1  \r\n",
            "SQL>\rworking",
            "\x1b]0;SQL>\x07user@host:~$",
        ] {
            assert!(!looks_like_sql_prompt(output), "{output:?}");
            for profile in [
                AiExecutionProfile::Auto,
                AiExecutionProfile::Posix,
                AiExecutionProfile::Powershell,
            ] {
                assert_eq!(resolve_sql_execution_profile(profile, output), profile);
            }
        }
        assert!(looks_like_sql_prompt("banner\r\n\x1b[32mapp_db=!>\x1b[0m "));
        assert!(looks_like_sql_prompt("progress\r\x1b[2KSQL>"));
        assert!(looks_like_sql_prompt("SQL>\r\n\r\n"));
    }
}
