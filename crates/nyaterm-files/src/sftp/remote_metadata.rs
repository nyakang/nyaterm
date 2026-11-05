//! SFTP path tokens, metadata formatting, and remote identity lookup.
use super::{RemoteFilePath, SftpFileType, SftpPathCodec, SshMultiplexHandle, SshSessionConfig};
use nyaterm_transport::ssh_execution::SshExecService;
use std::collections::HashMap;
use std::time::Duration;

const IDENTITY_LOOKUP_TIMEOUT: Duration = Duration::from_secs(10);

pub(super) fn attrs_to_sftp_file_type(
    attrs: &russh_sftp::protocol::FileAttributes,
) -> SftpFileType {
    match attrs.file_type() {
        russh_sftp::protocol::FileType::File => SftpFileType::File,
        russh_sftp::protocol::FileType::Dir => SftpFileType::Directory,
        russh_sftp::protocol::FileType::Symlink => SftpFileType::Symlink,
        russh_sftp::protocol::FileType::Other => SftpFileType::Other,
    }
}

pub(super) fn remote_file_name(path: &str) -> String {
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(path)
        .to_string()
}

pub(super) fn remote_file_path_bytes(
    codec: &SftpPathCodec,
    path: &RemoteFilePath,
) -> anyhow::Result<Vec<u8>> {
    path.raw_path()?
        .map(Ok)
        .unwrap_or_else(|| codec.encode_path(&path.display_path))
}

pub(super) fn rename_target_path_bytes(
    codec: &SftpPathCodec,
    old_path: &RemoteFilePath,
    new_path: &RemoteFilePath,
) -> anyhow::Result<Vec<u8>> {
    if let Some(raw) = new_path.raw_path()? {
        return Ok(raw);
    }
    if let Some(old_raw) = old_path.raw_path()?
        && remote_parent(&old_path.display_path) == remote_parent(&new_path.display_path)
        && let Some(name) = new_path.display_path.rsplit('/').next()
    {
        let mut parent = old_raw;
        if let Some(index) = parent.iter().rposition(|byte| *byte == b'/') {
            parent.truncate(index + 1);
            parent.extend_from_slice(&codec.encode_path(name)?);
            return Ok(parent);
        }
    }
    codec.encode_path(&new_path.display_path)
}

pub(super) fn remote_parent(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    trimmed
        .rsplit_once('/')
        .map(|(parent, _)| if parent.is_empty() { "/" } else { parent })
        .unwrap_or(".")
}

pub(super) fn ensure_safe_remote_delete_target(path: &str) -> anyhow::Result<()> {
    let trimmed = path.trim();
    if trimmed.is_empty()
        || matches!(trimmed, "/" | "." | "..")
        || trimmed.split('/').any(|part| part == "..")
    {
        anyhow::bail!("refusing to recursively delete unsafe remote path: {path}");
    }
    Ok(())
}

pub(super) fn format_sftp_permissions(file_type: SftpFileType, mode: u32) -> String {
    let mut output = String::with_capacity(10);
    output.push(match file_type {
        SftpFileType::Directory => 'd',
        SftpFileType::Symlink => 'l',
        _ => '-',
    });
    for (index, shift) in [6, 3, 0].into_iter().enumerate() {
        let bits = (mode >> shift) & 0o7;
        output.push(if bits & 0o4 != 0 { 'r' } else { '-' });
        output.push(if bits & 0o2 != 0 { 'w' } else { '-' });
        let execute = bits & 0o1 != 0;
        let special = match index {
            0 => mode & 0o4000 != 0,
            1 => mode & 0o2000 != 0,
            _ => mode & 0o1000 != 0,
        };
        output.push(match (index, execute, special) {
            (2, true, true) => 't',
            (2, false, true) => 'T',
            (_, true, true) => 's',
            (_, false, true) => 'S',
            (_, true, false) => 'x',
            (_, false, false) => '-',
        });
    }
    output
}

pub(super) fn ensure_remote_text_bytes(bytes: &[u8], max_bytes: u64) -> anyhow::Result<()> {
    if bytes.len() as u64 > max_bytes {
        anyhow::bail!(
            "File is too large to open as text ({} bytes > {} bytes)",
            bytes.len(),
            max_bytes
        );
    }
    if bytes.contains(&0) {
        anyhow::bail!("Only text files can be opened in the internal editor");
    }
    Ok(())
}

pub(super) fn resolve_remote_user_value(
    config: &SshSessionConfig,
    multiplex: Option<SshMultiplexHandle>,
    value: Option<&str>,
) -> anyhow::Result<Option<u32>> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    if let Ok(uid) = value.parse::<u32>() {
        return Ok(Some(uid));
    }
    let output = run_remote_identity_command(
        config,
        multiplex,
        format!("id -u {}", shell_quote(value)),
        "resolve remote user",
    )?;
    Ok(Some(output.trim().parse()?))
}

pub(super) fn resolve_remote_group_value(
    config: &SshSessionConfig,
    multiplex: Option<SshMultiplexHandle>,
    value: Option<&str>,
) -> anyhow::Result<Option<u32>> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    if let Ok(gid) = value.parse::<u32>() {
        return Ok(Some(gid));
    }
    let output = run_remote_identity_command(
        config,
        multiplex,
        format!(
            "getent group {} | awk -F: 'NR==1 {{print $3}}'",
            shell_quote(value)
        ),
        "resolve remote group",
    )?;
    Ok(Some(output.trim().parse()?))
}

pub(super) fn resolve_remote_user_name(
    config: &SshSessionConfig,
    multiplex: Option<SshMultiplexHandle>,
    uid: Option<u32>,
) -> Option<String> {
    let uid = uid?;
    run_remote_identity_command(
        config,
        multiplex,
        format!("getent passwd {uid} | awk -F: 'NR==1 {{print $1}}'"),
        "resolve remote uid",
    )
    .ok()
    .map(|value| value.trim().to_string())
    .filter(|value| !value.is_empty())
}

pub(super) fn resolve_remote_group_name(
    config: &SshSessionConfig,
    multiplex: Option<SshMultiplexHandle>,
    gid: Option<u32>,
) -> Option<String> {
    let gid = gid?;
    run_remote_identity_command(
        config,
        multiplex,
        format!("getent group {gid} | awk -F: 'NR==1 {{print $1}}'"),
        "resolve remote gid",
    )
    .ok()
    .map(|value| value.trim().to_string())
    .filter(|value| !value.is_empty())
}

pub(super) fn resolve_remote_identity_names(
    config: &SshSessionConfig,
    multiplex: Option<SshMultiplexHandle>,
    database: &'static str,
    ids: &[u32],
) -> HashMap<u32, String> {
    let mut ids = ids.to_vec();
    ids.sort_unstable();
    ids.dedup();
    if ids.is_empty() {
        return HashMap::new();
    }
    let keys = ids.iter().map(u32::to_string).collect::<Vec<_>>().join(" ");
    let Ok(output) = run_remote_identity_command(
        config,
        multiplex,
        format!("getent {database} {keys}"),
        "resolve remote identities",
    ) else {
        return HashMap::new();
    };
    output
        .lines()
        .filter_map(|line| {
            let fields = line.split(':').collect::<Vec<_>>();
            let id = fields.get(2)?.parse().ok()?;
            Some((id, fields.first()?.to_string()))
        })
        .collect()
}

fn run_remote_identity_command(
    config: &SshSessionConfig,
    multiplex: Option<SshMultiplexHandle>,
    command: String,
    context: &'static str,
) -> anyhow::Result<String> {
    let service = match multiplex {
        Some(multiplex) => SshExecService::with_multiplex(config.clone(), multiplex)?,
        None => SshExecService::new(config.clone()),
    };
    let output = service.run_command(command, IDENTITY_LOOKUP_TIMEOUT)?;
    let status = output.exit_status.unwrap_or(1);
    if status != 0 {
        anyhow::bail!("{context} failed: {}", output.stderr.trim());
    }
    Ok(output.stdout)
}

fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}
