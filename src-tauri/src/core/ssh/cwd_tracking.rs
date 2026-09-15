use super::client::SshConnectionHandles;
use super::io::{exec_remote_command, remote_install_command, sh_single_quote};
use super::osc::ShellKind;
use crate::core::SessionManager;
use crate::error::{AppError, AppResult};
use std::sync::Arc;

const TARGET_SHELL_MARKER: &str = "__NYATERM_TARGET_SHELL__=";

fn is_valid_user_name(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value.chars().enumerate().all(|(index, character)| {
            if index == 0 {
                character.is_ascii_alphabetic() || character == '_'
            } else {
                character.is_ascii_alphanumeric() || matches!(character, '_' | '.' | '-')
            }
        })
}

fn normalize_user_token(value: &str) -> Option<String> {
    let value = value.trim_matches(['\'', '"']);
    is_valid_user_name(value).then(|| value.to_string())
}

fn is_environment_assignment(value: &str) -> bool {
    let Some((name, _)) = value.split_once('=') else {
        return false;
    };
    is_valid_user_name(name)
}

fn parse_su_target(tokens: &[&str], mut index: usize) -> String {
    while let Some(token) = tokens.get(index).copied() {
        if token == "--" {
            index += 1;
            continue;
        }
        if token.starts_with('-') {
            index += 1;
            continue;
        }
        if let Some(user) = normalize_user_token(token) {
            return user;
        }
        break;
    }
    "root".to_string()
}

/// Extracts the user that a submitted `su`/`sudo su` command will enter.
///
/// This is deliberately conservative: commands that are not clearly a user
/// switch return `None`, so ordinary sudo commands are never delayed.
fn parse_user_switch_target(command: &str) -> Option<String> {
    let tokens = command.split_whitespace().collect::<Vec<_>>();
    if tokens.is_empty() {
        return None;
    }

    let mut index = 0;
    while tokens.get(index).copied() == Some("env") {
        index += 1;
    }
    while tokens
        .get(index)
        .copied()
        .is_some_and(is_environment_assignment)
    {
        index += 1;
    }

    match tokens.get(index).copied()? {
        "su" => Some(parse_su_target(&tokens, index + 1)),
        "doas" => {
            let su_index =
                (index + 1..tokens.len()).find(|candidate| tokens[*candidate] == "su")?;
            Some(parse_su_target(&tokens, su_index + 1))
        }
        "sudo" => {
            let mut sudo_user = None;
            let mut login_shell = false;
            let mut su_index = None;
            let mut cursor = index + 1;

            while let Some(token) = tokens.get(cursor).copied() {
                if token == "--" {
                    cursor += 1;
                    if tokens.get(cursor).copied() == Some("su") {
                        su_index = Some(cursor);
                    }
                    break;
                }
                if token == "-u" || token == "--user" {
                    sudo_user = tokens
                        .get(cursor + 1)
                        .and_then(|value| normalize_user_token(value));
                    cursor += 2;
                    continue;
                }
                if token == "su" {
                    su_index = Some(cursor);
                    break;
                }
                if token == "-i" || token == "--login" || token.starts_with("-i") {
                    login_shell = true;
                    cursor += 1;
                    continue;
                }
                if token.starts_with('-') {
                    cursor += 1;
                    continue;
                }
                return None;
            }

            if let Some(su_index) = su_index {
                return Some(parse_su_target(&tokens, su_index + 1));
            }
            if login_shell {
                return Some(sudo_user.unwrap_or_else(|| "root".to_string()));
            }
            None
        }
        _ => None,
    }
}

fn build_target_shell_lookup_command(username: &str) -> String {
    let quoted_user = sh_single_quote(username);
    format!(
        "entry=$(getent passwd {quoted_user} 2>/dev/null || awk -F: -v target={quoted_user} '$1 == target {{ print; exit }}' /etc/passwd 2>/dev/null || true); shell=$(printf '%s\\n' \"$entry\" | awk -F: 'NR == 1 {{ print $7 }}'); printf '%s%s\\n' '{TARGET_SHELL_MARKER}' \"$shell\""
    )
}

fn build_target_install_command(username: &str, setup_script: &str) -> String {
    let quoted_user = sh_single_quote(username);
    let quoted_setup = sh_single_quote(setup_script);
    format!(
        "if [ \"$(id -u 2>/dev/null)\" = 0 ]; then su -s /bin/sh {quoted_user} -c {quoted_setup}; else sudo -n -u {quoted_user} -H sh -c {quoted_setup}; fi"
    )
}

/// 在用户切换命令写入交互终端前，静默准备目标用户的 CWD hook。
///
/// 配置通过独立 exec 通道以目标用户身份执行，交互终端只会收到用户
/// 自己输入的 su/sudo 命令，不会出现 NyaTerm 自动注入的长命令。
pub(crate) async fn prepare_terminal_cwd_tracking_for_user_switch(
    manager: Arc<SessionManager>,
    session_id: &str,
    command: &str,
) -> AppResult<bool> {
    let Some(username) = parse_user_switch_target(command) else {
        return Ok(false);
    };

    let ssh_handle = {
        let sessions = manager.sessions.lock().await;
        let Some(session) = sessions.get(session_id) else {
            return Ok(false);
        };
        if !session.info.connected
            || !matches!(session.info.session_type, crate::core::SessionType::SSH)
            || session.info.ssh_runtime_mode == Some(crate::config::SshRuntimeMode::Sftp)
        {
            return Ok(false);
        }
        let Some(handle) = session.ssh_handle.as_ref() else {
            return Ok(false);
        };
        handle
            .clone()
            .downcast::<SshConnectionHandles>()
            .map_err(|_| AppError::Config("Failed to get SSH handle".to_string()))?
    };

    let lookup_output = {
        let handle_mtx = ssh_handle.target_handle();
        let mut handle = handle_mtx.lock().await;
        match exec_remote_command(
            &mut handle,
            &build_target_shell_lookup_command(&username),
            3000,
        )
        .await
        {
            Ok(output) => output,
            Err(error) => {
                tracing::debug!(
                    session_id,
                    target_user = %username,
                    %error,
                    "Could not inspect the target shell before user switch"
                );
                return Ok(false);
            }
        }
    };

    let Some(shell_name) = lookup_output
        .lines()
        .find_map(|line| line.strip_prefix(TARGET_SHELL_MARKER))
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(false);
    };
    let shell = ShellKind::from_name(shell_name);
    if matches!(shell, ShellKind::PosixSh | ShellKind::Unknown) {
        return Ok(false);
    }

    let Some(setup_script) = remote_install_command(shell) else {
        return Ok(false);
    };
    let install_command = build_target_install_command(&username, &setup_script);
    let result = {
        let handle_mtx = ssh_handle.target_handle();
        let mut handle = handle_mtx.lock().await;
        exec_remote_command(&mut handle, &install_command, 5000).await
    };

    match result {
        Ok(_) => {
            tracing::info!(
                session_id,
                target_user = %username,
                shell = ?shell,
                "Prepared CWD tracking before user switch"
            );
            Ok(true)
        }
        Err(error) => {
            // 无权限时不阻止原始 su/sudo 命令，也不向交互终端注入回退脚本。
            tracing::debug!(
                session_id,
                target_user = %username,
                %error,
                "Could not prepare CWD tracking for the target user"
            );
            Ok(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse_user_switch_target;

    #[test]
    fn parses_supported_user_switch_commands_conservatively() {
        assert_eq!(parse_user_switch_target("su iqc"), Some("iqc".to_string()));
        assert_eq!(parse_user_switch_target("su -"), Some("root".to_string()));
        assert_eq!(
            parse_user_switch_target("sudo su -"),
            Some("root".to_string())
        );
        assert_eq!(
            parse_user_switch_target("sudo -u iqc -i"),
            Some("iqc".to_string())
        );
        assert_eq!(
            parse_user_switch_target("env FOO=bar doas su iqc"),
            Some("iqc".to_string())
        );
        assert_eq!(parse_user_switch_target("sudo ls -la"), None);
        assert_eq!(parse_user_switch_target("echo su iqc"), None);
    }
}
