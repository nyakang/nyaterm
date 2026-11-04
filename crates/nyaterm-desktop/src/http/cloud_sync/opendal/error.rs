use std::error::Error as StdError;

use nyaterm_core::CloudSyncError;
use nyaterm_core::cloud_sync::error::CloudStorageErrorKind;
use opendal::{Error, ErrorKind};

/// Only this sanitized source crosses the HTTP/OpenDAL boundary. OpenDAL's
/// own context may contain paths, signed URLs and server response bodies.
#[derive(Debug, Clone)]
pub(super) enum Failure {
    Transport(CloudStorageErrorKind),
    Authentication {
        status: u16,
    },
    CredentialPersistence,
    CredentialsChanged,
    Http {
        status: u16,
        retry_after: Option<u64>,
    },
}

impl std::fmt::Display for Failure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("cloud storage request failed")
    }
}

impl StdError for Failure {}

impl Failure {
    pub(super) fn into_opendal(self) -> Error {
        let kind = match self {
            Self::Transport(CloudStorageErrorKind::Unsupported) => ErrorKind::Unsupported,
            Self::Authentication { .. } => ErrorKind::PermissionDenied,
            Self::Http {
                status: 304 | 412, ..
            } => ErrorKind::ConditionNotMatch,
            Self::Http { status: 404, .. } => ErrorKind::NotFound,
            Self::Http {
                status: 401 | 403, ..
            } => ErrorKind::PermissionDenied,
            Self::Http { status: 409, .. } => ErrorKind::Conflict,
            Self::Http { status: 429, .. } => ErrorKind::RateLimited,
            _ => ErrorKind::Unexpected,
        };
        let temporary = matches!(
            self,
            Self::Transport(
                CloudStorageErrorKind::Dns
                    | CloudStorageErrorKind::Connect
                    | CloudStorageErrorKind::ConnectionReset
                    | CloudStorageErrorKind::Timeout
            ) | Self::Http {
                status: 500 | 502..=599,
                retry_after: None
            }
        );
        let error = Error::new(kind, "cloud storage request failed").set_source(self);
        if temporary {
            error.set_temporary()
        } else {
            error
        }
    }
}

pub(super) fn failure(error: &Error) -> Option<&Failure> {
    let mut source = error.source();
    while let Some(current) = source {
        if let Some(failure) = current.downcast_ref::<Failure>() {
            return Some(failure);
        }
        source = current.source();
    }
    None
}

pub(super) fn map_error(
    provider: &'static str,
    operation: &'static str,
    error: Error,
) -> CloudSyncError {
    match failure(&error) {
        Some(Failure::Authentication { status }) => CloudSyncError::Authentication {
            provider: provider.into(),
            operation: "token refresh".into(),
            status: *status,
        },
        Some(Failure::CredentialPersistence) => CloudSyncError::CredentialPersistence,
        Some(Failure::CredentialsChanged) => CloudSyncError::CredentialsChanged,
        Some(Failure::Http { status: 401, .. }) => CloudSyncError::Authentication {
            provider: provider.into(),
            operation: operation.into(),
            status: 401,
        },
        Some(Failure::Http {
            status,
            retry_after,
        }) => CloudSyncError::Http {
            provider: provider.into(),
            operation: operation.into(),
            status: *status,
            retry_after_seconds: *retry_after,
        },
        Some(Failure::Transport(kind)) => storage_error(provider, operation, *kind),
        None => storage_error(
            provider,
            operation,
            match error.kind() {
                ErrorKind::ConfigInvalid => CloudStorageErrorKind::InvalidConfiguration,
                ErrorKind::Unsupported => CloudStorageErrorKind::Unsupported,
                ErrorKind::PermissionDenied => CloudStorageErrorKind::PermissionDenied,
                ErrorKind::NotFound => CloudStorageErrorKind::NotFound,
                ErrorKind::Conflict | ErrorKind::ConditionNotMatch | ErrorKind::AlreadyExists => {
                    CloudStorageErrorKind::Conflict
                }
                _ if error.message().contains("timeout") => CloudStorageErrorKind::Timeout,
                _ => CloudStorageErrorKind::Unexpected,
            },
        ),
    }
}

pub(super) fn storage_error(
    provider: &'static str,
    operation: &'static str,
    kind: CloudStorageErrorKind,
) -> CloudSyncError {
    CloudSyncError::Storage {
        provider: provider.into(),
        operation: operation.into(),
        kind,
    }
}

pub(super) fn transport_error(error: zed_reqwest::Error) -> Error {
    Failure::Transport(super::super::helpers::http_error_kind(&error)).into_opendal()
}
