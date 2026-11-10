use super::{TranslationEndpoints, translate_using};
use nyaterm_core::{TranslationSettings, baidu_translate_signature, youdao_translate_signature};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;
use std::time::Duration;

struct Request {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: String,
}

fn serve(
    responses: Vec<(u16, &'static str)>,
) -> (TranslationEndpoints, thread::JoinHandle<Vec<Request>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoints = TranslationEndpoints {
        root: Some(format!("http://{}", listener.local_addr().unwrap())),
    };
    // A missed dispatch must fail the fixture rather than leave a test hanging.
    listener.set_nonblocking(true).unwrap();
    let worker = thread::spawn(move || {
        let mut requests = Vec::new();
        for (status, body) in responses {
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            std::time::Instant::now() < deadline,
                            "provider did not send a request"
                        );
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("fixture accept failed: {error}"),
                }
            };
            // Windows accepted sockets inherit the listener's nonblocking mode.
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut raw = Vec::new();
            let header_end = loop {
                let mut buffer = [0; 4096];
                let count = stream.read(&mut buffer).unwrap();
                assert!(count > 0, "request ended before its headers");
                raw.extend_from_slice(&buffer[..count]);
                if let Some(end) = raw.windows(4).position(|window| window == b"\r\n\r\n") {
                    break end + 4;
                }
            };
            let header = std::str::from_utf8(&raw[..header_end]).unwrap();
            let mut lines = header.split("\r\n");
            let mut first = lines.next().unwrap().split_whitespace();
            let method = first.next().unwrap().to_string();
            let path = first.next().unwrap().to_string();
            let headers = lines
                .filter_map(|line| line.split_once(':'))
                .map(|(key, value)| (key.to_ascii_lowercase(), value.trim().to_string()))
                .collect::<HashMap<_, _>>();
            let size = headers
                .get("content-length")
                .map(|value| value.parse::<usize>().unwrap())
                .unwrap_or(0);
            while raw.len() < header_end + size {
                let mut buffer = [0; 4096];
                let count = stream.read(&mut buffer).unwrap();
                assert!(count > 0, "request body was truncated");
                raw.extend_from_slice(&buffer[..count]);
            }
            let request = Request {
                method,
                path,
                headers,
                body: String::from_utf8(raw[header_end..header_end + size].to_vec()).unwrap(),
            };
            write!(stream, "HTTP/1.1 {status} OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            requests.push(request);
        }
        requests
    });
    (endpoints, worker)
}

fn settings() -> TranslationSettings {
    TranslationSettings {
        deepl_api_key: "fixture-key:fx".into(),
        baidu_app_id: "fixture-app".into(),
        baidu_app_key: "fixture-key".into(),
        ali_app_id: "fixture-app".into(),
        ali_app_key: "fixture-key".into(),
        youdao_app_id: "fixture-app".into(),
        youdao_app_key: "fixture-key".into(),
        ..Default::default()
    }
}

#[test]
fn google_dispatch_preserves_text_query_and_legacy_language_mapping() {
    let (endpoints, worker) = serve(vec![(
        200,
        r#"{"sentences":[{"trans":"译文"}],"src":"en"}"#,
    )]);
    let result =
        translate_using("google", " hello & world ", "zh", &settings(), &endpoints).unwrap();
    assert_eq!(result.original, "hello & world");
    assert_eq!(result.translated, "译文");
    let requests = worker.join().unwrap();
    assert_eq!(requests[0].method, "GET");
    let url = url::Url::parse(&format!("http://fixture{}", requests[0].path)).unwrap();
    assert!(
        url.query_pairs()
            .any(|(key, value)| key == "q" && value == "hello & world")
    );
    assert!(
        url.query_pairs()
            .any(|(key, value)| key == "tl" && value == "zh-CN")
    );
}

#[test]
fn microsoft_auth_token_is_used_for_the_following_translation_request() {
    let (endpoints, worker) = serve(vec![
        (200, " fixture-token "),
        (
            200,
            r#"[{"detectedLanguage":{"language":"en"},"translations":[{"text":"译文"}]}]"#,
        ),
    ]);
    let result = translate_using("microsoft", "hello", "zh", &settings(), &endpoints).unwrap();
    assert_eq!(result.provider, "microsoft");
    let requests = worker.join().unwrap();
    assert_eq!(requests[0].path, "/microsoft-auth");
    assert_eq!(
        requests[1].headers.get("authorization").unwrap(),
        "Bearer fixture-token"
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&requests[1].body).unwrap(),
        serde_json::json!([{ "Text": "hello" }])
    );
}

#[test]
fn commercial_provider_dispatch_preserves_authentication_and_signed_forms() {
    for (provider, body) in [
        (
            "deepl",
            r#"{"translations":[{"text":"译文","detected_source_language":"EN"}]}"#,
        ),
        (
            "baidu",
            r#"{"from":"en","to":"zh","trans_result":[{"dst":"译文"}]}"#,
        ),
        (
            "ali",
            r#"{"Code":"200","Data":{"Translated":"译文","DetectedLanguage":"en"}}"#,
        ),
        (
            "youdao",
            r#"{"errorCode":"0","translation":["译文"],"l":"en2zh-CHS"}"#,
        ),
    ] {
        let (endpoints, worker) = serve(vec![(200, body)]);
        let result = translate_using(provider, "hello", "zh", &settings(), &endpoints).unwrap();
        assert_eq!(result.translated, "译文");
        let requests = worker.join().unwrap();
        let request = &requests[0];
        assert_eq!(request.method, "POST");
        assert_eq!(request.path, format!("/{provider}"));
        let form = url::form_urlencoded::parse(request.body.as_bytes())
            .into_owned()
            .collect::<HashMap<_, _>>();
        match provider {
            "deepl" => {
                assert_eq!(
                    request.headers.get("authorization").unwrap(),
                    "DeepL-Auth-Key fixture-key:fx"
                );
                assert_eq!(form.get("target_lang").unwrap(), "ZH-HANS");
            }
            "baidu" => assert_eq!(
                form.get("sign").unwrap(),
                &baidu_translate_signature("fixture-app", "hello", &form["salt"], "fixture-key")
            ),
            "youdao" => assert_eq!(
                form.get("sign").unwrap(),
                &youdao_translate_signature(
                    "fixture-app",
                    "hello",
                    &form["salt"],
                    &form["curtime"],
                    "fixture-key"
                )
            ),
            "ali" => {
                assert!(request.headers["authorization"].starts_with("ACS3-HMAC-SHA256"));
                assert_eq!(request.headers["x-acs-action"], "TranslateGeneral");
            }
            _ => unreachable!(),
        }
    }
}

#[test]
fn rejected_http_status_and_malformed_payload_are_returned_to_the_caller() {
    for (status, body) in [(429, "rate limited"), (200, "malformed")] {
        let (endpoints, worker) = serve(vec![(status, body)]);
        assert!(translate_using("google", "hello", "zh", &settings(), &endpoints).is_err());
        worker.join().unwrap();
    }
}

#[test]
fn invalid_input_and_missing_credentials_fail_before_network_io() {
    let endpoints = TranslationEndpoints {
        root: Some("http://127.0.0.1:1".into()),
    };
    assert!(
        translate_using("google", "  ", "zh", &settings(), &endpoints)
            .unwrap_err()
            .contains("empty")
    );
    assert!(
        translate_using("unknown", "hello", "zh", &settings(), &endpoints)
            .unwrap_err()
            .contains("unsupported")
    );
    for provider in ["deepl", "baidu", "ali", "youdao"] {
        assert!(
            translate_using(
                provider,
                "hello",
                "zh",
                &TranslationSettings::default(),
                &endpoints
            )
            .unwrap_err()
            .contains("not configured")
        );
    }
}
