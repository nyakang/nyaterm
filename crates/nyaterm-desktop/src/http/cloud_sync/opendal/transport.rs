use std::sync::{Arc, Mutex};
use std::time::Duration;

use http::{Request, Response};
use nyaterm_core::cloud_sync::error::CloudStorageErrorKind;
use nyaterm_core::cloud_sync::io::CloudSyncIoStats;
use nyaterm_core::{SecretString, WebdavSyncSettings};
use opendal::{Buffer, Error, ErrorKind, HttpBody, HttpTransport};
use zed_reqwest::StatusCode;
use zed_reqwest::header::{AUTHORIZATION, LOCATION};

use super::error::{Failure, transport_error};
use crate::http::cloud_sync::helpers::{
    build_digest_authorization, digest_challenge_headers, path_and_query, retry_after_seconds,
    webdav_cnonce,
};
use crate::http::cloud_sync::webdav::redirect_target;

#[derive(Clone)]
pub(super) struct Transport {
    client: zed_reqwest::Client,
    webdav: Option<(String, Option<SecretString>)>,
    stats: Arc<Mutex<CloudSyncIoStats>>,
}

impl Transport {
    #[cfg(test)]
    pub(super) fn with_test_root(mut self, certificate: zed_reqwest::Certificate) -> Self {
        self.client = zed_reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .redirect_policy(zed_reqwest::redirect::Policy::none())
            .add_root_certificate(certificate)
            .build()
            .unwrap();
        self
    }
    pub(super) fn new(
        stats: Arc<Mutex<CloudSyncIoStats>>,
        webdav: Option<&WebdavSyncSettings>,
    ) -> opendal::Result<Self> {
        Self::with_timeout(stats, webdav, Duration::from_secs(30))
    }

    pub(super) fn with_timeout(
        stats: Arc<Mutex<CloudSyncIoStats>>,
        webdav: Option<&WebdavSyncSettings>,
        timeout: Duration,
    ) -> opendal::Result<Self> {
        Ok(Self {
            client: zed_reqwest::Client::builder()
                .timeout(timeout)
                .connect_timeout(timeout.min(Duration::from_secs(10)))
                .redirect_policy(zed_reqwest::redirect::Policy::none())
                .user_agent("NyaTerm")
                .build()
                .map_err(transport_error)?,
            webdav: webdav
                .map(|settings| (settings.username.trim().into(), settings.password.clone())),
            stats,
        })
    }

    fn record(&self, update: impl FnOnce(&mut CloudSyncIoStats)) {
        update(
            &mut self
                .stats
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
    }

    async fn body(&self, mut response: zed_reqwest::Response) -> opendal::Result<Vec<u8>> {
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(transport_error)? {
            self.record(|stats| stats.downloaded_bytes += chunk.len() as u64);
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }
}

impl HttpTransport for Transport {
    async fn fetch(&self, request: Request<Buffer>) -> opendal::Result<Response<HttpBody>> {
        let (parts, body) = request.into_parts();
        let method = parts.method;
        let mut current = parts.uri.to_string();
        let mut challenge: Option<String> = None;
        let mut redirects = 0;
        let mut nonce_count = 0;
        let cnonce = webdav_cnonce();
        let body = body.to_bytes();
        loop {
            let mut headers = parts.headers.clone();
            if self.webdav.is_some() && method.as_str() == "MOVE" {
                // Replace rather than append: duplicate Overwrite headers can
                // cause a server to select the original OpenDAL value (T).
                headers.insert("overwrite", http::HeaderValue::from_static("F"));
            }
            let mut request = self
                .client
                .request(method.clone(), &current)
                .headers(headers)
                .body(body.clone());
            if let Some((username, password)) = &self.webdav {
                if let Some(challenge) = &challenge {
                    nonce_count += 1;
                    let authorization = build_digest_authorization(
                        challenge,
                        username,
                        password.as_deref().unwrap_or_default(),
                        method.as_str(),
                        path_and_query(&current),
                        &cnonce,
                        &format!("{nonce_count:08x}"),
                    )
                    .map_err(|_| {
                        Failure::Transport(CloudStorageErrorKind::Unsupported).into_opendal()
                    })?;
                    request = request.header(AUTHORIZATION, authorization);
                } else if !username.is_empty() {
                    request = request.basic_auth(username, password.as_deref());
                }
            }
            self.record(|stats| {
                stats.requests += 1;
                stats.uploaded_bytes += body.len() as u64;
            });
            let response = request.send().await.map_err(transport_error)?;
            let status = response.status();
            let headers = response.headers().clone();
            let retry_after = retry_after_seconds(&headers);
            let next_challenge = if status == StatusCode::UNAUTHORIZED
                && challenge.is_none()
                && self.webdav.as_ref().is_some_and(|(user, password)| {
                    !user.is_empty() && !password.as_deref().unwrap_or_default().is_empty()
                }) {
                digest_challenge_headers(&headers)
            } else {
                None
            };
            let next = if self.webdav.is_some() {
                redirect_target(
                    status,
                    headers.get(LOCATION).and_then(|value| value.to_str().ok()),
                    &current,
                    &method,
                )
                .map_err(|_| {
                    Failure::Transport(CloudStorageErrorKind::Unsupported).into_opendal()
                })?
            } else {
                None
            };
            let response_body = self.body(response).await?;
            if let Some(next_challenge) = next_challenge {
                challenge = Some(next_challenge);
                continue;
            }
            if let Some(next) = next {
                if redirects == 4 {
                    return Err(
                        Failure::Transport(CloudStorageErrorKind::Unsupported).into_opendal()
                    );
                }
                redirects += 1;
                current = next;
                continue;
            }
            if status == StatusCode::NOT_MODIFIED {
                self.record(|stats| stats.not_modified += 1);
            }
            // Surface controlled status/Retry-After instead of backend-specific
            // XML errors, which can include signed URLs or object contents.
            if !status.is_success()
                && !(status == StatusCode::METHOD_NOT_ALLOWED && method.as_str() == "MKCOL")
            {
                return Err(Failure::Http {
                    status: status.as_u16(),
                    retry_after,
                }
                .into_opendal());
            }
            let mut result = Response::builder().status(status);
            *result.headers_mut().expect("valid HTTP response") = headers;
            let length = response_body.len() as u64;
            return result
                .body(HttpBody::new(
                    futures::stream::iter([Ok(Buffer::from(response_body))]),
                    Some(length),
                ))
                .map_err(|_| Error::new(ErrorKind::Unexpected, "invalid cloud HTTP response"));
        }
    }
}
