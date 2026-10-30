use nyaterm_core::cloud_sync::io::{CloudObjectMetadata, CloudObjectRead, CloudSyncIoStats};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nyaterm_core::{
    CloudSyncError, CloudSyncRemote, S3HttpMethod, S3SyncSettings, build_s3_signed_request,
    build_s3_signed_request_with_query, remote_path, s3_payload_sha256,
};
use quick_xml::Reader;
use quick_xml::events::Event;
use zed_reqwest::{Method, StatusCode};

use super::helpers::map_s3_http_error;

#[derive(Clone)]
pub struct NativeS3Remote {
    client: zed_reqwest::blocking::Client,
    settings: S3SyncSettings,
    stats: Arc<Mutex<CloudSyncIoStats>>,
}

impl NativeS3Remote {
    pub fn new(settings: &S3SyncSettings) -> Result<Self, CloudSyncError> {
        Self::new_with_stats(settings, Arc::new(Mutex::new(CloudSyncIoStats::default())))
    }

    pub(crate) fn new_with_stats(
        settings: &S3SyncSettings,
        stats: Arc<Mutex<CloudSyncIoStats>>,
    ) -> Result<Self, CloudSyncError> {
        build_s3_signed_request(
            settings,
            S3HttpMethod::Head,
            "nyaterm/sync/latest.redb",
            &s3_payload_sha256(&[]),
            std::time::SystemTime::now(),
        )?;
        let client = zed_reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent("NyaTerm")
            .build()
            .map_err(map_s3_http_error)?;
        Ok(Self {
            client,
            settings: settings.clone(),
            stats,
        })
    }

    fn send(
        &self,
        method: S3HttpMethod,
        path: &str,
        body: Option<Vec<u8>>,
    ) -> Result<zed_reqwest::blocking::Response, CloudSyncError> {
        self.send_conditional(method, path, body, None)
    }

    fn send_conditional(
        &self,
        method: S3HttpMethod,
        path: &str,
        body: Option<Vec<u8>>,
        etag: Option<&str>,
    ) -> Result<zed_reqwest::blocking::Response, CloudSyncError> {
        let payload_hash = body
            .as_deref()
            .map(s3_payload_sha256)
            .unwrap_or_else(|| s3_payload_sha256(&[]));
        let request = build_s3_signed_request(
            &self.settings,
            method,
            path,
            &payload_hash,
            std::time::SystemTime::now(),
        )?;
        let http_method = match method {
            S3HttpMethod::Get => Method::GET,
            S3HttpMethod::Head => Method::HEAD,
            S3HttpMethod::Put => Method::PUT,
            S3HttpMethod::Delete => Method::DELETE,
        };
        let mut builder = self.client.request(http_method, &request.url);
        for (name, value) in &request.headers {
            builder = builder.header(name.as_str(), value.as_str());
        }
        if let Some(etag) = etag {
            builder = builder.header(zed_reqwest::header::IF_NONE_MATCH, etag);
        }
        {
            let mut stats = self
                .stats
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            stats.requests += 1;
            stats.uploaded_bytes += body.as_ref().map_or(0, Vec::len) as u64;
        }
        if let Some(body) = body {
            builder = builder.body(body);
        }
        builder.send().map_err(map_s3_http_error)
    }

    fn send_list(
        &self,
        prefix: &str,
        continuation: Option<&str>,
    ) -> Result<zed_reqwest::blocking::Response, CloudSyncError> {
        let mut query = BTreeMap::from([
            ("list-type".to_string(), "2".to_string()),
            ("prefix".to_string(), prefix.to_string()),
        ]);
        if let Some(token) = continuation {
            query.insert("continuation-token".to_string(), token.to_string());
        }
        let request = build_s3_signed_request_with_query(
            &self.settings,
            S3HttpMethod::Get,
            "",
            &query,
            &s3_payload_sha256(&[]),
            std::time::SystemTime::now(),
        )?;
        let mut builder = self.client.get(&request.url);
        for (name, value) in &request.headers {
            builder = builder.header(name.as_str(), value.as_str());
        }
        self.stats
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .requests += 1;
        builder.send().map_err(map_s3_http_error)
    }
    fn response_bytes(
        &self,
        mut response: zed_reqwest::blocking::Response,
    ) -> Result<Vec<u8>, CloudSyncError> {
        use std::io::Read;
        let mut bytes = Vec::new();
        let mut buffer = [0; 8_192];
        loop {
            let count = response.read(&mut buffer).map_err(|error| {
                CloudSyncError::Io(std::io::Error::new(
                    error.kind(),
                    "S3 response body could not be read",
                ))
            })?;
            if count == 0 {
                break;
            }
            self.stats
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .downloaded_bytes += count as u64;
            bytes.extend_from_slice(&buffer[..count]);
        }
        Ok(bytes)
    }
}

impl CloudSyncRemote for NativeS3Remote {
    fn list_sync_files(&self, path: &str) -> Result<Vec<String>, CloudSyncError> {
        let prefix = format!("{}/", path.trim_matches('/'));
        let mut result = BTreeSet::new();
        for entry in self.list_metadata(&prefix)? {
            let name = entry.path.strip_prefix(&prefix).ok_or_else(|| {
                CloudSyncError::Remote("S3 listing escaped its requested sync prefix".into())
            })?;
            if name.is_empty() {
                continue;
            }
            if name.contains('/') || name.contains('\\') || name.contains('\0') {
                return Err(CloudSyncError::Remote("Invalid S3 sync object path".into()));
            }
            result.insert(entry.path);
        }
        Ok(result.into_iter().collect())
    }
    fn provider(&self) -> &'static str {
        "s3"
    }

    fn create_dir(&self, _path: &str) -> Result<(), CloudSyncError> {
        Ok(())
    }

    fn read_if_exists(&self, path: &str) -> Result<Option<Vec<u8>>, CloudSyncError> {
        match self.read_conditional(path, None)? {
            CloudObjectRead::Missing => Ok(None),
            CloudObjectRead::Modified { bytes, .. } => Ok(Some(bytes)),
            CloudObjectRead::NotModified => {
                Err(CloudSyncError::Remote("S3 returned no object body".into()))
            }
        }
    }

    fn read_conditional(
        &self,
        path: &str,
        etag: Option<&str>,
    ) -> Result<CloudObjectRead, CloudSyncError> {
        let mut response = self.send_conditional(S3HttpMethod::Get, path, None, etag)?;
        if etag.is_some() && matches!(response.status().as_u16(), 400 | 405 | 501) {
            let _ = self.response_bytes(response);
            response = self.send_conditional(S3HttpMethod::Get, path, None, None)?;
        }
        let status = response.status();
        let retry = super::helpers::retry_after_seconds(response.headers());
        let etag = response
            .headers()
            .get(zed_reqwest::header::ETAG)
            .and_then(|h| h.to_str().ok())
            .filter(|s| !s.is_empty() && s.len() <= 1_024)
            .map(ToOwned::to_owned);
        let bytes = self.response_bytes(response);
        if status == StatusCode::NOT_MODIFIED {
            self.stats
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .not_modified += 1;
            return Ok(CloudObjectRead::NotModified);
        }
        if status == StatusCode::NOT_FOUND {
            return Ok(CloudObjectRead::Missing);
        }
        if status.is_success() {
            return Ok(CloudObjectRead::Modified {
                bytes: bytes?,
                etag,
            });
        }
        Err(super::helpers::http_failure(
            "S3",
            "read",
            status.as_u16(),
            retry,
        ))
    }

    fn write(&self, path: &str, bytes: &[u8]) -> Result<(), CloudSyncError> {
        let response = self.send(S3HttpMethod::Put, path, Some(bytes.to_vec()))?;
        let status = response.status();
        let retry = super::helpers::retry_after_seconds(response.headers());
        let _ = self.response_bytes(response);
        if status.is_success() {
            return Ok(());
        }
        Err(super::helpers::http_failure(
            "S3",
            "write",
            status.as_u16(),
            retry,
        ))
    }

    fn delete(&self, path: &str) -> Result<(), CloudSyncError> {
        let response = self.send(S3HttpMethod::Delete, path, None)?;
        let status = response.status();
        let retry = super::helpers::retry_after_seconds(response.headers());
        let _ = self.response_bytes(response);
        if status.is_success() || status == StatusCode::NOT_FOUND {
            return Ok(());
        }
        Err(super::helpers::http_failure(
            "S3",
            "delete",
            status.as_u16(),
            retry,
        ))
    }

    fn list_files(&self, path: &str) -> Result<Vec<String>, CloudSyncError> {
        Ok(self
            .list_metadata(path)?
            .into_iter()
            .map(|e| e.path)
            .collect())
    }

    fn list_metadata(&self, path: &str) -> Result<Vec<CloudObjectMetadata>, CloudSyncError> {
        let provider_root = self.settings.root.trim().trim_matches('/');
        let prefix = remote_path(provider_root, path);
        let mut continuation = None;
        let mut visited_tokens = BTreeSet::new();
        let mut visited_paths = BTreeSet::new();
        let mut files = Vec::new();
        loop {
            let response = self.send_list(&prefix, continuation.as_deref())?;
            let status = response.status();
            let _retry_after_seconds = super::helpers::retry_after_seconds(response.headers());
            let body = self.response_bytes(response);
            if !status.is_success() {
                return Err(super::helpers::http_failure(
                    "S3",
                    "request",
                    status.as_u16(),
                    _retry_after_seconds,
                ));
            }
            let body = String::from_utf8(body?)
                .map_err(|_| CloudSyncError::Remote("invalid S3 LIST response".into()))?;
            let page = parse_s3_list_page(&body)?;
            if page.objects.iter().any(|entry| {
                !entry.path.starts_with(&prefix) || !visited_paths.insert(entry.path.clone())
            }) {
                return Err(CloudSyncError::Remote(
                    "S3 listing contains duplicate objects or objects outside the requested prefix"
                        .into(),
                ));
            }
            files.extend(page.objects.into_iter().map(|mut entry| {
                if !provider_root.is_empty() {
                    entry.path = entry
                        .path
                        .strip_prefix(provider_root)
                        .unwrap_or(&entry.path)
                        .trim_start_matches('/')
                        .to_string();
                }
                entry
            }));
            if files.len() > 20_000 || visited_tokens.len() > 20_000 {
                return Err(CloudSyncError::Remote(
                    "S3 listing capacity exceeded".into(),
                ));
            }
            if !page.truncated {
                break;
            }
            continuation = page.next_token;
            if continuation
                .as_ref()
                .is_none_or(|token| token.is_empty() || !visited_tokens.insert(token.clone()))
            {
                return Err(CloudSyncError::Remote(
                    "S3 LIST response has a missing or repeated continuation token".to_string(),
                ));
            }
        }
        Ok(files)
    }
}

pub(super) struct S3ListPage {
    pub(super) objects: Vec<CloudObjectMetadata>,
    pub(super) truncated: bool,
    pub(super) next_token: Option<String>,
}

pub(super) fn parse_s3_list_page(body: &str) -> Result<S3ListPage, CloudSyncError> {
    let invalid = || CloudSyncError::Remote("invalid S3 LIST response".into());
    let mut reader = Reader::from_str(body);
    reader.config_mut().trim_text(true);
    let mut field = Vec::new();
    let mut stack = Vec::new();
    let mut root_seen = false;
    let mut truncated_seen = false;
    let mut objects = Vec::new();
    let mut current: Option<CloudObjectMetadata> = None;
    let mut truncated = false;
    let mut next_token = None;
    let mut text_value = String::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) => {
                field = event.local_name().as_ref().to_vec();
                if stack.is_empty() {
                    if root_seen || field != b"ListBucketResult" {
                        return Err(invalid());
                    }
                    root_seen = true;
                }
                stack.push(field.clone());
                text_value.clear();
                if field == b"Contents" {
                    current = Some(CloudObjectMetadata::default());
                }
            }
            Ok(Event::Text(text)) => text_value.push_str(&text.decode().map_err(|_| invalid())?),
            Ok(Event::GeneralRef(reference)) => {
                let name = reference.decode().map_err(|_| invalid())?;
                let decoded = quick_xml::escape::unescape(&format!("&{name};"))
                    .map_err(|_| invalid())?
                    .into_owned();
                text_value.push_str(&decoded);
            }
            Ok(Event::End(event)) => {
                match event.local_name().as_ref() {
                    b"Key" => {
                        if let Some(entry) = &mut current {
                            entry.path = text_value.clone();
                        }
                    }
                    b"ETag" => {
                        if let Some(entry) = &mut current {
                            entry.etag = Some(text_value.clone());
                        }
                    }
                    b"Size" => {
                        if let Some(entry) = &mut current {
                            entry.size = text_value.parse().ok();
                        }
                    }
                    b"Contents" => {
                        if let Some(entry) = current.take() {
                            objects.push(entry);
                        }
                    }
                    b"IsTruncated" => {
                        if truncated_seen
                            || stack.len() != 2
                            || !matches!(text_value.as_str(), "true" | "false")
                        {
                            return Err(invalid());
                        }
                        truncated_seen = true;
                        truncated = text_value == "true";
                    }
                    b"NextContinuationToken" => next_token = Some(text_value.clone()),
                    _ => {}
                }
                if stack.pop().as_deref() != Some(event.local_name().as_ref()) {
                    return Err(invalid());
                }
                field.clear();
                text_value.clear();
            }
            Ok(Event::Eof) => {
                if !root_seen
                    || !stack.is_empty()
                    || !truncated_seen
                    || objects.iter().any(|entry| entry.path.is_empty())
                {
                    return Err(invalid());
                }
                break;
            }
            Ok(_) => {}
            Err(_) => return Err(invalid()),
        }
    }
    Ok(S3ListPage {
        objects,
        truncated,
        next_token,
    })
}

#[cfg(test)]
mod tests;
