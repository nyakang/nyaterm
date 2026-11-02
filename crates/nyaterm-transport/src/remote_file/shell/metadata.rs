use crate::remote_file::{RemoteFileBackendKind, format_permissions, shell_properties};
use crate::sftp::{RemoteFilePath, SftpFileProperties, SftpFileType};

use super::{ShellRemote, shell_quote, shell_quote_path};

/// A failed existence check is not evidence that a target can safely be created.
pub(in crate::remote_file) fn shell_upload_properties(
    shell: &ShellRemote,
    target: &str,
) -> anyhow::Result<Option<SftpFileProperties>> {
    let parent = target.rsplit_once('/').map_or(
        ".",
        |(parent, _)| {
            if parent.is_empty() { "/" } else { parent }
        },
    );
    let output = shell.exec_ok(
        format!(
            "if test -e {target} || test -L {target}; then printf present; \
         elif test -d {parent} && test -x {parent}; then printf missing; else exit 1; fi",
            target = shell_quote_path(target),
            parent = shell_quote_path(parent),
        ),
        None,
    )?;
    if output == b"missing" {
        return Ok(None);
    }
    anyhow::ensure!(
        output == b"present",
        "Unexpected remote upload target response"
    );
    let properties = shell_file_properties(shell, &RemoteFilePath::new(target))?;
    anyhow::ensure!(
        properties.file_type == SftpFileType::File,
        "Upload replacement requires a regular file"
    );
    Ok(Some(properties))
}

pub(in crate::remote_file) fn shell_file_properties(
    shell: &ShellRemote,
    path: &RemoteFilePath,
) -> anyhow::Result<SftpFileProperties> {
    let mut properties = shell_properties(shell, path)?;
    if shell.kind() == RemoteFileBackendKind::ScpNormal {
        // The normal backend has no stat dependency. Read numeric IDs for
        // ownership-preserving saves; property views also use numeric identities.
        let output = shell.exec_ok(
            format!("LC_ALL=C ls -ldn {}", shell_quote_path(&path.display_path)),
            None,
        )?;
        let line = String::from_utf8_lossy(&output);
        let columns = line.split_whitespace().collect::<Vec<_>>();
        properties.uid = columns.get(2).and_then(|value| value.parse().ok());
        properties.gid = columns.get(3).and_then(|value| value.parse().ok());
    }
    Ok(properties)
}

pub(in crate::remote_file) fn shell_file_replace_command(
    properties: &SftpFileProperties,
    temporary: &str,
    target: &str,
) -> anyhow::Result<String> {
    let (Some(uid), Some(gid), Some(mode)) =
        (properties.uid, properties.gid, properties.permissions)
    else {
        anyhow::bail!(
            "Remote replacement refused: original ownership or permissions are unavailable"
        );
    };
    let temporary = shell_quote_path(temporary);
    let target = shell_quote_path(target);
    let mode = mode & 0o7777;
    let symbolic = format_permissions(SftpFileType::File, mode);
    // Skip a no-op chown, restore mode after chown clears special bits, and
    // verify both before mv. A rejected or ignored attribute change must not
    // replace the original. Numeric ls works in both shell backends.
    Ok(format!(
        "(attrs=$(LC_ALL=C ls -ldn {temporary}) && \
         set -f && set -- $attrs && \
         (test \"$3:$4\" = '{uid}:{gid}' || chown {uid}:{gid} {temporary})) && \
         chmod {mode:o} {temporary} && \
         (attrs=$(LC_ALL=C ls -ldn {temporary}) && set -f && set -- $attrs && \
         case \"$1\" in '{symbolic}'*) test \"$3:$4\" = '{uid}:{gid}' ;; *) false ;; esac) && \
         mv -f {temporary} {target}"
    ))
}

pub(in crate::remote_file) fn shell_file_upload_command(
    properties: &SftpFileProperties,
    temporary: &str,
    target: &str,
    byte_count: usize,
) -> anyhow::Result<String> {
    let replace = shell_file_replace_command(properties, temporary, target)?;
    Ok(format!(
        "umask 077; cat > {} && test $(wc -c < {}) -eq {byte_count} && {replace}",
        shell_quote(temporary),
        shell_quote(temporary),
    ))
}
