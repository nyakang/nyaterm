use super::metrics::{CountedRequest, CountedResponse};
use std::time::Duration;

use nyaterm_core::{CloudSyncError, CloudSyncRemote, WebdavSyncSettings};
use quick_xml::Reader;
use quick_xml::events::Event;
use zed_reqwest::header::AUTHORIZATION;
use zed_reqwest::{Method, StatusCode};

use super::helpers::{
    build_digest_authorization, digest_challenge, map_webdav_http_error, normalize_endpoint,
    path_and_query, trim_remote_path, webdav_cnonce,
};

#[derive(Clone)]
pub struct NativeWebdavRemote {
    client: zed_reqwest::blocking::Client,
    endpoint: String,
    root: String,
    username: String,
    password: Option<nyaterm_core::SecretString>,
}

impl NativeWebdavRemote {
    pub fn new(settings: &WebdavSyncSettings) -> Result<Self, CloudSyncError> {
        let endpoint = normalize_endpoint(&settings.endpoint);
        if endpoint.is_empty() {
            return Err(CloudSyncError::Remote(
                "WebDAV endpoint is required".to_string(),
            ));
        }
        let client = zed_reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent("NyaTerm")
            .build()
            .map_err(map_webdav_http_error)?;
        Ok(Self {
            client,
            endpoint,
            root: trim_remote_path(&settings.root),
            username: settings.username.trim().to_string(),
            password: settings.password.clone(),
        })
    }

    pub(super) fn url_for(&self, path: &str) -> String {
        let mut parts = Vec::new();
        if !self.root.is_empty() {
            parts.push(self.root.as_str());
        }
        let path = trim_remote_path(path);
        if !path.is_empty() {
            parts.push(path.as_str());
        }
        if parts.is_empty() {
            self.endpoint.clone()
        } else {
            format!("{}/{}", self.endpoint, parts.join("/"))
        }
    }

    fn send(
        &self,
        method: Method,
        url: &str,
        body: Option<Vec<u8>>,
    ) -> Result<zed_reqwest::blocking::Response, CloudSyncError> {
        self.send_with_headers(method, url, body, &[])
    }

    fn send_with_headers(
        &self,
        method: Method,
        url: &str,
        body: Option<Vec<u8>>,
        headers: &[(&str, &str)],
    ) -> Result<zed_reqwest::blocking::Response, CloudSyncError> {
        let response = self.send_once(method.clone(), url, body.clone(), None, headers)?;
        if response.status() != StatusCode::UNAUTHORIZED {
            return Ok(response);
        }
        let Some(challenge) = digest_challenge(&response) else {
            return Ok(response);
        };
        if self.username.is_empty() || self.password.as_deref().unwrap_or_default().is_empty() {
            return Ok(response);
        }
        let auth = build_digest_authorization(
            &challenge,
            &self.username,
            self.password.as_deref().unwrap_or_default(),
            method.as_str(),
            path_and_query(url),
            &webdav_cnonce(),
            "00000001",
        )?;
        let _ = response.bytes_counted();
        self.send_once(method, url, body, Some(auth), headers)
    }

    fn send_once(
        &self,
        method: Method,
        url: &str,
        body: Option<Vec<u8>>,
        authorization: Option<String>,
        headers: &[(&str, &str)],
    ) -> Result<zed_reqwest::blocking::Response, CloudSyncError> {
        let mut request = self.client.request(method, url);
        if let Some(authorization) = authorization {
            request = request.header(AUTHORIZATION, authorization);
        } else if !self.username.is_empty() {
            request = request.basic_auth(&self.username, self.password.as_deref());
        }
        if let Some(body) = body {
            request = request.body(body);
        }
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        request.send_counted().map_err(map_webdav_http_error)
    }
}

impl CloudSyncRemote for NativeWebdavRemote {
    fn provider(&self) -> &'static str {
        "webdav"
    }

    fn create_dir(&self, path: &str) -> Result<(), CloudSyncError> {
        let mut current = String::new();
        // The configured provider root may itself be new. MKCOL is not
        // recursive, so create it before the protocol's relative directories.
        let relative = [self.root.as_str(), &trim_remote_path(path)].join("/");
        for segment in relative.split('/').filter(|value| !value.is_empty()) {
            if !current.is_empty() {
                current.push('/');
            }
            current.push_str(segment);
            let url = format!("{}/{}", self.endpoint, current);
            let method = Method::from_bytes(b"MKCOL").map_err(|error| {
                CloudSyncError::Remote(format!("failed to build WebDAV MKCOL method: {error}"))
            })?;
            let response = self.send(method, &url, None)?;
            let status = response.status();
            let retry = super::helpers::retry_after_seconds(response.headers());
            let _ = response.bytes_counted();
            match status {
                StatusCode::CREATED | StatusCode::OK | StatusCode::METHOD_NOT_ALLOWED => {}
                status if status.is_success() => {}
                status => {
                    return Err(super::helpers::http_failure(
                        "WebDAV",
                        "create directory",
                        status.as_u16(),
                        retry,
                    ));
                }
            }
        }
        Ok(())
    }

    fn read_if_exists(&self, path: &str) -> Result<Option<Vec<u8>>, CloudSyncError> {
        let url = self.url_for(path);
        let response = self.send(Method::GET, &url, None)?;
        let status = response.status();
        let _retry_after_seconds = super::helpers::retry_after_seconds(response.headers());
        if status == StatusCode::NOT_FOUND {
            let _ = response.bytes_counted();
            return Ok(None);
        }
        if status.is_success() {
            return response.bytes_counted().map(|bytes| Some(bytes.to_vec()));
        }
        let _body = response.text_counted().unwrap_or_default();
        Err(super::helpers::http_failure(
            "WebDAV",
            "request",
            status.as_u16(),
            _retry_after_seconds,
        ))
    }

    fn write(&self, path: &str, bytes: &[u8]) -> Result<(), CloudSyncError> {
        let parent = path
            .rsplit_once('/')
            .map(|(parent, _)| parent)
            .unwrap_or("");
        if !parent.is_empty() {
            self.create_dir(parent)?;
        }
        let url = self.url_for(path);
        let response = self.send(Method::PUT, &url, Some(bytes.to_vec()))?;
        let status = response.status();
        let _retry_after_seconds = super::helpers::retry_after_seconds(response.headers());
        if status.is_success() {
            let _ = response.bytes_counted();
            return Ok(());
        }
        let _body = response.text_counted().unwrap_or_default();
        Err(super::helpers::http_failure(
            "WebDAV",
            "request",
            status.as_u16(),
            _retry_after_seconds,
        ))
    }

    fn delete(&self, path: &str) -> Result<(), CloudSyncError> {
        let response = self.send(Method::DELETE, &self.url_for(path), None)?;
        if response.status().is_success() || response.status() == StatusCode::NOT_FOUND {
            let _ = response.bytes_counted();
            return Ok(());
        }
        let status = response.status();
        let _retry_after_seconds = super::helpers::retry_after_seconds(response.headers());
        let _body = response.text_counted().unwrap_or_default();
        Err(super::helpers::http_failure(
            "WebDAV",
            "request",
            status.as_u16(),
            _retry_after_seconds,
        ))
    }

    fn list_files(&self, path: &str) -> Result<Vec<String>, CloudSyncError> {
        self.list_names(path, false)
    }

    fn list_sync_files(&self, path: &str) -> Result<Vec<String>, CloudSyncError> {
        self.list_names(path, true)
    }
}

impl NativeWebdavRemote {
    fn list_names(&self, path: &str, v4: bool) -> Result<Vec<String>, CloudSyncError> {
        let method = Method::from_bytes(b"PROPFIND").map_err(|error| {
            CloudSyncError::Remote(format!("failed to build WebDAV PROPFIND method: {error}"))
        })?;
        let response = self.send_with_headers(
            method,
            &self.url_for(path),
            Some(b"<?xml version=\"1.0\"?><propfind xmlns=\"DAV:\"><prop><resourcetype/></prop></propfind>".to_vec()),
            &[("Depth", "1"), ("Content-Type", "application/xml")],
        )?;
        let status = response.status();
        let _retry_after_seconds = super::helpers::retry_after_seconds(response.headers());
        if status == StatusCode::NOT_FOUND {
            let _ = response.bytes_counted();
            return Ok(Vec::new());
        }
        let body = response.text_counted()?;
        if !status.is_success() && status.as_u16() != 207 {
            return Err(super::helpers::http_failure(
                "WebDAV",
                "request",
                status.as_u16(),
                _retry_after_seconds,
            ));
        }
        (if v4 {
            parse_webdav_names(&body, true)
        } else {
            parse_webdav_file_names(&body)
        })
        .map(|names| {
            let prefix = trim_remote_path(path);
            names
                .into_iter()
                .map(|name| format!("{prefix}/{name}"))
                .collect()
        })
    }
}

pub(super) fn parse_webdav_file_names(body: &str) -> Result<Vec<String>, CloudSyncError> {
    parse_webdav_names(body, false)
}

fn parse_webdav_names(body: &str, v4: bool) -> Result<Vec<String>, CloudSyncError> {
    let invalid = || CloudSyncError::Remote("invalid WebDAV PROPFIND response".into());
    let mut reader = Reader::from_str(body);
    reader.config_mut().trim_text(true);
    let mut stack: Vec<Vec<u8>> = Vec::new();
    let mut root_seen = false;
    let mut text_value = String::new();
    let mut names = Vec::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) => {
                let name = event.local_name().as_ref().to_vec();
                if stack.is_empty() {
                    if root_seen || name != b"multistatus" {
                        return Err(invalid());
                    }
                    root_seen = true;
                }
                stack.push(name);
                text_value.clear();
            }
            Ok(Event::Empty(event)) if stack.is_empty() => {
                if root_seen || event.local_name().as_ref() != b"multistatus" {
                    return Err(invalid());
                }
                root_seen = true;
            }
            Ok(Event::Text(text)) => {
                let decoded = text.decode().map_err(|_| invalid())?;
                if stack.is_empty() && !decoded.trim().is_empty() {
                    return Err(invalid());
                }
                text_value.push_str(&decoded);
            }
            Ok(Event::GeneralRef(reference)) => {
                let reference = reference.decode().map_err(|_| invalid())?;
                let reference = format!("&{reference};");
                text_value
                    .push_str(&quick_xml::escape::unescape(&reference).map_err(|_| invalid())?);
            }
            Ok(Event::End(event)) => {
                let name = event.local_name();
                if name.as_ref() == b"status" {
                    let code = text_value
                        .split_whitespace()
                        .nth(1)
                        .and_then(|code| code.parse::<u16>().ok())
                        .ok_or_else(invalid)?;
                    if !(200..300).contains(&code) {
                        // A failed child response is not an empty directory.
                        return Err(invalid());
                    }
                }
                if name.as_ref() == b"href" {
                    let href = text_value.trim_end_matches('/');
                    if let Some(name) = href.rsplit('/').next().filter(|name| {
                        if v4 {
                            name.ends_with(".bin")
                        } else {
                            name.ends_with(".redb.enc") || name.ends_with(".json")
                        }
                    }) {
                        let name = percent_decode_path_segment(name)?;
                        if name.contains(['/', '\\', '\0']) || name == "." || name == ".." {
                            return Err(invalid());
                        }
                        names.push(name);
                    }
                }
                if stack.pop().as_deref() != Some(name.as_ref()) {
                    return Err(invalid());
                }
                text_value.clear();
            }
            Ok(Event::Eof) => {
                if !root_seen || !stack.is_empty() {
                    return Err(invalid());
                }
                break;
            }
            Ok(Event::DocType(_)) => return Err(invalid()),
            Ok(_) => {}
            Err(_) => return Err(invalid()),
        }
    }
    Ok(names)
}

fn percent_decode_path_segment(value: &str) -> Result<String, CloudSyncError> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let Some(encoded) = bytes.get(index + 1..index + 3) else {
                return Err(CloudSyncError::Remote(
                    "invalid WebDAV percent encoding".to_string(),
                ));
            };
            let encoded = std::str::from_utf8(encoded).map_err(|_| {
                CloudSyncError::Remote("invalid WebDAV percent encoding".to_string())
            })?;
            decoded.push(u8::from_str_radix(encoded, 16).map_err(|_| {
                CloudSyncError::Remote("invalid WebDAV percent encoding".to_string())
            })?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded)
        .map_err(|_| CloudSyncError::Remote("WebDAV path is not valid UTF-8".to_string()))
}

#[cfg(test)]
mod tests;
