//! NyaTerm owns OAuth and the safety policy; OpenDAL never sees refresh credentials.
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use http::{Request, Response, StatusCode};
use nyaterm_core::cloud_sync::credentials::{CloudCredentialUpdate, DriveProvider};
use nyaterm_core::cloud_sync::error::CloudStorageErrorKind;
use nyaterm_core::{CloudSyncError, OAuthDriveSyncSettings, SecretString};
use opendal::Buffer;
use serde_json::{Value, json};

use super::error::Failure;
use super::transport::Transport;
use crate::CloudTokenSink;
use crate::helpers::{form_urlencoded, oauth_failure, retry_after_seconds};
use crate::listing::{ListingProgress, name};

pub(super) const GOOGLE_FILES: &str = "https://www.googleapis.com/drive/v3/files";
pub(super) const GRAPH: &str = "https://graph.microsoft.com/v1.0/me/drive";
pub(super) const ALIYUN: &str = "https://openapi.alipan.com";
pub(super) const MANAGED: &str = "Bearer nyaterm-managed-access-token";

pub(super) fn invalid() -> opendal::Error {
    Failure::Transport(CloudStorageErrorKind::Unexpected).into_opendal()
}
pub(super) fn conflict() -> opendal::Error {
    Failure::Http {
        status: 409,
        retry_after: None,
    }
    .into_opendal()
}
pub(super) fn text<'a>(value: &'a Value, key: &str) -> opendal::Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(invalid)
}
pub(super) fn decode(body: &Buffer) -> opendal::Result<Value> {
    serde_json::from_slice(&body.to_bytes()).map_err(|_| invalid())
}
pub(super) fn request(
    method: &str,
    url: &str,
    body: Option<Value>,
) -> opendal::Result<Request<Buffer>> {
    Request::builder()
        .method(method)
        .uri(url)
        .header("authorization", MANAGED)
        .header("content-type", "application/json")
        .body(Buffer::from(
            body.map(|v| serde_json::to_vec(&v))
                .transpose()
                .map_err(|_| invalid())?
                .unwrap_or_default(),
        ))
        .map_err(|_| invalid())
}

struct Tokens {
    access: SecretString,
    refresh: SecretString,
}
pub(super) struct DriveHttp {
    pub(super) provider: DriveProvider,
    tokens: Mutex<Tokens>,
    client_id: Option<String>,
    client_secret: Option<SecretString>,
    sink: CloudTokenSink,
    signed: Mutex<BTreeSet<String>>,
}
impl DriveHttp {
    pub(super) fn new(
        provider: DriveProvider,
        settings: &OAuthDriveSyncSettings,
        sink: CloudTokenSink,
    ) -> Arc<Self> {
        Arc::new(Self {
            provider,
            tokens: Mutex::new(Tokens {
                access: settings
                    .access_token
                    .as_deref()
                    .unwrap_or_default()
                    .trim()
                    .into(),
                refresh: settings
                    .refresh_token
                    .as_deref()
                    .unwrap_or_default()
                    .trim()
                    .into(),
            }),
            client_id: settings.client_id.clone().filter(|s| !s.trim().is_empty()),
            client_secret: settings
                .client_secret
                .clone()
                .filter(|s| !s.trim().is_empty()),
            sink,
            signed: Mutex::new(BTreeSet::new()),
        })
    }
    fn trusted(&self, url: &url::Url) -> bool {
        let host = match self.provider {
            DriveProvider::Google => "www.googleapis.com",
            DriveProvider::OneDrive => "graph.microsoft.com",
            DriveProvider::Aliyun => "openapi.alipan.com",
        };
        url.scheme() == "https"
            && url.host_str() == Some(host)
            && url.port_or_known_default() == Some(443)
            && url.username().is_empty()
            && url.password().is_none()
            && url.fragment().is_none()
            && match self.provider {
                DriveProvider::Google => {
                    url.path().starts_with("/drive/v3/")
                        || url.path().starts_with("/upload/drive/v3/")
                }
                DriveProvider::OneDrive => url.path().starts_with("/v1.0/"),
                DriveProvider::Aliyun => url.path().starts_with("/adrive/v1.0/"),
            }
    }
    fn remember_signed(&self, target: &str) -> opendal::Result<()> {
        let url = url::Url::parse(target).map_err(|_| invalid())?;
        if url.scheme() != "https"
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
        {
            return Err(invalid());
        }
        self.signed
            .lock()
            .expect("signed addresses")
            .insert(target.into());
        Ok(())
    }
    fn remember_response(&self, value: &Value) -> opendal::Result<()> {
        match value {
            Value::Object(fields) => {
                for (key, value) in fields {
                    if matches!(
                        key.as_str(),
                        "uploadUrl"
                            | "upload_url"
                            | "download_url"
                            | "@microsoft.graph.downloadUrl"
                    ) {
                        self.remember_signed(value.as_str().ok_or_else(invalid)?)?;
                    } else {
                        self.remember_response(value)?;
                    }
                }
            }
            Value::Array(values) => {
                for value in values {
                    self.remember_response(value)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    async fn refresh(&self, transport: &Transport) -> opendal::Result<()> {
        let (old_access, old_refresh) = {
            let state = self.tokens.lock().expect("drive tokens");
            (state.access.clone(), state.refresh.clone())
        };
        let id = self
            .client_id
            .as_deref()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                Failure::Http {
                    status: 401,
                    retry_after: None,
                }
                .into_opendal()
            })?;
        if old_refresh.is_empty()
            || (self.provider != DriveProvider::OneDrive && self.client_secret.is_none())
        {
            return Err(Failure::Http {
                status: 401,
                retry_after: None,
            }
            .into_opendal());
        }
        let (url, mime, body) = if self.provider == DriveProvider::Aliyun {
            (format!("{ALIYUN}/oauth/access_token"), "application/json", serde_json::to_vec(&json!({
                "client_id": id, "client_secret": self.client_secret.as_deref().unwrap_or_default(),
                "refresh_token": old_refresh.expose_secret(), "grant_type": "refresh_token"
            })).map_err(|_| invalid())?)
        } else {
            let mut fields = vec![
                ("client_id", id),
                ("grant_type", "refresh_token"),
                ("refresh_token", old_refresh.expose_secret()),
            ];
            if let Some(secret) = self.client_secret.as_deref() {
                fields.push(("client_secret", secret));
            }
            (
                match self.provider {
                    DriveProvider::Google => "https://oauth2.googleapis.com/token",
                    _ => "https://login.microsoftonline.com/common/oauth2/v2.0/token",
                }
                .into(),
                "application/x-www-form-urlencoded",
                form_urlencoded(&fields).into_bytes(),
            )
        };
        let req = Request::post(url)
            .header("content-type", mime)
            .body(Buffer::from(body))
            .map_err(|_| invalid())?;
        let response = transport.exchange(req).await?;
        if !response.status().is_success() {
            // Reuse Native's typed OAuth classification; never propagate its body.
            let error = oauth_failure(
                "drive",
                response.status().as_u16(),
                &String::from_utf8_lossy(&response.body().to_bytes()),
                retry_after_seconds(response.headers()),
            );
            return Err(match error {
                CloudSyncError::Authentication { status, .. } => Failure::Authentication { status },
                CloudSyncError::Http {
                    status,
                    retry_after_seconds,
                    ..
                } => Failure::Http {
                    status,
                    retry_after: retry_after_seconds,
                },
                _ => Failure::Transport(CloudStorageErrorKind::Unexpected),
            }
            .into_opendal());
        }
        let value = decode(response.body())?;
        let access: SecretString = text(&value, "access_token")?.trim().into();
        if access.is_empty() {
            return Err(invalid());
        }
        let refresh = value
            .get("refresh_token")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(SecretString::from);
        // This synchronous Store actor call runs on our joined storage worker.
        // There is deliberately no await/cancellation point between save and install.
        (self.sink)(CloudCredentialUpdate {
            provider: self.provider,
            client_id: self.client_id.clone(),
            client_secret: self.client_secret.clone(),
            expected_access_token: Some(old_access),
            expected_refresh_token: Some(old_refresh),
            access_token: access.clone(),
            refresh_token: refresh.clone(),
        })
        .map_err(|error| {
            match error {
                CloudSyncError::CredentialsChanged => Failure::CredentialsChanged,
                _ => Failure::CredentialPersistence,
            }
            .into_opendal()
        })?;
        let mut state = self.tokens.lock().expect("drive tokens");
        state.access = access;
        if let Some(refresh) = refresh {
            state.refresh = refresh;
        }
        Ok(())
    }
    async fn authorized(
        &self,
        transport: &Transport,
        request: Request<Buffer>,
    ) -> opendal::Result<Response<Buffer>> {
        let (parts, body) = request.into_parts();
        let mut current = parts.uri.to_string();
        let mut method = parts.method;
        let mut authenticated = parts.headers.contains_key("authorization");
        let initial = url::Url::parse(&current).map_err(|_| invalid())?;
        if authenticated && !self.trusted(&initial) {
            return Err(invalid());
        }
        if !authenticated
            && !self
                .signed
                .lock()
                .expect("signed addresses")
                .contains(&current)
        {
            return Err(invalid());
        }
        // Retry individual read exchanges, not the whole OpenDAL operation: an
        // outer RetryLayer could replay a refresh-token rotation after timeout.
        let read_only = method == http::Method::GET
            || (authenticated
                && self.provider == DriveProvider::Aliyun
                && matches!(
                    initial.path().rsplit('/').next(),
                    Some("getDriveInfo" | "get_by_path" | "list" | "getDownloadUrl")
                ));
        let mut read_retries = 0;
        let mut refreshed = false;
        let mut redirects = 0;
        loop {
            if authenticated && self.tokens.lock().expect("drive tokens").access.is_empty() {
                self.refresh(transport)
                    .await
                    .map_err(opendal::Error::set_persistent)?;
                refreshed = true;
            }
            let mut headers = parts.headers.clone();
            headers.remove("authorization");
            if !authenticated {
                headers.remove("proxy-authorization");
                headers.remove("cookie");
            }
            if authenticated {
                let state = self.tokens.lock().expect("drive tokens");
                headers.insert(
                    "authorization",
                    http::HeaderValue::from_str(&format!(
                        "Bearer {}",
                        state.access.expose_secret()
                    ))
                    .map_err(|_| invalid())?,
                );
            }
            let mut req = Request::builder().method(method.clone()).uri(&current);
            *req.headers_mut().expect("request headers") = headers;
            let response = match transport
                .exchange(req.body(body.clone()).map_err(|_| invalid())?)
                .await
            {
                Err(error) if read_only && error.is_temporary() && read_retries < 2 => {
                    read_retries += 1;
                    tokio::time::sleep(Duration::from_millis(100 * read_retries)).await;
                    continue;
                }
                result => result?,
            };
            if read_only
                && response.status().is_server_error()
                && read_retries < 2
                && retry_after_seconds(response.headers()).is_none()
            {
                read_retries += 1;
                tokio::time::sleep(Duration::from_millis(100 * read_retries)).await;
                continue;
            }
            if response.status() == StatusCode::UNAUTHORIZED && authenticated && !refreshed {
                self.refresh(transport)
                    .await
                    .map_err(opendal::Error::set_persistent)?;
                refreshed = true;
                continue;
            }
            if response.status().is_redirection() && response.status() != StatusCode::NOT_MODIFIED {
                // Drive redirects are only downloads. Never replay an upload at a Location.
                if method != http::Method::GET || redirects == 4 {
                    return Err(invalid());
                }
                let target = response
                    .headers()
                    .get("location")
                    .and_then(|v| v.to_str().ok())
                    .ok_or_else(invalid)?;
                let resolved = url::Url::parse(&current)
                    .map_err(|_| invalid())?
                    .join(target)
                    .map_err(|_| invalid())?;
                self.remember_signed(resolved.as_str())?;
                current = resolved.to_string();
                authenticated = false;
                method = http::Method::GET;
                redirects += 1;
                continue;
            }
            if !response.status().is_success() {
                let mut status = response.status().as_u16();
                if self.provider == DriveProvider::Aliyun
                    && status == 400
                    && let Ok(value) = decode(response.body())
                    && matches!(
                        value.get("code").and_then(Value::as_str),
                        Some("NotFound.File" | "NotFound.FileId" | "NotFound.FilePath")
                    )
                {
                    status = 404;
                }
                if status == 304 {
                    transport.record(|s| s.not_modified += 1);
                }
                return Err(Failure::Http {
                    status,
                    retry_after: retry_after_seconds(response.headers()),
                }
                .into_opendal());
            }
            let content_response = method == http::Method::GET
                && (initial.path().ends_with(":/content")
                    || initial
                        .query_pairs()
                        .any(|(k, v)| k == "alt" && v == "media"));
            if authenticated
                && !content_response
                && let Ok(value) = decode(response.body())
            {
                self.remember_response(&value)?;
                if self.provider == DriveProvider::Aliyun
                    && initial.path().ends_with("/getDownloadUrl")
                {
                    self.remember_signed(text(&value, "url")?)?;
                }
            }
            // OneDrive may finish the last upload-session fragment with HTTP 200.
            // Normalize only a complete DriveItem whose size equals the advertised total.
            if self.provider == DriveProvider::OneDrive
                && !authenticated
                && parts.headers.contains_key("content-range")
            {
                let range = parts
                    .headers
                    .get("content-range")
                    .and_then(|v| v.to_str().ok())
                    .ok_or_else(invalid)?;
                let (end, total) = range
                    .strip_prefix("bytes ")
                    .and_then(|v| v.split_once('/'))
                    .and_then(|(span, total)| {
                        Some((
                            span.split_once('-')?.1.parse::<u64>().ok()?,
                            total.parse::<u64>().ok()?,
                        ))
                    })
                    .ok_or_else(invalid)?;
                if end + 1 == total {
                    let value = decode(response.body())?;
                    if !matches!(response.status(), StatusCode::OK | StatusCode::CREATED)
                        || value.get("size").and_then(Value::as_u64) != Some(total)
                        || text(&value, "id").is_err()
                        || text(&value, "eTag").is_err()
                        || text(&value, "lastModifiedDateTime").is_err()
                    {
                        return Err(invalid());
                    }
                    let (mut parts, body) = response.into_parts();
                    parts.status = StatusCode::CREATED;
                    return Ok(Response::from_parts(parts, body));
                }
                if response.status() == StatusCode::CREATED {
                    return Err(invalid());
                }
            }
            return Ok(response);
        }
    }
    pub(super) async fn fetch(
        &self,
        transport: &Transport,
        request: Request<Buffer>,
    ) -> opendal::Result<Response<Buffer>> {
        let url = url::Url::parse(&request.uri().to_string()).map_err(|_| invalid())?;
        if self.provider != DriveProvider::Google
            || request.method() != http::Method::GET
            || url.path() != "/drive/v3/files"
        {
            return self.authorized(transport, request).await;
        }
        // GDrive's path resolver asks for pageSize=1 and silently chooses the first
        // duplicate. Return a fully traversed, validated query before it can do so.
        let mut query: Vec<(String, String)> = url
            .query_pairs()
            .filter(|(k, _)| !matches!(k.as_ref(), "fields" | "pageSize" | "pageToken"))
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        query.push((
            "fields".into(),
            "nextPageToken,incompleteSearch,files(id,name,mimeType,size,modifiedTime,parents)"
                .into(),
        ));
        query.push(("pageSize".into(), "1000".into()));
        let (parts, _) = request.into_parts();
        let mut progress = ListingProgress::default();
        let mut files = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let mut next = url.clone();
            next.set_query(None);
            next.query_pairs_mut().extend_pairs(&query);
            if let Some(cursor) = &cursor {
                next.query_pairs_mut().append_pair("pageToken", cursor);
            }
            let mut req = Request::builder().method("GET").uri(next.as_str());
            *req.headers_mut().expect("headers") = parts.headers.clone();
            let response = self
                .authorized(transport, req.body(Buffer::new()).map_err(|_| invalid())?)
                .await?;
            let value = decode(response.body())?;
            if value
                .get("incompleteSearch")
                .is_some_and(|v| v != &Value::Bool(false))
            {
                return Err(invalid());
            }
            let page = value
                .get("files")
                .and_then(Value::as_array)
                .ok_or_else(invalid)?;
            let mut names = Vec::new();
            for file in page {
                text(file, "id")?;
                text(file, "mimeType")?;
                names.push(name(file).map_err(|_| invalid())?);
            }
            let next = crate::listing::next(&value, "nextPageToken").map_err(|_| invalid())?;
            progress
                .observe(&names, next.as_deref())
                .map_err(|_| invalid())?;
            files.extend_from_slice(page);
            cursor = next;
            if cursor.is_none() {
                break;
            }
        }
        Response::builder()
            .status(200)
            .body(Buffer::from(
                serde_json::to_vec(&json!({"files": files})).map_err(|_| invalid())?,
            ))
            .map_err(|_| invalid())
    }
}
