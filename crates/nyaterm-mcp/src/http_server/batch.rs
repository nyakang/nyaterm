//! 2025-03-26 batches use one response stream; no cross-stream replay cursor is exposed.
use super::{Body, HttpServer, MAX_BODY, response};
use bytes::Bytes;
use futures::{StreamExt, stream};
use http::{Request, Response, StatusCode, request::Parts};
use http_body_util::{BodyExt, Full, StreamBody};
use hyper::body::Frame;
use serde_json::{Value, json};
use std::convert::Infallible;

impl HttpServer {
    pub(super) async fn batch(
        &self,
        parts: Parts,
        message: Value,
        version: Option<String>,
    ) -> Response<Body> {
        if version.as_deref() != Some("2025-03-26") {
            return response(StatusCode::BAD_REQUEST);
        }
        let items = message.as_array().expect("array checked");
        if !valid_batch(items) {
            return response(StatusCode::BAD_REQUEST);
        }
        let mut streams = Vec::new();
        for item in items {
            let request = Request::from_parts(
                parts.clone(),
                Full::new(Bytes::from(serde_json::to_vec(item).expect("JSON value"))),
            );
            let result = self.service.handle(request).await;
            if result.status() == StatusCode::ACCEPTED {
                continue;
            }
            let state = BatchStream {
                is_sse: result
                    .headers()
                    .get("content-type")
                    .is_some_and(|v| v.as_bytes().starts_with(b"text/event-stream")),
                success: result.status().is_success(),
                body: result.into_body().boxed_unsync(),
                id: item.get("id").cloned(),
                pending: Vec::new(),
                done: false,
                emitted_json: false,
            };
            streams.push(
                stream::unfold(state, |mut state| async move {
                    state
                        .next_frame()
                        .await
                        .map(|frame| (Ok::<_, Infallible>(frame), state))
                })
                .boxed(),
            );
        }
        if streams.is_empty() {
            return response(StatusCode::ACCEPTED);
        }
        Response::builder()
            .header("content-type", "text/event-stream")
            .body(StreamBody::new(stream::select_all(streams)).boxed_unsync())
            .expect("valid response")
    }
}

fn valid_batch(items: &[Value]) -> bool {
    if items.is_empty() || items.len() > 64 {
        return false;
    }
    let mut responses = false;
    let mut requests = false;
    let mut ids = std::collections::HashSet::new();
    for item in items {
        if item.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            return false;
        }
        if item.get("params").is_some_and(|value| !value.is_object()) {
            return false;
        }
        if let Some(id) = item.get("id") {
            if !id.is_string() && !id.is_i64() && !id.is_u64() {
                return false;
            }
            if !ids.insert(id.to_string()) {
                return false;
            }
        }
        if item.get("method").is_some() {
            if !item["method"].is_string() || item["method"] == "initialize" {
                return false;
            }
            requests = true;
        } else if item.get("id").is_some()
            && (item.get("result").is_some() ^ item.get("error").is_some())
        {
            responses = true;
        } else {
            return false;
        }
    }
    !(requests && responses)
}

struct BatchStream {
    body: Body,
    pending: Vec<u8>,
    id: Option<Value>,
    is_sse: bool,
    success: bool,
    done: bool,
    emitted_json: bool,
}
impl BatchStream {
    fn failure(&self) -> Value {
        json!({"jsonrpc":"2.0","id":self.id,"error":{"code":-32603,"message":"Batch member failed"}})
    }
    async fn next_frame(&mut self) -> Option<Frame<Bytes>> {
        loop {
            if self.is_sse
                && let Some(end) = self.pending.windows(2).position(|w| w == b"\n\n")
            {
                let event: Vec<_> = self.pending.drain(..end + 2).collect();
                let text = String::from_utf8_lossy(&event);
                let data = text
                    .lines()
                    .filter(|line| line.starts_with("data:") || line.starts_with("event:"))
                    .collect::<Vec<_>>()
                    .join("\n");
                // SDK cursors belong to individual streams and cannot describe this merged stream.
                if !data.is_empty() {
                    return Some(Frame::data(Bytes::from(format!("{data}\n\n"))));
                }
                continue;
            }
            if self.done {
                if !self.is_sse && !self.emitted_json {
                    self.emitted_json = true;
                    let value = if self.success {
                        serde_json::from_slice(&self.pending).unwrap_or_else(|_| self.failure())
                    } else {
                        self.failure()
                    };
                    self.pending.clear();
                    return Some(Frame::data(Bytes::from(format!("data: {value}\n\n"))));
                }
                return None;
            }
            match self.body.frame().await {
                Some(Ok(frame)) => {
                    if let Ok(data) = frame.into_data() {
                        self.pending.extend_from_slice(&data);
                        if self.pending.len() > MAX_BODY {
                            self.pending.clear();
                            self.done = true;
                            self.is_sse = false;
                            self.success = false;
                        }
                    }
                }
                _ => self.done = true,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::valid_batch;
    use serde_json::json;
    #[test]
    fn batch_validation_precedes_dispatch_and_keeps_response_batches_separate() {
        assert!(valid_batch(&[
            json!({"jsonrpc":"2.0","id":1,"method":"ping"}),
            json!({"jsonrpc":"2.0","method":"notifications/initialized"})
        ]));
        for items in [
            vec![],
            vec![json!({"jsonrpc":"2.0","id":1,"method":"ping","params":5})],
            vec![
                json!({"jsonrpc":"2.0","id":1,"method":"ping"}),
                json!({"jsonrpc":"2.0","id":2,"result":{}}),
            ],
            vec![json!({"jsonrpc":"2.0","id":1,"method":"ping"}); 2],
        ] {
            assert!(!valid_batch(&items));
        }
    }
}
