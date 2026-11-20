#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FtpError {
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
}

impl FtpError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Connection => "FtpConnectionFailed",
            Self::AuthenticationFailed => "FtpAuthenticationFailed",
            Self::AnonymousLoginRejected => "FtpAnonymousLoginRejected",
            Self::Protocol(_) => "FtpProtocolError",
            Self::UnsupportedEncoding => "FtpUnsupportedEncoding",
            Self::InvalidPath => "FtpInvalidPath",
            Self::Unavailable => "FtpUnavailable",
            Self::Cancelled => "FtpTransferCancelled",
        }
    }
}

impl From<suppaftp::FtpError> for FtpError {
    fn from(error: suppaftp::FtpError) -> Self {
        Self::Protocol(error.to_string())
    }
}
