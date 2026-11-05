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
