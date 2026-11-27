#[derive(Clone, PartialEq, Eq, thiserror::Error)]
pub enum FtpError {
    #[error("Remote file changed; confirm replacement before uploading")]
    Conflict,
    #[error("FTP transfer paused")]
    Paused,
    #[error("FTP server does not support restarting this transfer")]
    RestartUnsupported,
    #[error("FTP source changed during transfer")]
    SourceChanged,
    #[error("FTP TLS negotiation or certificate validation failed")]
    Tls,

    #[error("FTP connection failed")]
    Connection,
    #[error("FTP authentication failed")]
    AuthenticationFailed,
    #[error("FTP server rejected anonymous login")]
    AnonymousLoginRejected,
    #[error("FTP protocol error: {0}")]
    Protocol(String),
    #[error("FTP filename encoding is not supported")]
    UnsupportedEncoding,
    #[error("FTP path is invalid")]
    InvalidPath,
    #[error("FTP service is unavailable")]
    Unavailable,
    #[error("FTP transfer was cancelled")]
    Cancelled,
    #[error("FTP operation timed out")]
    Timeout,
    #[error("FTP file is too large to open or preview")]
    LimitExceeded,
    #[error("FTP file was not found")]
    NotFound,
    #[error("FTP settings are unsupported")]
    UnsupportedSettings,
    #[error("FTP file saved; old backup retained at {backup_path}")]
    BackupRetained { backup_path: String },
    #[error("FTP replacement needs recovery; new file: {temporary_path}; backup: {backup_path:?}")]
    RecoveryRequired {
        temporary_path: String,
        backup_path: Option<String>,
    },
}

// Recovery paths and server responses are UI data, never diagnostic fields.
impl std::fmt::Debug for FtpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code())
    }
}

impl FtpError {
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::Connection | Self::Timeout | Self::Unavailable)
    }
    pub fn code(&self) -> &'static str {
        match self {
            Self::Conflict => "FtpEditConflict",
            Self::Paused => "FtpPaused",
            Self::RestartUnsupported => "FtpRestartUnsupported",
            Self::SourceChanged => "FtpSourceChanged",
            Self::Tls => "FtpTlsFailed",
            Self::Connection => "FtpConnectionFailed",
            Self::AuthenticationFailed => "FtpAuthenticationFailed",
            Self::AnonymousLoginRejected => "FtpAnonymousLoginRejected",
            Self::Protocol(_) => "FtpProtocolError",
            Self::UnsupportedEncoding => "FtpUnsupportedEncoding",
            Self::InvalidPath => "FtpInvalidPath",
            Self::Unavailable => "FtpUnavailable",
            Self::Cancelled => "FtpTransferCancelled",
            Self::Timeout => "FtpTimeout",
            Self::LimitExceeded => "FtpLimitExceeded",
            Self::NotFound => "FtpNotFound",
            Self::UnsupportedSettings => "FtpUnsupportedSettings",
            Self::BackupRetained { .. } => "FtpBackupRetained",
            Self::RecoveryRequired { .. } => "FtpRecoveryRequired",
        }
    }
}

impl From<suppaftp::FtpError> for FtpError {
    fn from(error: suppaftp::FtpError) -> Self {
        match error {
            suppaftp::FtpError::ConnectionError(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                ) =>
            {
                Self::Timeout
            }
            suppaftp::FtpError::UnexpectedResponse(response) if response.status as u32 == 421 => {
                Self::Connection
            }
            suppaftp::FtpError::SecureError(_) => Self::Tls,
            suppaftp::FtpError::BadResponse => Self::Connection,
            suppaftp::FtpError::UnexpectedResponse(response) => {
                Self::Protocol(format!("unexpected response {}", response.status as u32))
            }
            suppaftp::FtpError::ConnectionError(_) => Self::Connection,
            _ => Self::Protocol("operation failed".into()),
        }
    }
}
