use std::time::Duration;

use opendal::layers::{RetryLayer, TimeoutLayer, TracingLayer};
use opendal::services::{Webdav, S3};
use opendal::{ErrorKind, Operator};

use crate::config::CloudSyncSettings;
use crate::error::{AppError, AppResult};

use super::remote::remote_path;

pub(super) fn build_operator(settings: &CloudSyncSettings) -> AppResult<Operator> {
    match settings.provider.as_str() {
        "webdav" => {
            let mut builder = Webdav::default().endpoint(&settings.webdav.endpoint);
            if !settings.webdav.root.trim().is_empty() {
                builder = builder.root(&settings.webdav.root);
            }
            if !settings.webdav.username.trim().is_empty() {
                builder = builder.username(&settings.webdav.username);
            }
            if let Some(password) = settings
                .webdav
                .password
                .as_deref()
                .filter(|value| !value.is_empty())
            {
                builder = builder.password(password);
            }
            Ok(Operator::new(builder)
                .map_err(map_storage_error)?
                .layer(
                    TimeoutLayer::new()
                        .with_timeout(Duration::from_secs(30))
                        .with_io_timeout(Duration::from_secs(30)),
                )
                .layer(RetryLayer::new().with_max_times(3))
                .layer(TracingLayer)
                .finish())
        }
        "s3" => {
            let mut builder = S3::default().bucket(&settings.s3.bucket);
            if !settings.s3.endpoint.trim().is_empty() {
                builder = builder.endpoint(&settings.s3.endpoint);
            }
            if !settings.s3.region.trim().is_empty() {
                builder = builder.region(&settings.s3.region);
            }
            if !settings.s3.root.trim().is_empty() {
                builder = builder.root(&settings.s3.root);
            }
            if let Some(access_key_id) = settings
                .s3
                .access_key_id
                .as_deref()
                .filter(|value| !value.is_empty())
            {
                builder = builder.access_key_id(access_key_id);
            }
            if let Some(secret_access_key) = settings
                .s3
                .secret_access_key
                .as_deref()
                .filter(|value| !value.is_empty())
            {
                builder = builder.secret_access_key(secret_access_key);
            }
            if let Some(session_token) = settings
                .s3
                .session_token
                .as_deref()
                .filter(|value| !value.is_empty())
            {
                builder = builder.session_token(session_token);
            }
            if settings.s3.virtual_host_style {
                builder = builder.enable_virtual_host_style();
            }
            Ok(Operator::new(builder)
                .map_err(map_storage_error)?
                .layer(
                    TimeoutLayer::new()
                        .with_timeout(Duration::from_secs(30))
                        .with_io_timeout(Duration::from_secs(30)),
                )
                .layer(RetryLayer::new().with_max_times(3))
                .layer(TracingLayer)
                .finish())
        }
        other => Err(AppError::Config(format!(
            "Unsupported cloud provider '{}'",
            other
        ))),
    }
}

pub(super) async fn ensure_remote_layout(op: &Operator, base_root: &str) -> AppResult<()> {
    op.create_dir(&remote_path(base_root, super::remote::SYNC_SNAPSHOTS_DIR))
        .await
        .map_err(map_storage_error)?;
    op.create_dir(&remote_path(
        base_root,
        super::remote::BACKUPS_SNAPSHOTS_DIR,
    ))
    .await
    .map_err(map_storage_error)?;
    Ok(())
}

pub(super) fn map_storage_error(error: opendal::Error) -> AppError {
    let raw = error.to_string();
    if let Some(message) = map_webdav_auth_error(&raw) {
        return AppError::Config(message);
    }

    let label = match error.kind() {
        ErrorKind::NotFound => "not found",
        ErrorKind::PermissionDenied => "permission denied",
        ErrorKind::ConfigInvalid => "invalid config",
        ErrorKind::Unsupported => "unsupported",
        ErrorKind::RateLimited => "rate limited",
        _ => "unexpected error",
    };
    AppError::Config(format!("cloud storage {label}: {raw}"))
}

fn map_webdav_auth_error(raw: &str) -> Option<String> {
    let lower = raw.to_ascii_lowercase();
    let is_webdav = lower.contains("service: webdav");
    let is_unauthorized = lower.contains("status: 401") || lower.contains("401 unauthorized");

    if is_webdav && is_unauthorized {
        return Some(
            "WebDAV authentication failed (401 Unauthorized). NyaTerm currently supports WebDAV Basic/Bearer authentication only and does not support Apache Digest auth. If you are using bytemark/webdav, change AUTH_TYPE to Basic and prefer HTTPS; otherwise verify the username and password."
                .to_string(),
        );
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn webdav_401_error_reports_digest_hint() {
        let message = map_webdav_auth_error(
            "Unexpected (persistent) at stat, context: { service: webdav, response: Parts { status: 401 } } => 401 Unauthorized",
        );

        assert!(message.is_some());
        assert!(message
            .unwrap()
            .contains("does not support Apache Digest auth"));
    }

    #[test]
    fn non_webdav_error_does_not_report_digest_hint() {
        let message = map_webdav_auth_error(
            "Unexpected (persistent) at stat, context: { service: s3, response: Parts { status: 401 } } => 401 Unauthorized",
        );

        assert!(message.is_none());
    }
}
