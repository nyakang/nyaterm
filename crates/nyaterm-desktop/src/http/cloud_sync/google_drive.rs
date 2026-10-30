use super::metrics::{CountedRequest, CountedResponse};
use nyaterm_core::cloud_sync::credentials::{CloudCredentialUpdate, DriveProvider};
use std::sync::Mutex;
use std::time::Duration;

use nyaterm_core::{
    CloudSyncError, CloudSyncRemote, OAuthDriveSyncSettings, drive_remote_segments,
    google_drive_query_literal,
};
use serde_json::json;
use zed_reqwest::StatusCode;
use zed_reqwest::blocking::RequestBuilder;
use zed_reqwest::header::{AUTHORIZATION, CONTENT_TYPE};

use super::helpers::{
    form_urlencoded, request_nonce, trim_optional, trim_optional_secret, trim_remote_path,
};

const GOOGLE_DRIVE_FILES_URL: &str = "https://www.googleapis.com/drive/v3/files";
const GOOGLE_DRIVE_UPLOAD_FILES_URL: &str = "https://www.googleapis.com/upload/drive/v3/files";
const GOOGLE_OAUTH_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const GOOGLE_DRIVE_FOLDER_MIME: &str = "application/vnd.google-apps.folder";

pub struct NativeGoogleDriveRemote {
    pub(super) token_sink: super::CloudTokenSink,
    client: zed_reqwest::blocking::Client,
    root: String,
    access_token: Mutex<nyaterm_core::SecretString>,
    refresh_token: Mutex<nyaterm_core::SecretString>,
    client_id: Option<String>,
    client_secret: Option<nyaterm_core::SecretString>,
}

impl NativeGoogleDriveRemote {
    pub fn new(settings: &OAuthDriveSyncSettings) -> Result<Self, CloudSyncError> {
        Self::new_with_token_sink(
            settings,
            std::sync::Arc::new(|_| Err(CloudSyncError::CredentialPersistence)),
        )
    }

    pub(crate) fn new_with_token_sink(
        settings: &OAuthDriveSyncSettings,
        token_sink: super::CloudTokenSink,
    ) -> Result<Self, CloudSyncError> {
        let client = zed_reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent("NyaTerm")
            .build()
            .map_err(map_google_drive_http_error)?;
        let remote = Self {
            token_sink,
            client,
            root: trim_remote_path(&settings.root),
            access_token: Mutex::new(trim_optional_secret(settings.access_token.as_deref()).into()),
            refresh_token: Mutex::new(
                trim_optional_secret(settings.refresh_token.as_deref()).into(),
            ),
            client_id: trim_optional(settings.client_id.as_deref()),
            client_secret: trim_optional(settings.client_secret.as_deref()).map(Into::into),
        };

        if remote.bearer_token().is_empty() {
            if remote.can_refresh_access_token() {
                remote.refresh_access_token()?;
            } else {
                return Err(CloudSyncError::Remote(
                    "Google Drive access token is required".to_string(),
                ));
            }
        }

        Ok(remote)
    }

    fn bearer_token(&self) -> String {
        self.access_token
            .lock()
            .expect("google drive access token lock")
            .expose_secret()
            .to_owned()
    }

    fn can_refresh_access_token(&self) -> bool {
        !self
            .refresh_token
            .lock()
            .expect("refresh token lock")
            .is_empty()
            && self
                .client_id
                .as_deref()
                .is_some_and(|value| !value.is_empty())
            && self
                .client_secret
                .as_deref()
                .is_some_and(|value| !value.is_empty())
    }

    fn refresh_access_token(&self) -> Result<(), CloudSyncError> {
        let expected_access_token =
            Some(self.access_token.lock().expect("access token lock").clone());
        let refresh_token = self
            .refresh_token
            .lock()
            .expect("refresh token lock")
            .clone();
        if refresh_token.is_empty() {
            return Err(CloudSyncError::Remote("refresh token is required".into()));
        }
        let client_id = self
            .client_id
            .as_deref()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                CloudSyncError::Remote("Google Drive client ID is required".to_string())
            })?;
        let client_secret = self
            .client_secret
            .as_deref()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                CloudSyncError::Remote("Google Drive client secret is required".to_string())
            })?;
        let body = form_urlencoded(&[
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("refresh_token", refresh_token.expose_secret()),
            ("grant_type", "refresh_token"),
        ]);
        let response = self
            .client
            .post(GOOGLE_OAUTH_TOKEN_URL)
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(body)
            .send_counted()
            .map_err(map_google_drive_http_error)?;
        let status = response.status();
        let _retry_after_seconds = super::helpers::retry_after_seconds(response.headers());
        let body = response.text_counted()?;
        if !status.is_success() {
            return Err(super::helpers::oauth_failure(
                "Google Drive",
                status.as_u16(),
                &body,
                _retry_after_seconds,
            ));
        }
        let value: serde_json::Value = super::helpers::safe_json(&body, "Google Drive")?;
        let access_token = value
            .get("access_token")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                CloudSyncError::Remote(
                    "Google Drive token refresh response did not include access_token".to_string(),
                )
            })?;
        let rotated = value
            .get("refresh_token")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(nyaterm_core::SecretString::from);
        let access_token: nyaterm_core::SecretString = access_token.into();
        super::helpers::install_refreshed_tokens(
            &self.access_token,
            &self.refresh_token,
            &self.token_sink,
            CloudCredentialUpdate {
                provider: DriveProvider::Google,
                client_id: self.client_id.clone(),
                client_secret: self.client_secret.clone(),
                expected_access_token,
                expected_refresh_token: Some(refresh_token),
                access_token: access_token.clone(),
                refresh_token: rotated.clone(),
            },
        )?;
        Ok(())
    }

    fn send_authorized(
        &self,
        build: impl Fn(&zed_reqwest::blocking::Client, &str) -> RequestBuilder,
    ) -> Result<zed_reqwest::blocking::Response, CloudSyncError> {
        if self.bearer_token().is_empty() && self.can_refresh_access_token() {
            self.refresh_access_token()?;
        }
        let response = build(&self.client, &self.bearer_token())
            .send_counted()
            .map_err(map_google_drive_http_error)?;
        if response.status() != StatusCode::UNAUTHORIZED || !self.can_refresh_access_token() {
            return Ok(response);
        }
        let _ = response.bytes_counted();
        self.refresh_access_token()?;
        build(&self.client, &self.bearer_token())
            .send_counted()
            .map_err(map_google_drive_http_error)
    }

    fn create_folder(&self, parent_id: &str, name: &str) -> Result<String, CloudSyncError> {
        let metadata = json!({
            "name": name,
            "mimeType": GOOGLE_DRIVE_FOLDER_MIME,
            "parents": [parent_id],
        });
        let response = self.send_authorized(|client, token| {
            client
                .post(GOOGLE_DRIVE_FILES_URL)
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .json(&metadata)
                .query(&[("fields", "id")])
        })?;
        let status = response.status();
        let _retry_after_seconds = super::helpers::retry_after_seconds(response.headers());
        let body = response.text_counted()?;
        if !status.is_success() {
            return Err(super::helpers::http_failure(
                "Google Drive",
                "request",
                status.as_u16(),
                _retry_after_seconds,
            ));
        }
        google_drive_json_id(&body, "created folder")
    }

    fn ensure_folder_segments(&self, segments: &[String]) -> Result<String, CloudSyncError> {
        let mut parent_id = "root".to_string();
        for segment in segments {
            if segment.is_empty() {
                continue;
            }
            parent_id = if let Some(folder) = self.find_child(&parent_id, segment, true)? {
                folder.id
            } else {
                self.create_folder(&parent_id, segment)?
            };
        }
        Ok(parent_id)
    }

    fn locate_file(&self, path: &str) -> Result<Option<GoogleDriveFile>, CloudSyncError> {
        let segments = drive_remote_segments(&self.root, path);
        let Some((file_name, parent_segments)) = segments.split_last() else {
            return Ok(None);
        };
        let parent_id = match self.locate_folder_segments(parent_segments)? {
            Some(parent_id) => parent_id,
            None => return Ok(None),
        };
        self.find_child(&parent_id, file_name, false)
    }

    fn locate_folder_segments(
        &self,
        segments: &[String],
    ) -> Result<Option<String>, CloudSyncError> {
        let mut parent_id = "root".to_string();
        for segment in segments {
            let Some(folder) = self.find_child(&parent_id, segment, true)? else {
                return Ok(None);
            };
            parent_id = folder.id;
        }
        Ok(Some(parent_id))
    }

    fn find_child(
        &self,
        parent_id: &str,
        name: &str,
        folder_only: bool,
    ) -> Result<Option<GoogleDriveFile>, CloudSyncError> {
        let mut query = format!(
            "name = {} and {} in parents and trashed = false",
            google_drive_query_literal(name),
            google_drive_query_literal(parent_id)
        );
        if folder_only {
            query.push_str(&format!(
                " and mimeType = {}",
                google_drive_query_literal(GOOGLE_DRIVE_FOLDER_MIME)
            ));
        }
        let response = self.send_authorized(|client, token| {
            client
                .get(GOOGLE_DRIVE_FILES_URL)
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .query(&[
                    ("q", query.as_str()),
                    ("pageSize", "10"),
                    ("fields", "files(id,name,mimeType)"),
                ])
        })?;
        let status = response.status();
        let _retry_after_seconds = super::helpers::retry_after_seconds(response.headers());
        let body = response.text_counted()?;
        if !status.is_success() {
            return Err(super::helpers::http_failure(
                "Google Drive",
                "request",
                status.as_u16(),
                _retry_after_seconds,
            ));
        }
        unique_google_drive_child(&body, folder_only)
    }

    fn read_file_content(&self, file_id: &str) -> Result<Option<Vec<u8>>, CloudSyncError> {
        let url = format!("{GOOGLE_DRIVE_FILES_URL}/{file_id}");
        let response = self.send_authorized(|client, token| {
            client
                .get(&url)
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .query(&[("alt", "media")])
        })?;
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
            "Google Drive",
            "request",
            status.as_u16(),
            _retry_after_seconds,
        ))
    }

    fn create_file(&self, parent_id: &str, name: &str, bytes: &[u8]) -> Result<(), CloudSyncError> {
        let (boundary, body) = google_drive_multipart_body(parent_id, name, bytes)?;
        let content_type = format!("multipart/related; boundary={boundary}");
        let response = self.send_authorized(|client, token| {
            client
                .post(GOOGLE_DRIVE_UPLOAD_FILES_URL)
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .header(CONTENT_TYPE, content_type.clone())
                .query(&[("uploadType", "multipart"), ("fields", "id")])
                .body(body.clone())
        })?;
        google_drive_expect_success(response, "Google Drive file create")
    }

    fn update_file_content(&self, file_id: &str, bytes: &[u8]) -> Result<(), CloudSyncError> {
        let url = format!("{GOOGLE_DRIVE_UPLOAD_FILES_URL}/{file_id}");
        let response = self.send_authorized(|client, token| {
            client
                .patch(&url)
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .header(CONTENT_TYPE, "application/octet-stream")
                .query(&[("uploadType", "media")])
                .body(bytes.to_vec())
        })?;
        google_drive_expect_success(response, "Google Drive file update")
    }

    fn delete_file(&self, file_id: &str) -> Result<(), CloudSyncError> {
        let url = format!("{GOOGLE_DRIVE_FILES_URL}/{file_id}");
        let response = self.send_authorized(|client, token| {
            client
                .delete(&url)
                .header(AUTHORIZATION, format!("Bearer {token}"))
        })?;
        if response.status().is_success() || response.status() == StatusCode::NOT_FOUND {
            let _ = response.bytes_counted();
            return Ok(());
        }
        let status = response.status();
        let _retry_after_seconds = super::helpers::retry_after_seconds(response.headers());
        let _body = response.text_counted().unwrap_or_default();
        Err(super::helpers::http_failure(
            "Google Drive",
            "request",
            status.as_u16(),
            _retry_after_seconds,
        ))
    }

    fn list_child_files(&self, parent_id: &str) -> Result<Vec<String>, CloudSyncError> {
        let query = format!(
            "{} in parents and trashed = false",
            google_drive_query_literal(parent_id)
        );
        let mut page_token: Option<String> = None;
        let mut progress = super::listing::ListingProgress::default();
        let mut names = Vec::new();
        loop {
            let response = self.send_authorized(|client, token| {
                let mut request = client
                    .get(GOOGLE_DRIVE_FILES_URL)
                    .header(AUTHORIZATION, format!("Bearer {token}"))
                    .query(&[
                        ("q", query.as_str()),
                        ("pageSize", "1000"),
                        (
                            "fields",
                            "nextPageToken,incompleteSearch,files(id,name,mimeType)",
                        ),
                    ]);
                if let Some(token) = page_token.as_deref() {
                    request = request.query(&[("pageToken", token)]);
                }
                request
            })?;
            let status = response.status();
            let _retry_after_seconds = super::helpers::retry_after_seconds(response.headers());
            let body = response.text_counted()?;
            if !status.is_success() {
                return Err(super::helpers::http_failure(
                    "Google Drive",
                    "request",
                    status.as_u16(),
                    _retry_after_seconds,
                ));
            }
            let page = parse_google_drive_list_page(&body)?;
            progress.observe(&page.names, page.next_page_token.as_deref())?;
            names.extend(page.names);
            page_token = page.next_page_token;
            if page_token.is_none() {
                break;
            }
        }
        Ok(names)
    }
}

fn unique_google_drive_child(
    body: &str,
    folder_only: bool,
) -> Result<Option<GoogleDriveFile>, CloudSyncError> {
    let value: serde_json::Value = super::helpers::safe_json(body, "Google Drive")?;
    let files = value
        .get("files")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| {
            CloudSyncError::Remote("Google Drive file lookup response is missing files".to_string())
        })?;
    let mut found = None;
    for file in files {
        let id = file
            .get("id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .trim();
        let mime_type = file
            .get("mimeType")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .trim();
        if id.is_empty() {
            continue;
        }
        if folder_only || mime_type != GOOGLE_DRIVE_FOLDER_MIME {
            if found.is_some() {
                return Err(CloudSyncError::Remote("Google Drive has duplicate names in the sync path; resolve the duplicate files or folders before syncing".into()));
            }
            found = Some(GoogleDriveFile { id: id.to_string() });
        }
    }
    Ok(found)
}

pub(super) struct GoogleDriveListPage {
    pub(super) names: Vec<String>,
    pub(super) next_page_token: Option<String>,
}

pub(super) fn parse_google_drive_list_page(
    body: &str,
) -> Result<GoogleDriveListPage, CloudSyncError> {
    let value: serde_json::Value = super::helpers::safe_json(body, "Google Drive")?;
    if value
        .get("incompleteSearch")
        .is_some_and(|value| value != &serde_json::Value::Bool(false))
    {
        return Err(super::listing::invalid());
    }
    let entries = value
        .get("files")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(super::listing::invalid)?;
    let mut names = Vec::new();
    for file in entries {
        let name = super::listing::name(file)?;
        let mime = file
            .get("mimeType")
            .and_then(serde_json::Value::as_str)
            .filter(|mime| !mime.is_empty())
            .ok_or_else(super::listing::invalid)?;
        if mime != GOOGLE_DRIVE_FOLDER_MIME {
            names.push(name);
        }
    }
    let next_page_token = super::listing::next(&value, "nextPageToken")?;
    Ok(GoogleDriveListPage {
        names,
        next_page_token,
    })
}

impl CloudSyncRemote for NativeGoogleDriveRemote {
    fn provider(&self) -> &'static str {
        "google_drive"
    }

    fn create_dir(&self, path: &str) -> Result<(), CloudSyncError> {
        let segments = drive_remote_segments(&self.root, path);
        self.ensure_folder_segments(&segments)?;
        Ok(())
    }

    fn read_if_exists(&self, path: &str) -> Result<Option<Vec<u8>>, CloudSyncError> {
        let Some(file) = self.locate_file(path)? else {
            return Ok(None);
        };
        self.read_file_content(&file.id)
    }

    fn write(&self, path: &str, bytes: &[u8]) -> Result<(), CloudSyncError> {
        let segments = drive_remote_segments(&self.root, path);
        let Some((file_name, parent_segments)) = segments.split_last() else {
            return Err(CloudSyncError::InvalidRemotePath {
                path: path.to_string(),
            });
        };
        let parent_id = self.ensure_folder_segments(parent_segments)?;
        if let Some(existing) = self.find_child(&parent_id, file_name, false)? {
            self.update_file_content(&existing.id, bytes)
        } else {
            self.create_file(&parent_id, file_name, bytes)
        }
    }

    fn delete(&self, path: &str) -> Result<(), CloudSyncError> {
        let Some(file) = self.locate_file(path)? else {
            return Ok(());
        };
        self.delete_file(&file.id)
    }

    fn list_files(&self, path: &str) -> Result<Vec<String>, CloudSyncError> {
        let segments = drive_remote_segments(&self.root, path);
        let Some(parent_id) = self.locate_folder_segments(&segments)? else {
            return Ok(Vec::new());
        };
        let prefix = path.trim().trim_matches('/');
        self.list_child_files(&parent_id).map(|names| {
            names
                .into_iter()
                .map(|name| {
                    if prefix.is_empty() {
                        name
                    } else {
                        format!("{prefix}/{name}")
                    }
                })
                .collect()
        })
    }
}

#[derive(Debug, Clone)]
struct GoogleDriveFile {
    id: String,
}

fn google_drive_json_id(body: &str, label: &str) -> Result<String, CloudSyncError> {
    let value: serde_json::Value = super::helpers::safe_json(body, "Google Drive")?;
    value
        .get("id")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| {
            CloudSyncError::Remote(format!("Google Drive {label} response is missing id"))
        })
}

fn google_drive_expect_success(
    response: zed_reqwest::blocking::Response,
    operation: &str,
) -> Result<(), CloudSyncError> {
    let status = response.status();
    let _retry_after_seconds = super::helpers::retry_after_seconds(response.headers());
    if status.is_success() {
        let _ = response.bytes_counted();
        return Ok(());
    }
    let _body = response.text_counted().unwrap_or_default();
    Err(super::helpers::http_failure(
        "Google Drive",
        operation,
        status.as_u16(),
        _retry_after_seconds,
    ))
}

pub(super) fn google_drive_multipart_body(
    parent_id: &str,
    name: &str,
    bytes: &[u8],
) -> Result<(String, Vec<u8>), CloudSyncError> {
    let boundary = format!("nyaterm-{}", request_nonce());
    let metadata = serde_json::to_vec(&json!({
        "name": name,
        "parents": [parent_id],
    }))?;
    let mut body = Vec::new();
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(b"Content-Type: application/json; charset=UTF-8\r\n\r\n");
    body.extend_from_slice(&metadata);
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(b"Content-Type: application/octet-stream\r\n\r\n");
    body.extend_from_slice(bytes);
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    Ok((boundary, body))
}

fn map_google_drive_http_error(error: zed_reqwest::Error) -> CloudSyncError {
    super::helpers::safe_http_error("Google Drive", error)
}

#[cfg(test)]
mod lookup_tests {
    use super::unique_google_drive_child;

    #[test]
    fn duplicate_files_or_folders_fail_instead_of_hiding_a_device_branch() {
        let files = r#"{"files":[{"id":"one","mimeType":"application/octet-stream"},{"id":"two","mimeType":"application/octet-stream"}]}"#;
        let folders = r#"{"files":[{"id":"one","mimeType":"application/vnd.google-apps.folder"},{"id":"two","mimeType":"application/vnd.google-apps.folder"}]}"#;
        assert!(unique_google_drive_child(files, false).is_err());
        assert!(unique_google_drive_child(folders, true).is_err());
        assert_eq!(
            unique_google_drive_child(
                r#"{"files":[{"id":"one","mimeType":"application/octet-stream"}]}"#,
                false
            )
            .unwrap()
            .unwrap()
            .id,
            "one"
        );
        assert!(
            unique_google_drive_child(r#"{"files":[]}"#, true)
                .unwrap()
                .is_none()
        );
    }
}
