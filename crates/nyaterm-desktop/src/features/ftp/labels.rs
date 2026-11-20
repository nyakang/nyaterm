use nyaterm_files::ftp::FtpError;
use rust_i18n::t;

pub(in crate::features) fn error_message(error: &FtpError) -> String {
    match error {
        FtpError::Connection | FtpError::Protocol(_) => t!("ftpErrors.connection"),
        FtpError::AuthenticationFailed | FtpError::AnonymousLoginRejected => {
            t!("ftpErrors.authentication")
        }
        FtpError::Timeout => t!("ftpErrors.timeout"),
        FtpError::Unavailable => t!("ftpErrors.unavailable"),
        FtpError::UnsupportedSettings => t!("ftpErrors.unsupportedSettings"),
        FtpError::LimitExceeded => t!("ftpErrors.limitExceeded"),
        FtpError::NotFound => t!("ftpErrors.notFound"),
        FtpError::InvalidPath => t!("fileExplorer.transferInvalidDestination"),
        FtpError::UnsupportedEncoding => t!("dialog.ftpFilenameEncodingInvalid"),
        FtpError::Cancelled => return nyaterm_files::sftp::SFTP_TRANSFER_CANCELLED.into(),
        FtpError::BackupRetained { backup_path } => {
            t!("ftpErrors.backupRetained", backup = backup_path)
        }
        FtpError::RecoveryRequired {
            temporary_path,
            backup_path,
        } => t!(
            "ftpErrors.recoveryRequired",
            temporary = temporary_path,
            backup = backup_path.as_deref().unwrap_or("—")
        ),
    }
    .into_owned()
}
