use std::sync::Arc;

use crate::core::session::{SessionInfo, SessionManager, SessionType};
use crate::core::sftp::{self, FileEntry, FileProperties, RemoteTextFile, WriteRemoteTextResult};
use crate::error::{AppError, AppResult};

use super::RiskAssessment;
use super::policy::risk;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SftpRiskOperation {
    Read,
    Write,
    Mkdir,
    Rename,
    Delete,
    Chmod,
}

pub fn assess_sftp_risk(
    operation: SftpRiskOperation,
    path: &str,
    destination_path: Option<&str>,
    force: bool,
    mode: Option<&str>,
) -> RiskAssessment {
    if operation == SftpRiskOperation::Delete {
        return risk(
            crate::config::RiskLevel::High,
            "remote path deletion is destructive",
            false,
        );
    }
    if operation == SftpRiskOperation::Read {
        return risk(
            crate::config::RiskLevel::Medium,
            "remote file access may expose sensitive data",
            true,
        );
    }
    let (path, path_has_parent_traversal) = normalize_remote_path(path);
    let (destination, destination_has_parent_traversal) = destination_path
        .map(normalize_remote_path)
        .unwrap_or_default();
    let sensitive =
        is_sensitive_path(&path) || (!destination.is_empty() && is_sensitive_path(&destination));
    if force {
        return risk(
            crate::config::RiskLevel::High,
            "force write bypasses optimistic concurrency protection",
            false,
        );
    }
    if sensitive || path_has_parent_traversal || destination_has_parent_traversal {
        return risk(
            crate::config::RiskLevel::High,
            "mutation targets a sensitive or ambiguously resolved remote path",
            false,
        );
    }
    if operation == SftpRiskOperation::Chmod && mode.is_some_and(is_dangerous_mode) {
        return risk(
            crate::config::RiskLevel::Medium,
            "permission change grants broad remote access",
            true,
        );
    }
    risk(
        crate::config::RiskLevel::Medium,
        "ordinary remote filesystem mutation",
        true,
    )
}

fn normalize_remote_path(path: &str) -> (String, bool) {
    let path = path.trim().replace('\\', "/");
    let absolute = path.starts_with('/');
    let home = path == "~" || path.starts_with("~/");
    let mut parent_traversal = false;
    let mut parts = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parent_traversal = true;
                if parts.last().is_some_and(|part| *part != "~") {
                    parts.pop();
                }
            }
            _ => parts.push(part),
        }
    }
    let joined = parts.join("/");
    let normalized = if absolute && joined.is_empty() {
        "/".to_string()
    } else if absolute {
        format!("/{joined}")
    } else if home && !joined.starts_with('~') {
        format!("~/{joined}")
    } else {
        joined
    };
    let normalized = if normalized == "/" {
        normalized
    } else {
        normalized.trim_end_matches('/').to_string()
    };
    (normalized, parent_traversal)
}

fn is_sensitive_path(path: &str) -> bool {
    const SYSTEM_ROOTS: &[&str] = &[
        "/etc", "/boot", "/bin", "/sbin", "/usr", "/lib", "/lib64", "/var/lib", "/root",
    ];
    if path == "/"
        || SYSTEM_ROOTS
            .iter()
            .any(|root| path == *root || path.starts_with(&format!("{root}/")))
    {
        return true;
    }
    let components = path.split('/').collect::<Vec<_>>();
    if components.contains(&".ssh")
        || path == "~/.ssh"
        || path.starts_with("~/.ssh/")
        || path == ".ssh"
        || path.starts_with(".ssh/")
        || path.contains("/.ssh/")
    {
        return true;
    }
    let basename = components.last().copied().unwrap_or_default();
    matches!(
        basename,
        "authorized_keys" | "sshd_config" | "sudoers" | "passwd" | "shadow" | "group" | "crontab"
    ) || components.iter().any(|part| {
        matches!(
            *part,
            "sudoers.d"
                | "systemd"
                | "nginx"
                | "cron"
                | "cron.d"
                | "cron.daily"
                | "cron.hourly"
                | "cron.monthly"
                | "cron.weekly"
        )
    }) || matches!(
        path.rsplit_once('.').map(|(_, extension)| extension),
        Some("service" | "socket" | "timer" | "target")
    )
}

fn is_dangerous_mode(mode: &str) -> bool {
    let mode = mode.trim().strip_prefix("0o").unwrap_or(mode.trim());
    u32::from_str_radix(mode.trim_start_matches('0'), 8)
        .is_ok_and(|value| matches!(value, 0o666 | 0o777))
}

pub fn is_available(info: &SessionInfo) -> bool {
    info.connected && info.session_type == SessionType::SSH && info.remote_file_browser_enabled
}

pub async fn require_available(
    manager: &SessionManager,
    session_id: &str,
) -> AppResult<SessionInfo> {
    let info = manager.session_info(session_id).await?;
    if is_available(&info) {
        Ok(info)
    } else {
        Err(AppError::Config(
            "SFTP is not available for this session.".to_string(),
        ))
    }
}

pub async fn home(manager: Arc<SessionManager>, session_id: &str) -> AppResult<String> {
    require_available(&manager, session_id).await?;
    sftp::get_home_dir(manager, session_id).await
}

pub async fn list(
    manager: Arc<SessionManager>,
    session_id: &str,
    path: &str,
) -> AppResult<Vec<FileEntry>> {
    require_available(&manager, session_id).await?;
    sftp::list_remote_dir(manager, session_id, path, None).await
}

pub async fn stat(
    manager: Arc<SessionManager>,
    session_id: &str,
    path: &str,
) -> AppResult<FileProperties> {
    require_available(&manager, session_id).await?;
    sftp::get_file_properties(manager, session_id, path, None).await
}

pub async fn read_text(
    manager: Arc<SessionManager>,
    session_id: &str,
    path: &str,
    max_bytes: u64,
) -> AppResult<RemoteTextFile> {
    require_available(&manager, session_id).await?;
    sftp::read_remote_file_text(manager, session_id, path, max_bytes).await
}

#[allow(clippy::too_many_arguments)]
pub async fn write_text(
    manager: Arc<SessionManager>,
    session_id: &str,
    path: &str,
    content: &str,
    expected_mtime: Option<u64>,
    expected_size: Option<u64>,
    expected_hash: Option<&str>,
    force: bool,
) -> AppResult<WriteRemoteTextResult> {
    require_available(&manager, session_id).await?;
    sftp::write_remote_file_text(
        manager,
        session_id,
        path,
        content,
        expected_mtime,
        expected_size,
        expected_hash,
        force,
    )
    .await
}

pub async fn mkdir(
    manager: Arc<SessionManager>,
    session_id: &str,
    path: &str,
    mode: Option<String>,
) -> AppResult<()> {
    require_available(&manager, session_id).await?;
    sftp::create_remote_dir(manager, session_id, path, mode).await
}

pub async fn rename(
    manager: Arc<SessionManager>,
    session_id: &str,
    old_path: &str,
    new_path: &str,
) -> AppResult<()> {
    require_available(&manager, session_id).await?;
    sftp::rename_remote_file(manager, session_id, old_path, new_path, None, None).await
}

pub async fn delete(manager: Arc<SessionManager>, session_id: &str, path: &str) -> AppResult<()> {
    require_available(&manager, session_id).await?;
    sftp::delete_remote_file(manager, session_id, path, None).await
}

pub async fn chmod(
    manager: Arc<SessionManager>,
    session_id: &str,
    path: &str,
    mode: &str,
) -> AppResult<()> {
    require_available(&manager, session_id).await?;
    sftp::chmod_remote_file(manager, session_id, path, mode).await
}

#[cfg(test)]
mod tests {
    use crate::config::RiskLevel;

    use super::*;

    #[test]
    fn assesses_remote_filesystem_mutations_dynamically() {
        let ordinary = assess_sftp_risk(
            SftpRiskOperation::Write,
            "/home/alice/notes.txt",
            None,
            false,
            None,
        );
        assert_eq!(ordinary.level, RiskLevel::Medium);
        assert!(ordinary.auto_executable);

        for path in [
            "/etc/nginx/nginx.conf",
            "/home/alice/.ssh/authorized_keys",
            "/home/alice/.ssh",
            "~/.ssh/config",
            "/var/lib/app/state",
            "/tmp/example.service",
        ] {
            let assessment = assess_sftp_risk(SftpRiskOperation::Write, path, None, false, None);
            assert_eq!(assessment.level, RiskLevel::High, "path: {path}");
            assert!(!assessment.auto_executable);
        }

        assert_eq!(
            assess_sftp_risk(
                SftpRiskOperation::Write,
                "/home/alice/notes.txt",
                None,
                true,
                None,
            )
            .level,
            RiskLevel::High
        );
        assert_eq!(
            assess_sftp_risk(
                SftpRiskOperation::Rename,
                "/home/alice/config",
                Some("/etc/app.conf"),
                false,
                None,
            )
            .level,
            RiskLevel::High
        );
        assert_eq!(
            assess_sftp_risk(
                SftpRiskOperation::Chmod,
                "~/.ssh/authorized_keys",
                None,
                false,
                Some("0777"),
            )
            .level,
            RiskLevel::High
        );
        assert_eq!(
            assess_sftp_risk(
                SftpRiskOperation::Delete,
                "/home/alice/notes.txt",
                None,
                false,
                None,
            )
            .level,
            RiskLevel::High
        );
    }
}
