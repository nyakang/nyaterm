//! Transport categories contain no server-controlled diagnostic text.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloudStorageErrorKind {
    Dns,
    Connect,
    Tls,
    Timeout,
    ConnectionReset,
    PermissionDenied,
    NotFound,
    Conflict,
    Unsupported,
    InvalidConfiguration,
    Unexpected,
}

impl CloudStorageErrorKind {
    pub fn classification(self) -> &'static str {
        match self {
            Self::Dns => "dns",
            Self::Connect => "connect",
            Self::Tls => "tls",
            Self::Timeout => "timeout",
            Self::ConnectionReset => "connection_reset",
            Self::PermissionDenied => "permission_denied",
            Self::NotFound => "not_found",
            Self::Conflict => "conflict",
            Self::Unsupported => "unsupported",
            Self::InvalidConfiguration => "invalid_configuration",
            Self::Unexpected => "remote",
        }
    }

    pub fn is_permanent(self) -> bool {
        matches!(
            self,
            Self::Tls | Self::PermissionDenied | Self::Unsupported | Self::InvalidConfiguration
        )
    }
}

impl std::fmt::Display for CloudStorageErrorKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Dns => "DNS resolution",
            Self::Connect => "connection",
            Self::Tls => "TLS/certificate validation",
            Self::Timeout => "timeout",
            Self::ConnectionReset => "connection reset",
            Self::PermissionDenied => "permission denied",
            Self::NotFound => "remote not found",
            Self::Conflict => "remote conflict",
            Self::Unsupported => "unsupported operation",
            Self::InvalidConfiguration => "invalid configuration",
            Self::Unexpected => "remote storage error",
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::cloud_sync::CloudSyncError;

    #[test]
    fn http_failures_keep_status_categories_and_rate_limit_deadlines() {
        for (status, category) in [
            (401, "authentication"),
            (403, "permission_denied"),
            (404, "not_found"),
            (409, "conflict"),
            (412, "conflict"),
            (429, "rate_limited"),
            (501, "unsupported"),
            (503, "server"),
        ] {
            let error = CloudSyncError::Http {
                provider: "fixture".into(),
                operation: "read".into(),
                status,
                retry_after_seconds: None,
            };
            assert_eq!(error.classification(), category);
        }
        let limited = CloudSyncError::Http {
            provider: "fixture".into(),
            operation: "read".into(),
            status: 403,
            retry_after_seconds: Some(120),
        };
        assert_eq!(limited.classification(), "rate_limited");
        assert_eq!(
            limited.retry_after(),
            Some(std::time::Duration::from_secs(120))
        );
    }
}
