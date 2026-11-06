//! Drive service adapters. Protocol and durable pending ownership stay above this layer.
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nyaterm_core::cloud_sync::credentials::DriveProvider;
use nyaterm_core::cloud_sync::io::{CloudObjectMetadata, CloudObjectRead, CloudSyncIoStats};
use nyaterm_core::{
    AliyunDriveSyncSettings, CloudSyncError, CloudSyncRemote, OAuthDriveSyncSettings,
    google_drive_query_literal,
};
use opendal::layers::TimeoutLayer;
use opendal::services::{AliyunDrive, Gdrive, Onedrive};
use opendal::{ErrorKind, HttpTransport, HttpTransporter, OperationContext, Operator};
use serde_json::{Value, json};

use super::drive_http::{
    ALIYUN, DriveHttp, GOOGLE_FILES, GRAPH, conflict, decode, invalid, request, text,
};
use super::error::{Failure, failure, map_error, storage_error};
use super::path as validate_path;
use super::transport::Transport;
use super::worker::Worker;
use crate::CloudTokenSink;
use crate::helpers::percent_encode_path;
use crate::listing::{ListingProgress, name, next, validate_next_link};
use nyaterm_core::cloud_sync::error::CloudStorageErrorKind;

const FOLDER: &str = "application/vnd.google-apps.folder";

#[derive(Clone)]
struct Session {
    provider: DriveProvider,
    root: String,
    drive_type: String,
    transport: Transport,
}

pub struct OpenDalDriveRemote {
    session: Session,
    worker: Worker<()>,
}
impl OpenDalDriveRemote {
    pub fn google_drive(
        settings: &OAuthDriveSyncSettings,
        sink: CloudTokenSink,
        stats: Arc<Mutex<CloudSyncIoStats>>,
    ) -> Result<Self, CloudSyncError> {
        Self::new(DriveProvider::Google, settings, "default", sink, stats)
    }
    pub fn onedrive(
        settings: &OAuthDriveSyncSettings,
        sink: CloudTokenSink,
        stats: Arc<Mutex<CloudSyncIoStats>>,
    ) -> Result<Self, CloudSyncError> {
        Self::new(DriveProvider::OneDrive, settings, "default", sink, stats)
    }
    pub fn aliyun_drive(
        settings: &AliyunDriveSyncSettings,
        sink: CloudTokenSink,
        stats: Arc<Mutex<CloudSyncIoStats>>,
    ) -> Result<Self, CloudSyncError> {
        Self::new(
            DriveProvider::Aliyun,
            &OAuthDriveSyncSettings {
                root: settings.root.clone(),
                access_token: settings.access_token.clone(),
                refresh_token: settings.refresh_token.clone(),
                client_id: settings.client_id.clone(),
                client_secret: settings.client_secret.clone(),
            },
            settings.drive_type.trim(),
            sink,
            stats,
        )
    }
    fn new(
        provider: DriveProvider,
        settings: &OAuthDriveSyncSettings,
        drive_type: &str,
        sink: CloudTokenSink,
        stats: Arc<Mutex<CloudSyncIoStats>>,
    ) -> Result<Self, CloudSyncError> {
        let label = label(provider);
        if !matches!(drive_type, "" | "default" | "resource" | "backup") {
            return Err(storage_error(
                label,
                "configure",
                CloudStorageErrorKind::InvalidConfiguration,
            ));
        }
        let mut transport =
            Transport::new(stats, None).map_err(|e| map_error(label, "configure", e))?;
        transport.drive = Some(DriveHttp::new(provider, settings, sink));
        #[cfg(any(test, feature = "test-support"))]
        {
            transport.test_endpoint = crate::metrics::test_drive_endpoint();
        }
        Ok(Self {
            session: Session {
                provider,
                root: path(&settings.root)?,
                drive_type: drive_type.into(),
                transport,
            },
            worker: Worker::with_context(|| Ok(()))?,
        })
    }
    fn run<T: Send + 'static, F: Future<Output = opendal::Result<T>> + Send + 'static>(
        &self,
        operation: &'static str,
        job: impl FnOnce(Session) -> F + Send + 'static,
    ) -> Result<T, CloudSyncError> {
        let session = self.session.clone();
        let provider = self.provider();
        self.worker.run(async move {
            job(session)
                .await
                .map_err(|e| map_error(provider, operation, e))
        })
    }
}
fn path(value: &str) -> Result<String, CloudSyncError> {
    // Keep Native's segment trimming and repeated-slash normalization, then
    // enforce the shared traversal/path safety contract.
    validate_path(&nyaterm_core::drive_remote_segments("", value).join("/"))
}
fn label(provider: DriveProvider) -> &'static str {
    match provider {
        DriveProvider::Google => "google_drive",
        DriveProvider::OneDrive => "onedrive",
        DriveProvider::Aliyun => "aliyun_drive",
    }
}
#[derive(Clone)]
struct Item {
    id: String,
    name: String,
    folder: bool,
    size: Option<u64>,
    etag: Option<String>,
}
impl Session {
    fn full(&self, name: &str) -> String {
        [self.root.as_str(), name]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("/")
    }
    fn operator(&self) -> opendal::Result<Operator> {
        // Rebuild only the lightweight service/path caches, never the runtime or HTTP
        // client. A previous operation's cached Drive IDs cannot hide remote changes.
        let op = tracing::subscriber::with_default(
            tracing::subscriber::NoSubscriber::default(),
            || match self.provider {
                DriveProvider::Google => Operator::new(
                    Gdrive::default()
                        .root(&self.root)
                        .access_token("nyaterm-managed-access-token"),
                ),
                DriveProvider::OneDrive => Operator::new(
                    Onedrive::default()
                        .root(&self.root)
                        .access_token("nyaterm-managed-access-token"),
                ),
                DriveProvider::Aliyun => Operator::new(
                    AliyunDrive::default()
                        .root(&self.root)
                        .drive_type(&self.drive_type)
                        .access_token("nyaterm-managed-access-token"),
                ),
            },
        )?;
        Ok(op
            .with_context(
                OperationContext::new()
                    .with_http_transport(HttpTransporter::new(self.transport.clone())),
            )
            .layer(
                TimeoutLayer::new()
                    .with_timeout(Duration::from_secs(30))
                    .with_io_timeout(Duration::from_secs(30)),
            ))
    }
    async fn api(&self, method: &str, url: &str, body: Option<Value>) -> opendal::Result<Value> {
        let response = self.transport.fetch(request(method, url, body)?).await?;
        // HttpBody is a stream; collect it through the public body API.
        let body = response.into_body().to_buffer().await?;
        if body.is_empty() {
            return Ok(Value::Null);
        }
        decode(&body)
    }
    async fn drive_id(&self) -> opendal::Result<String> {
        let value = self
            .api(
                "POST",
                &format!("{ALIYUN}/adrive/v1.0/user/getDriveInfo"),
                Some(json!({})),
            )
            .await?;
        let key = match self.drive_type.as_str() {
            "backup" => "backup_drive_id",
            "resource" => "resource_drive_id",
            _ => "default_drive_id",
        };
        Ok(value
            .get(key)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .unwrap_or(text(&value, "default_drive_id")?)
            .into())
    }
    fn parse_item(&self, value: &Value) -> opendal::Result<Item> {
        let item_name = name(value).map_err(|_| invalid())?;
        let (id_key, folder) = match self.provider {
            DriveProvider::Google => ("id", text(value, "mimeType")? == FOLDER),
            DriveProvider::OneDrive => {
                let folder = value.get("folder").is_some_and(Value::is_object);
                if !folder && !value.get("file").is_some_and(Value::is_object) {
                    return Err(invalid());
                }
                ("id", folder)
            }
            DriveProvider::Aliyun => (
                "file_id",
                match text(value, "type")? {
                    "folder" => true,
                    "file" => false,
                    _ => return Err(invalid()),
                },
            ),
        };
        let id = text(value, id_key)?;
        // IDs are URL path components in publication primitives.
        if id.contains(['/', '\\', '?', '#', '\0']) {
            return Err(invalid());
        }
        let size = match value.get("size") {
            None => None,
            Some(Value::String(size)) => Some(size.parse::<u64>().map_err(|_| invalid())?),
            Some(value) => Some(value.as_u64().ok_or_else(invalid)?),
        };
        let etag = value
            .get("eTag")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty() && s.len() <= 1024)
            .map(str::to_owned);
        Ok(Item {
            id: id.into(),
            name: item_name,
            folder,
            size,
            etag,
        })
    }
    async fn google_children(
        &self,
        parent: &str,
        child: Option<&str>,
    ) -> opendal::Result<Vec<Item>> {
        let mut url = url::Url::parse(GOOGLE_FILES).map_err(|_| invalid())?;
        let mut query = format!(
            "{} in parents and trashed = false",
            google_drive_query_literal(parent)
        );
        if let Some(child) = child {
            query.push_str(&format!(
                " and name = {}",
                google_drive_query_literal(child)
            ));
        }
        url.query_pairs_mut().append_pair("q", &query);
        let value = self.api("GET", url.as_str(), None).await?;
        value
            .get("files")
            .and_then(Value::as_array)
            .ok_or_else(invalid)?
            .iter()
            .map(|v| self.parse_item(v))
            .collect()
    }
    async fn lookup_full(&self, full: &str) -> opendal::Result<Option<Item>> {
        let result = match self.provider {
            DriveProvider::Google => {
                let mut parent = Item {
                    id: "root".into(),
                    name: "root".into(),
                    folder: true,
                    size: None,
                    etag: None,
                };
                for segment in full.split('/').filter(|s| !s.is_empty()) {
                    if !parent.folder {
                        return Err(invalid());
                    }
                    let children = self.google_children(&parent.id, Some(segment)).await?;
                    let Some(item) = children.into_iter().find(|v| v.name == segment) else {
                        return Ok(None);
                    };
                    parent = item;
                }
                return Ok(Some(parent));
            }
            DriveProvider::OneDrive => {
                let url = if full.is_empty() {
                    format!("{GRAPH}/root")
                } else {
                    format!("{GRAPH}/root:/{}", percent_encode_path(full))
                };
                self.api("GET", &url, None).await
            }
            DriveProvider::Aliyun => {
                self.api(
                    "POST",
                    &format!("{ALIYUN}/adrive/v1.0/openFile/get_by_path"),
                    Some(json!({
                        "drive_id": self.drive_id().await?, "file_path": format!("/{full}")
                    })),
                )
                .await
            }
        };
        match result {
            Ok(value) => self.parse_item(&value).map(Some),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }
    async fn ensure_full(&self, full: &str) -> opendal::Result<Item> {
        let mut parent_path = String::new();
        let mut parent = if self.provider == DriveProvider::Aliyun {
            Item {
                id: "root".into(),
                name: "root".into(),
                folder: true,
                size: None,
                etag: None,
            }
        } else {
            self.lookup_full("").await?.ok_or_else(invalid)?
        };
        for segment in full.split('/').filter(|s| !s.is_empty()) {
            let current = if parent_path.is_empty() {
                segment.into()
            } else {
                format!("{parent_path}/{segment}")
            };
            parent = if let Some(item) = self.lookup_full(&current).await? {
                item
            } else {
                let result = match self.provider {
                    DriveProvider::Google => self.api("POST", GOOGLE_FILES, Some(json!({"name": segment, "mimeType": FOLDER, "parents": [parent.id]}))).await,
                    DriveProvider::OneDrive => self.api("POST", &format!("{GRAPH}/items/{}/children", parent.id), Some(json!({"name": segment, "folder": {}, "@microsoft.graph.conflictBehavior": "fail"}))).await,
                    DriveProvider::Aliyun => self.api("POST", &format!("{ALIYUN}/adrive/v1.0/openFile/create"), Some(json!({"drive_id": self.drive_id().await?, "parent_file_id": parent.id, "name": segment, "type": "folder", "check_name_mode": "refuse"}))).await,
                };
                if let Err(error) = result
                    && !matches!(error.kind(), ErrorKind::Conflict | ErrorKind::AlreadyExists)
                {
                    return Err(error);
                }
                // Re-resolve even successful creates. Google can race another device
                // and create duplicate folders; never continue down an arbitrary one.
                self.lookup_full(&current).await?.ok_or_else(invalid)?
            };
            if !parent.folder {
                return Err(conflict());
            }
            parent_path = current;
        }
        Ok(parent)
    }
    async fn list(&self, relative: &str) -> opendal::Result<Vec<CloudObjectMetadata>> {
        let Some(parent) = self.lookup_full(&self.full(relative)).await? else {
            return Ok(Vec::new());
        };
        if !parent.folder {
            return Err(invalid());
        }
        let mut items = Vec::new();
        if self.provider == DriveProvider::Google {
            items = self.google_children(&parent.id, None).await?;
        } else {
            let first = format!("{GRAPH}/items/{}/children", parent.id);
            let mut link = first.clone();
            let mut marker = String::new();
            let mut progress = ListingProgress::default();
            loop {
                let value = if self.provider == DriveProvider::OneDrive {
                    self.api("GET", &link, None).await?
                } else {
                    self.api("POST", &format!("{ALIYUN}/adrive/v1.0/openFile/list"), Some(json!({"drive_id": self.drive_id().await?, "parent_file_id": parent.id, "limit": 100, "marker": marker}))).await?
                };
                let array = value
                    .get(if self.provider == DriveProvider::OneDrive {
                        "value"
                    } else {
                        "items"
                    })
                    .and_then(Value::as_array)
                    .ok_or_else(invalid)?;
                let page = array
                    .iter()
                    .map(|v| self.parse_item(v))
                    .collect::<opendal::Result<Vec<_>>>()?;
                let cursor = if self.provider == DriveProvider::OneDrive {
                    next(&value, "@odata.nextLink").map_err(|_| invalid())?
                } else {
                    match value.get("next_marker") {
                        None => None,
                        Some(Value::String(v)) if v.is_empty() => None,
                        Some(Value::String(v)) => Some(v.clone()),
                        _ => return Err(invalid()),
                    }
                };
                progress
                    .observe(
                        &page.iter().map(|v| v.name.clone()).collect::<Vec<_>>(),
                        cursor.as_deref(),
                    )
                    .map_err(|_| invalid())?;
                items.extend(page);
                let Some(cursor) = cursor else {
                    break;
                };
                if self.provider == DriveProvider::OneDrive {
                    validate_next_link(&first, &cursor).map_err(|_| invalid())?;
                    link = cursor;
                } else {
                    marker = cursor;
                }
            }
        }
        Ok(items
            .into_iter()
            .filter(|v| !v.folder)
            .map(|v| CloudObjectMetadata {
                path: if relative.is_empty() {
                    v.name
                } else {
                    format!("{relative}/{}", v.name)
                },
                size: v.size,
                etag: v.etag,
            })
            .collect())
    }
    async fn read(&self, name: &str) -> opendal::Result<Option<Vec<u8>>> {
        let Some(item) = self.lookup_full(&self.full(name)).await? else {
            return Ok(None);
        };
        if item.folder {
            return Err(invalid());
        }
        self.operator()?.read(name).await.map(|v| Some(v.to_vec()))
    }
    async fn publish(&self, name: &str, bytes: Vec<u8>) -> opendal::Result<()> {
        if let Some(existing) = self.read(name).await? {
            return if existing == bytes {
                Ok(())
            } else {
                Err(conflict())
            };
        }
        let (parent, basename) = name.rsplit_once('/').unwrap_or(("", name));
        let destination = self.ensure_full(&self.full(parent)).await?;
        let stage = format!(
            "{}/.sync-v4-staging/{}/{basename}",
            parent,
            uuid::Uuid::new_v4()
        )
        .trim_start_matches('/')
        .to_owned();
        let stage_parent = stage.rsplit_once('/').ok_or_else(invalid)?.0;
        self.ensure_full(&self.full(stage_parent)).await?;
        // New random staging paths avoid AliyunDrive's destructive overwrite branch.
        if self.provider == DriveProvider::Aliyun && bytes.is_empty() {
            // OpenDAL's lazy Aliyun writer does not create an object for an empty
            // buffer. Use the same service protocol/transport for this narrow case.
            let parent = self
                .lookup_full(&self.full(stage_parent))
                .await?
                .ok_or_else(invalid)?;
            let drive = self.drive_id().await?;
            let created = self
                .api(
                    "POST",
                    &format!("{ALIYUN}/adrive/v1.0/openFile/create"),
                    Some(json!({
                        "drive_id": drive, "parent_file_id": parent.id, "name": basename,
                        "type": "file", "size": 0, "check_name_mode": "refuse"
                    })),
                )
                .await?;
            if created.get("exist").and_then(Value::as_bool) == Some(true) {
                return Err(conflict());
            }
            let id = text(&created, "file_id")?;
            if let Some(upload) = created
                .get("upload_id")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
            {
                let urls = self.api("POST", &format!("{ALIYUN}/adrive/v1.0/openFile/getUploadUrl"), Some(json!({
                    "drive_id": drive, "file_id": id, "upload_id": upload, "part_info_list": [{"part_number": 1}]
                }))).await?;
                let first = urls
                    .get("part_info_list")
                    .and_then(Value::as_array)
                    .and_then(|v| v.first())
                    .ok_or_else(invalid)?;
                let upload_request = http::Request::put(text(first, "upload_url")?)
                    .body(opendal::Buffer::new())
                    .map_err(|_| invalid())?;
                self.transport.fetch(upload_request).await?;
                self.api(
                    "POST",
                    &format!("{ALIYUN}/adrive/v1.0/openFile/complete"),
                    Some(json!({"drive_id":drive, "file_id":id, "upload_id":upload})),
                )
                .await?;
            }
        } else {
            self.operator()?.write(&stage, bytes.clone()).await?;
        }
        if self.read(&stage).await?.as_deref() != Some(bytes.as_slice()) {
            return Err(invalid());
        }
        let source = self
            .lookup_full(&self.full(&stage))
            .await?
            .ok_or_else(invalid)?;
        // Recheck destination after upload; no provider is allowed to replace it.
        if let Some(existing) = self.read(name).await? {
            return if existing == bytes {
                Ok(())
            } else {
                Err(conflict())
            };
        }
        let source_parent = self
            .lookup_full(&self.full(stage_parent))
            .await?
            .ok_or_else(invalid)?;
        let result = match self.provider {
            DriveProvider::Google => {
                let mut url = url::Url::parse(&format!("{GOOGLE_FILES}/{}", source.id)).map_err(|_| invalid())?;
                url.query_pairs_mut().append_pair("addParents", &destination.id).append_pair("removeParents", &source_parent.id);
                self.api("PATCH", url.as_str(), Some(json!({}))).await
            },
            DriveProvider::OneDrive => self.api("PATCH", &format!("{GRAPH}/items/{}?@microsoft.graph.conflictBehavior=fail", source.id), Some(json!({"name": basename, "parentReference": {"id": destination.id}, "@microsoft.graph.conflictBehavior": "fail"}))).await,
            DriveProvider::Aliyun => self.api("POST", &format!("{ALIYUN}/adrive/v1.0/openFile/move"), Some(json!({"drive_id": self.drive_id().await?, "file_id": source.id, "to_parent_file_id": destination.id, "check_name_mode": "refuse"}))).await,
        };
        // A definite conflict may be an identical competing publication. An
        // ambiguous network failure belongs to durable pending recovery instead.
        if let Err(error) = result
            && (!matches!(error.kind(), ErrorKind::Conflict | ErrorKind::AlreadyExists)
                || self.read(name).await?.as_deref() != Some(bytes.as_slice()))
        {
            return Err(error);
        }
        if self.read(name).await?.as_deref() != Some(bytes.as_slice()) {
            return Err(invalid());
        }
        Ok(())
    }
}
impl CloudSyncRemote for OpenDalDriveRemote {
    fn provider(&self) -> &'static str {
        label(self.session.provider)
    }
    fn create_dir(&self, name: &str) -> Result<(), CloudSyncError> {
        let name = path(name)?;
        self.run("create_dir", |s| async move {
            s.ensure_full(&s.full(&name)).await.map(|_| ())
        })
    }
    fn read_if_exists(&self, name: &str) -> Result<Option<Vec<u8>>, CloudSyncError> {
        let name = path(name)?;
        self.run("read", |s| async move { s.read(&name).await })
    }
    fn read_conditional(
        &self,
        name: &str,
        etag: Option<&str>,
    ) -> Result<CloudObjectRead, CloudSyncError> {
        let name = path(name)?;
        let etag = etag
            .filter(|v| !v.is_empty() && v.len() <= 1024)
            .map(str::to_owned);
        self.run("read_conditional", |s| async move {
            let Some(item) = s.lookup_full(&s.full(&name)).await? else {
                return Ok(CloudObjectRead::Missing);
            };
            if item.folder {
                return Err(invalid());
            }
            let op = s.operator()?;
            let mut read = op.reader_with(&name);
            if s.provider == DriveProvider::OneDrive
                && let Some(etag) = etag
            {
                read = read.if_none_match(&etag);
            }
            let result = async {
                let reader = read.await?;
                let bytes = reader.read(..).await?.to_vec();
                // Bind validators to the content response, never an earlier stat.
                let etag = reader
                    .metadata()
                    .and_then(|m| m.etag())
                    .filter(|s| !s.is_empty() && s.len() <= 1024)
                    .map(str::to_owned);
                Ok(CloudObjectRead::Modified { bytes, etag })
            }
            .await;
            match result {
                Err(error)
                    if matches!(failure(&error), Some(Failure::Http { status: 304, .. })) =>
                {
                    Ok(CloudObjectRead::NotModified)
                }
                other => other,
            }
        })
    }
    fn write(&self, name: &str, bytes: &[u8]) -> Result<(), CloudSyncError> {
        let name = path(name)?;
        let bytes = bytes.to_vec();
        self.run("write", |s| async move {
            if s.provider == DriveProvider::Aliyun {
                return s.publish(&name, bytes).await;
            }
            let parent = name.rsplit_once('/').map_or("", |v| v.0);
            s.ensure_full(&s.full(parent)).await?;
            // Strict lookup also protects Google ordinary overwrites from ambiguity.
            s.lookup_full(&s.full(&name)).await?;
            s.operator()?.write(&name, bytes.clone()).await?;
            if s.read(&name).await?.as_deref() != Some(bytes.as_slice()) {
                return Err(invalid());
            }
            Ok(())
        })
    }
    fn delete(&self, name: &str) -> Result<(), CloudSyncError> {
        let name = path(name)?;
        self.run("delete", |s| async move {
            let Some(item) = s.lookup_full(&s.full(&name)).await? else {
                return Ok(());
            };
            // Never let a file deletion become OpenDAL's recursive directory delete.
            if item.folder {
                return Err(conflict());
            }
            s.operator()?.delete(&name).await
        })
    }
    fn list_files(&self, name: &str) -> Result<Vec<String>, CloudSyncError> {
        Ok(self
            .list_metadata(name)?
            .into_iter()
            .map(|m| m.path)
            .collect())
    }
    fn list_metadata(&self, name: &str) -> Result<Vec<CloudObjectMetadata>, CloudSyncError> {
        let name = path(name)?;
        self.run("list", |s| async move { s.list(&name).await })
    }
    fn list_sync_files(&self, name: &str) -> Result<Vec<String>, CloudSyncError> {
        self.list_files(name)
    }
    fn write_sync_file(&self, name: &str, bytes: &[u8]) -> Result<(), CloudSyncError> {
        let name = path(name)?;
        let bytes = bytes.to_vec();
        self.run("publish", |s| async move { s.publish(&name, bytes).await })
    }
}

#[cfg(test)]
pub(super) mod tests;
