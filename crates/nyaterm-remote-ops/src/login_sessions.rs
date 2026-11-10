//! Logged-in sessions of the machine running the application, for the local
//! Resources panel. SSH hosts report theirs through the stats script.
//!
//! Unix uses `who` (the same records, parsed by the same code, as the SSH path).
//! Windows asks the terminal-services API directly, which also works on editions
//! without `quser`.

use crate::stats::{LoggedInSession, LoggedInUser, LoggedInUsersStatus, parse_login_session};
use std::collections::BTreeMap;

#[cfg(windows)]
mod windows_wts;

pub struct LocalUsers {
    pub status: LoggedInUsersStatus,
    pub users: Vec<LoggedInUser>,
    pub sessions: Vec<LoggedInSession>,
}

pub fn collect_local() -> LocalUsers {
    match sessions() {
        Ok(mut sessions) => {
            sessions.sort_by(|a, b| (&a.name, &a.terminal).cmp(&(&b.name, &b.terminal)));
            LocalUsers {
                status: LoggedInUsersStatus::Available,
                users: count_users(&sessions),
                sessions,
            }
        }
        #[cfg(not(windows))]
        Err(LoginSessionsError::Unavailable) => LocalUsers {
            status: LoggedInUsersStatus::Unavailable,
            users: Vec::new(),
            sessions: Vec::new(),
        },
        Err(LoginSessionsError::Failed) => LocalUsers {
            status: LoggedInUsersStatus::Failed,
            users: Vec::new(),
            sessions: Vec::new(),
        },
    }
}

enum LoginSessionsError {
    #[cfg(not(windows))]
    Unavailable,
    Failed,
}

fn count_users(sessions: &[LoggedInSession]) -> Vec<LoggedInUser> {
    let mut counts: BTreeMap<&str, u32> = BTreeMap::new();
    for session in sessions {
        *counts.entry(session.name.as_str()).or_default() += 1;
    }
    counts
        .into_iter()
        .map(|(name, sessions)| LoggedInUser {
            name: name.to_string(),
            sessions,
        })
        .collect()
}

#[cfg(unix)]
fn sessions() -> Result<Vec<LoggedInSession>, LoginSessionsError> {
    let output = std::process::Command::new("who")
        .env("LC_ALL", "C")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .map_err(|_| LoginSessionsError::Unavailable)?;
    if !output.status.success() {
        return Err(LoginSessionsError::Failed);
    }
    Ok(parse_who(&String::from_utf8_lossy(&output.stdout)))
}

#[cfg(windows)]
fn sessions() -> Result<Vec<LoggedInSession>, LoginSessionsError> {
    windows_wts::sessions().map_err(|_| LoginSessionsError::Failed)
}

#[cfg(not(any(unix, windows)))]
fn sessions() -> Result<Vec<LoggedInSession>, LoginSessionsError> {
    Err(LoginSessionsError::Unavailable)
}

/// Records that do not parse are skipped, never fabricated.
pub fn parse_who(output: &str) -> Vec<LoggedInSession> {
    output
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(parse_login_session)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{count_users, parse_who};

    #[test]
    fn who_lines_become_sessions_with_counts_per_user() {
        let sessions = parse_who(
            "me       console      Nov  8 13:33\nme       ttys001      Nov  8 14:02 (10.0.0.4)\nroot     pts/0        2026-11-08 09:00 (host)\n\ngarbage(\n",
        );
        assert_eq!(sessions.len(), 3, "the malformed line is dropped");
        assert_eq!(sessions[1].source.as_deref(), Some("10.0.0.4"));
        let users = count_users(&sessions);
        assert_eq!(users.iter().find(|u| u.name == "me").unwrap().sessions, 2);
        assert_eq!(users.iter().find(|u| u.name == "root").unwrap().sessions, 1);
    }

    #[cfg(unix)]
    #[test]
    fn the_live_who_command_is_available_on_unix() {
        let users = super::collect_local();
        assert_ne!(
            users.status,
            crate::stats::LoggedInUsersStatus::Failed,
            "`who` should run on a unix machine"
        );
    }
}
