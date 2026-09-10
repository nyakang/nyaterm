use nyaterm_transport::SftpTransferOptions;

use crate::features::NyaTermApp;

impl NyaTermApp {
    pub(in crate::features) fn sftp_transfer_options(&self) -> SftpTransferOptions {
        SftpTransferOptions::default()
            .with_download_threads(self.settings.summary().transfer_download_threads as usize)
            .with_buffer_size_bytes(self.settings.summary().transfer_buffer_size as usize * 1024)
            .with_max_retries(self.settings.summary().transfer_max_retries)
            .with_preserve_timestamps(self.settings.summary().transfer_preserve_timestamps)
            .with_default_file_permissions(
                &self.settings.summary().transfer_default_file_permissions,
            )
            .with_resume_broken_transfer(self.settings.summary().transfer_resume_broken_transfer)
            .with_directory_upload_threads(self.settings.summary().transfer_upload_threads as usize)
    }
}
