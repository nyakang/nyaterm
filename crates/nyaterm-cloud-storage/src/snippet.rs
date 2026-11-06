use super::metrics::{CountedRequest, CountedResponse};
use std::time::Duration;

use nyaterm_core::{
    CloudSyncError, SnippetHttpClient, SnippetHttpMethod, SnippetHttpRequest, SnippetHttpResponse,
};

#[derive(Clone)]
pub struct NativeSnippetHttpClient {
    client: zed_reqwest::blocking::Client,
}

impl NativeSnippetHttpClient {
    pub fn new() -> Result<Self, CloudSyncError> {
        let client = zed_reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent("NyaTerm")
            .build()
            .map_err(map_http_error)?;
        Ok(Self { client })
    }
}

impl SnippetHttpClient for NativeSnippetHttpClient {
    fn send(&self, request: SnippetHttpRequest) -> Result<SnippetHttpResponse, CloudSyncError> {
        let provider = if request.headers.contains_key("X-GitHub-Api-Version") {
            "GitHub Gist"
        } else {
            "Gitee snippet"
        };
        let mut builder = match request.method {
            SnippetHttpMethod::Get => self.client.get(&request.url),
            SnippetHttpMethod::Patch => self.client.patch(&request.url),
        };

        if !request.query.is_empty() {
            builder = builder.query(&request.query);
        }
        for (name, value) in &request.headers {
            builder = builder.header(name.as_str(), value.as_str());
        }
        if let Some(body) = request.json_body {
            builder = builder.json(&body);
        }

        let response = builder
            .send_counted()
            .map_err(|error| super::helpers::safe_http_error(provider, error))?;
        let status = response.status().as_u16();
        let retry = super::helpers::retry_after_seconds(response.headers());
        if matches!(status, 401 | 403 | 429 | 500..=599) {
            let _ = response.bytes_counted();
            return Err(super::helpers::http_failure(
                provider, "request", status, retry,
            ));
        }
        let body = response.text_counted()?;
        Ok(SnippetHttpResponse { status, body })
    }
}

fn map_http_error(error: zed_reqwest::Error) -> CloudSyncError {
    super::helpers::safe_http_error("snippet", error)
}
