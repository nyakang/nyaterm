use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

/// Private, session-scoped dispatch storage. No profile or persistent shell
/// configuration is changed, and command contents never pass through PSReadLine.
pub struct PowershellDispatch {
    directory: tempfile::TempDir,
}

pub struct PowershellCommandFile {
    path: PathBuf,
    _dispatch: Arc<PowershellDispatch>,
}

impl Drop for PowershellCommandFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

impl PowershellDispatch {
    pub fn new() -> crate::error::AppResult<Arc<Self>> {
        let directory = tempfile::Builder::new().prefix("nyaterm-exec-").tempdir()?;
        // Apply the ACL while the directory is still empty, before writing secrets.
        #[cfg(windows)]
        crate::core::mcp::set_current_user_only(directory.path(), true)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(Arc::new(Self { directory }))
    }

    pub fn init_script(&self) -> String {
        let path = self.directory.path().to_string_lossy().replace('\'', "''");
        // Dot-source both the helper and the payload to preserve interactive
        // session variables and cwd. Only a random hexadecimal token is input.
        format!(
            concat!(
                "function global:__ntx {{ param([string]$ntId); if ($ntId -notmatch '^[a-f0-9]{{16}}$') {{ throw 'Invalid NyaTerm dispatch' }}; . ('{path}' + [IO.Path]::DirectorySeparatorChar + $ntId + '.ps1'); Remove-Variable ntId -ErrorAction Ignore }}; ",
                "try {{ Import-Module PSReadLine -ErrorAction Stop; $ntHistory = (Get-PSReadLineOption).AddToHistoryHandler; ",
                "Set-PSReadLineOption -AddToHistoryHandler ({{ param($line); if ($line -match '^\\. __ntx ''[a-f0-9]{{16}}''$') {{ return $false }}; if ($ntHistory) {{ return $ntHistory.Invoke($line) }}; return $true }}.GetNewClosure()) ",
                "}} catch {{ }}; Remove-Variable ntHistory -ErrorAction Ignore; "
            ),
            path = path
        )
    }

    pub fn prepare(
        self: &Arc<Self>,
        marker_id: &str,
        command: &str,
    ) -> crate::error::AppResult<(String, PowershellCommandFile)> {
        let token = uuid::Uuid::new_v4().simple().to_string()[..16].to_string();
        let path = self.directory.path().join(format!("{token}.ps1"));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        #[cfg(windows)]
        crate::core::mcp::set_current_user_only(&path, false)?;
        let guard = PowershellCommandFile {
            path,
            _dispatch: self.clone(),
        };
        // Windows PowerShell 5.1 needs a BOM when reading UTF-8 script files.
        file.write_all(b"\xef\xbb\xbf")?;
        file.write_all(build_powershell_capture_command(marker_id, command).as_bytes())?;
        file.flush()?;
        // ConPTY treats CR as AcceptLine. A following LF is another PSReadLine
        // editing action and can leave a spurious `>>` continuation prompt.
        Ok((format!(". __ntx '{token}'\r"), guard))
    }
}

#[cfg(test)]
mod powershell_dispatch_tests {
    use super::*;

    #[test]
    fn private_command_files_are_unique_and_follow_execution_lifetime() {
        let dispatch = PowershellDispatch::new().unwrap();
        let directory = dispatch.directory.path().to_path_buf();
        #[cfg(windows)]
        crate::core::mcp::assert_current_user_only(&directory, true);
        let (first, first_file) = dispatch.prepare("first", "Write-Output '秘密'").unwrap();
        let (second, second_file) = dispatch.prepare("second", "Write-Output 'second'").unwrap();
        assert_ne!(first, second);
        assert!(first.len() < 50);
        let first_path = first_file.path.clone();
        #[cfg(windows)]
        crate::core::mcp::assert_current_user_only(&first_path, false);
        assert!(
            std::fs::read(&first_path)
                .unwrap()
                .starts_with(b"\xef\xbb\xbf")
        );
        drop(first_file);
        assert!(!first_path.exists());
        drop(dispatch);
        assert!(directory.exists());
        drop(second_file);
        assert!(!directory.exists());
    }
}
