//! Storage-only bridge; protocol, caches, credentials and cancellation stay in
//! their existing owners. Construct/use/drop only inside desktop blocking jobs.

mod drive;
mod drive_http;
mod error;
mod transport;
mod worker;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use nyaterm_core::cloud_sync::error::CloudStorageErrorKind;
use nyaterm_core::cloud_sync::io::{CloudObjectMetadata, CloudObjectRead, CloudSyncIoStats};
use nyaterm_core::{CloudSyncError, CloudSyncRemote, S3SyncSettings, WebdavSyncSettings};
use opendal::layers::{RetryLayer, TimeoutLayer};
use opendal::services::{S3, Webdav};
use opendal::{Buffer, ErrorKind, HttpTransport, HttpTransporter, OperationContext, Operator};
use tracing::Instrument;

use self::error::{Failure, failure, map_error, storage_error};
use self::transport::Transport;
use self::worker::Worker;
use super::s3::NativeS3Remote;
use super::webdav::NativeWebdavRemote;

// OpenDAL 0.59 requires timestamps in directory/list responses and accepts XML
// shapes our supported services do not guarantee. Keep the existing strict
// listing and recursive MKCOL contracts as fixed capability adapters.
enum DirectoryAdapter {
    Webdav(NativeWebdavRemote),
    S3(NativeS3Remote),
}

pub use drive::OpenDalDriveRemote;

pub struct OpenDalRemote {
    operator: Operator,
    transport: Transport,
    stats: Arc<Mutex<CloudSyncIoStats>>,
    worker: Worker<DirectoryAdapter>,
    provider: &'static str,
}

impl OpenDalRemote {
    pub fn webdav(
        settings: &WebdavSyncSettings,
        stats: Arc<Mutex<CloudSyncIoStats>>,
    ) -> Result<Self, CloudSyncError> {
        let endpoint = endpoint(&settings.endpoint, "webdav")?;
        let root = path(&settings.root)?;
        let transport = Transport::new(stats.clone(), Some(settings))
            .map_err(|error| map_error("webdav", "configure", error))?;
        // Transport owns authentication. Never put credentials in the builder's
        // Debug representation or in OpenDAL service diagnostics.
        // Upstream builders log endpoint/root at debug level. This synchronous
        // scope suppresses those configuration diagnostics in the app's log bridge.
        let operator =
            tracing::subscriber::with_default(tracing::subscriber::NoSubscriber::default(), || {
                Operator::new(
                    Webdav::default()
                        .endpoint(&endpoint)
                        .root(&root)
                        .disable_create_dir(true),
                )
            })
            .map_err(|error| map_error("webdav", "configure", error))?;
        let settings = settings.clone();
        let worker = Worker::with_context(move || {
            NativeWebdavRemote::new(&settings).map(DirectoryAdapter::Webdav)
        })?;
        Self::new(operator, transport, "webdav", worker, stats)
    }

    pub fn s3(
        settings: &S3SyncSettings,
        stats: Arc<Mutex<CloudSyncIoStats>>,
    ) -> Result<Self, CloudSyncError> {
        let default_endpoint = format!(
            "https://s3.{}.amazonaws.com",
            if settings.region.trim().is_empty() {
                "us-east-1"
            } else {
                settings.region.trim()
            }
        );
        let endpoint = endpoint(
            if settings.endpoint.trim().is_empty() {
                &default_endpoint
            } else {
                &settings.endpoint
            },
            "s3",
        )?;
        let root = path(&settings.root)?;
        let invalid = || {
            storage_error(
                "s3",
                "configure",
                CloudStorageErrorKind::InvalidConfiguration,
            )
        };
        if settings.bucket.trim().is_empty() {
            return Err(invalid());
        }
        let access_key = settings
            .access_key_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(invalid)?;
        let secret_key = settings
            .secret_access_key
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(invalid)?;
        let mut builder = S3::default()
            .endpoint(&endpoint)
            .bucket(settings.bucket.trim())
            .root(&root)
            .region(if settings.region.trim().is_empty() {
                "us-east-1"
            } else {
                settings.region.trim()
            })
            .access_key_id(access_key)
            .secret_access_key(secret_key)
            // Never silently select environment/instance credentials instead
            // of the account whose identity the Store validated.
            .disable_config_load()
            .disable_ec2_metadata();
        if let Some(token) = settings
            .session_token
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            builder = builder.session_token(token);
        }
        if settings.virtual_host_style {
            builder = builder.enable_virtual_host_style();
        }
        let native_settings = settings.clone();
        let native_stats = stats.clone();
        let worker = Worker::with_context(move || {
            NativeS3Remote::new_with_stats(&native_settings, native_stats).map(DirectoryAdapter::S3)
        })?;
        let transport = Transport::new(stats.clone(), None)
            .map_err(|error| map_error("s3", "configure", error))?;
        let operator =
            tracing::subscriber::with_default(tracing::subscriber::NoSubscriber::default(), || {
                Operator::new(builder)
            })
            .map_err(|error| map_error("s3", "configure", error))?;
        Self::new(operator, transport, "s3", worker, stats)
    }

    fn new(
        operator: Operator,
        transport: Transport,
        provider: &'static str,
        worker: Worker<DirectoryAdapter>,
        stats: Arc<Mutex<CloudSyncIoStats>>,
    ) -> Result<Self, CloudSyncError> {
        let operator = operator
            .with_context(
                OperationContext::new()
                    .with_http_transport(HttpTransporter::new(transport.clone())),
            )
            .layer(
                TimeoutLayer::new()
                    .with_timeout(Duration::from_secs(30))
                    .with_io_timeout(Duration::from_secs(30)),
            );
        Ok(Self {
            operator,
            transport,
            stats,
            worker,
            provider,
        })
    }

    fn directory<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&DirectoryAdapter) -> Result<T, CloudSyncError> + Send + 'static,
    ) -> Result<T, CloudSyncError> {
        let stats = self.stats.clone();
        self.worker.blocking(move |directories| {
            super::metrics::with_stats(stats, || operation(directories))
        })
    }

    fn read_operator(&self) -> Operator {
        // This clone is used ONLY by reads. RetryLayer's default interceptor
        // logs raw errors (including paths), so replace it with fixed fields.
        self.operator.clone().layer(
            RetryLayer::new()
                .with_max_times(2)
                .with_jitter()
                .with_notify(|event: opendal::layers::RetryEvent<'_>| {
                    tracing::debug!(attempt = event.attempt, "cloud storage read retry");
                }),
        )
    }

    fn run<T, F>(&self, operation: &'static str, future: F) -> Result<T, CloudSyncError>
    where
        T: Send + 'static,
        F: Future<Output = Result<T, CloudSyncError>> + Send + 'static,
    {
        // Upstream TracingLayer records object paths. Fixed-field spans retain
        // diagnostics without emitting user namespaces or publication contents.
        self.worker.run(future.instrument(tracing::debug_span!(
            "cloud_storage",
            provider = self.provider,
            operation
        )))
    }
}

impl CloudSyncRemote for OpenDalRemote {
    fn provider(&self) -> &'static str {
        self.provider
    }

    fn create_dir(&self, directory: &str) -> Result<(), CloudSyncError> {
        let directory = path(directory)?;
        self.directory(move |directories| match directories {
            DirectoryAdapter::Webdav(remote) => remote.create_dir(&directory),
            DirectoryAdapter::S3(_) => Ok(()),
        })
    }

    fn read_if_exists(&self, path: &str) -> Result<Option<Vec<u8>>, CloudSyncError> {
        match self.read_conditional(path, None)? {
            CloudObjectRead::Missing => Ok(None),
            CloudObjectRead::Modified { bytes, .. } => Ok(Some(bytes)),
            CloudObjectRead::NotModified => Err(storage_error(
                self.provider,
                "read",
                CloudStorageErrorKind::Unexpected,
            )),
        }
    }

    fn read_conditional(
        &self,
        name: &str,
        etag: Option<&str>,
    ) -> Result<CloudObjectRead, CloudSyncError> {
        let name = path(name)?;
        let etag = etag
            .filter(|s| !s.is_empty() && s.len() <= 1_024)
            .map(ToOwned::to_owned);
        let op = self.read_operator();
        let provider = self.provider;
        self.run("read", async move {
            let mut validator = etag;
            loop {
                let mut request = op.reader_with(&name);
                if let Some(etag) = &validator {
                    request = request.if_none_match(etag);
                }
                let result = async {
                    let reader = request.await?;
                    let bytes = reader.read(..).await?.to_vec();
                    let etag = reader
                        .metadata()
                        .and_then(|metadata| metadata.etag())
                        .filter(|s| !s.is_empty() && s.len() <= 1_024)
                        .map(ToOwned::to_owned);
                    Ok::<_, opendal::Error>(CloudObjectRead::Modified { bytes, etag })
                }
                .await;
                match result {
                    Ok(result) => return Ok(result),
                    Err(error) if error.kind() == ErrorKind::NotFound => {
                        return Ok(CloudObjectRead::Missing);
                    }
                    Err(error)
                        if validator.is_some()
                            && matches!(
                                failure(&error),
                                Some(Failure::Http { status: 304, .. })
                            ) =>
                    {
                        return Ok(CloudObjectRead::NotModified);
                    }
                    Err(error)
                        if validator.is_some()
                            && matches!(
                                failure(&error),
                                Some(Failure::Http {
                                    status: 400 | 405 | 501,
                                    ..
                                })
                            ) =>
                    {
                        validator = None;
                    }
                    Err(error) => return Err(map_error(provider, "read", error)),
                }
            }
        })
    }

    fn write(&self, name: &str, bytes: &[u8]) -> Result<(), CloudSyncError> {
        let name = path(name)?;
        if self.provider == "webdav" {
            self.create_dir(name.rsplit_once('/').map_or("", |(parent, _)| parent))?;
        }
        let op = self.operator.clone();
        let bytes = bytes.to_vec();
        let provider = self.provider;
        self.run("write", async move {
            // A single buffered write lets S3 use one atomic PUT. No automatic
            // retry layer is attached to the mutation operator.
            op.write(&name, bytes)
                .await
                .map(|_| ())
                .map_err(|error| map_error(provider, "write", error))
        })
    }

    fn write_sync_file(&self, name: &str, bytes: &[u8]) -> Result<(), CloudSyncError> {
        let name = path(name)?;
        if let Some(existing) = self.read_if_exists(&name)? {
            return if existing == bytes {
                Ok(())
            } else {
                Err(storage_error(
                    self.provider,
                    "publish",
                    CloudStorageErrorKind::Conflict,
                ))
            };
        }
        let stage = format!(
            "{}/.sync-v4-staging/{}",
            name.rsplit_once('/').map_or("", |(parent, _)| parent),
            uuid::Uuid::new_v4()
        );
        let move_urls = if self.provider == "webdav" {
            self.create_dir(stage.rsplit_once('/').map_or("", |(parent, _)| parent))?;
            let source = stage.clone();
            let destination = name.clone();
            Some(self.directory(move |directories| match directories {
                DirectoryAdapter::Webdav(remote) => {
                    Ok((remote.url_for(&source), remote.url_for(&destination)))
                }
                DirectoryAdapter::S3(_) => unreachable!("WebDAV publication adapter"),
            })?)
        } else {
            None
        };
        let transport = self.transport.clone();
        let op = self.operator.clone();
        let bytes = bytes.to_vec();
        let expected = bytes.clone();
        let target = name.clone();
        let provider = self.provider;
        let result = self.run("publish", async move {
            if provider == "s3" {
                op.write_with(&target, bytes)
                    .if_not_exists(true)
                    .await
                    .map(|_| ())
                    .map_err(|error| map_error(provider, "publish", error))
            } else {
                // OpenDAL rename first PROPFINDs the source, requiring optional
                // metadata unsupported by some NAS servers. Publish with the
                // protocol primitive on the same authenticated HTTP transport.
                op.write(&stage, bytes)
                    .await
                    .map_err(|error| map_error(provider, "stage publication", error))?;
                let (source, destination) = move_urls.expect("WebDAV MOVE URLs");
                let request = http::Request::builder()
                    .method("MOVE")
                    .uri(source)
                    .header("destination", destination)
                    .header("overwrite", "F")
                    .body(Buffer::new())
                    .map_err(|_| {
                        storage_error(
                            provider,
                            "publish",
                            CloudStorageErrorKind::InvalidConfiguration,
                        )
                    })?;
                // No replay on ambiguous failure; retain staging for recovery.
                transport
                    .fetch(request)
                    .await
                    .map(|_| ())
                    .map_err(|error| map_error(provider, "publish", error))
            }
        });
        // Resolve only an explicit create-if-absent race. Network/timeout errors
        // remain errors so existing pending-publication recovery owns ambiguity.
        if let Err(error) = result
            && (!matches!(
                error,
                CloudSyncError::Http {
                    status: 409 | 412,
                    ..
                } | CloudSyncError::Storage {
                    kind: CloudStorageErrorKind::Conflict,
                    ..
                }
            ) || self.read_if_exists(&name)?.as_deref() != Some(expected.as_slice()))
        {
            return Err(error);
        }
        if self.read_if_exists(&name)?.as_deref() != Some(expected.as_slice()) {
            return Err(storage_error(
                provider,
                "verify publication",
                CloudStorageErrorKind::Conflict,
            ));
        }
        Ok(())
    }

    fn delete(&self, name: &str) -> Result<(), CloudSyncError> {
        let name = path(name)?;
        let op = self.operator.clone();
        let provider = self.provider;
        self.run("delete", async move {
            match op.delete(&name).await {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
                Err(error) => Err(map_error(provider, "delete", error)),
            }
        })
    }

    fn list_files(&self, directory: &str) -> Result<Vec<String>, CloudSyncError> {
        Ok(self
            .list_metadata(directory)?
            .into_iter()
            .map(|entry| entry.path)
            .collect())
    }

    fn list_sync_files(&self, directory: &str) -> Result<Vec<String>, CloudSyncError> {
        let directory = path(directory)?;
        self.directory(move |directories| match directories {
            DirectoryAdapter::Webdav(remote) => remote.list_sync_files(&directory),
            DirectoryAdapter::S3(remote) => remote.list_sync_files(&directory),
        })
    }

    fn list_metadata(&self, directory: &str) -> Result<Vec<CloudObjectMetadata>, CloudSyncError> {
        // Preserve S3's exact prefix semantics, including a caller-supplied
        // trailing slash, and the native duplicate/token/capacity guards.
        path(directory)?;
        let directory = directory.to_owned();
        self.directory(move |directories| match directories {
            DirectoryAdapter::Webdav(remote) => Ok(remote
                .list_files(&directory)?
                .into_iter()
                .map(|path| CloudObjectMetadata {
                    path,
                    etag: None,
                    size: None,
                })
                .collect()),
            DirectoryAdapter::S3(remote) => remote.list_metadata(&directory),
        })
    }
}

fn path(value: &str) -> Result<String, CloudSyncError> {
    let name = value.trim().trim_matches('/');
    if name.contains(['\\', '\0'])
        || name
            .split('/')
            .any(|part| matches!(part, "." | "..") || part.is_empty() && !name.is_empty())
    {
        return Err(CloudSyncError::Remote("invalid cloud storage path".into()));
    }
    Ok(name.into())
}

fn endpoint(value: &str, provider: &'static str) -> Result<String, CloudSyncError> {
    let invalid = || {
        storage_error(
            provider,
            "configure",
            CloudStorageErrorKind::InvalidConfiguration,
        )
    };
    let endpoint = value.trim().trim_end_matches('/');
    let url = url::Url::parse(endpoint).map_err(|_| invalid())?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid());
    }
    Ok(endpoint.into())
}

#[cfg(test)]
mod tests;
