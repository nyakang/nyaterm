use crate::features::NyaTermApp;
use nyaterm_files::sftp_transfer_types::SftpTransferOptions;

impl NyaTermApp {
    pub(in crate::features) fn sftp_transfer_options(&self) -> SftpTransferOptions {
        SftpTransferOptions::default()
            .with_buffer_size_bytes(self.settings.summary().transfer_buffer_size as usize * 1024)
            .with_max_retries(self.settings.summary().transfer_max_retries)
            .with_preserve_timestamps(self.settings.summary().transfer_preserve_timestamps)
            .with_default_file_permissions(
                &self.settings.summary().transfer_default_file_permissions,
            )
            .with_resume_broken_transfer(self.settings.summary().transfer_resume_broken_transfer)
    }
}

pub(in crate::features) fn path_options(
    policy: nyaterm_files::sftp_transfer_types::SftpDuplicatePolicy,
    resolver: Option<std::sync::Arc<dyn nyaterm_files::sftp_transfer_types::SftpDuplicateResolver>>,
    transfer: SftpTransferOptions,
) -> nyaterm_files::sftp_transfer_types::SftpPathTransferOptions {
    nyaterm_files::sftp_transfer_types::SftpPathTransferOptions::new(policy, resolver, transfer)
        .with_staging_permissions(|path| {
            #[cfg(windows)]
            {
                crate::platform::windows_acl::set_current_user_only(path, true)
            }
            #[cfg(not(windows))]
            {
                let _ = path;
                Ok(())
            }
        })
}
