use std::path::Path;
use std::time::Duration;

use nyaterm_core::test_support::TestTempDir;
use nyaterm_mcp_protocol::{
    DiscoveryDocument, PROTOCOL_VERSION, RpcError, RpcRequest, RpcResponse,
};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::net::TcpListener;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use super::{BridgeClient, BridgeEndpoint, cancel_request};

type RequestLines = Lines<BufReader<OwnedReadHalf>>;
const DEADLINE: Duration = Duration::from_secs(2);

async fn request(lines: &mut RequestLines) -> RpcRequest {
    let line = tokio::time::timeout(DEADLINE, lines.next_line())
        .await
        .expect("host request deadline")
        .unwrap()
        .expect("host request");
    serde_json::from_str(&line).unwrap()
}

async fn expect_closed(lines: &mut RequestLines) {
    assert!(
        tokio::time::timeout(DEADLINE, lines.next_line())
            .await
            .expect("failed connection must close without another request")
            .unwrap()
            .is_none()
    );
}

async fn response(writer: &mut OwnedWriteHalf, id: u64, result: Result<Value, &str>) {
    let response = match result {
        Ok(value) => RpcResponse {
            id,
            result: Some(value),
            error: None,
        },
        Err(code) => RpcResponse {
            id,
            result: None,
            error: Some(RpcError {
                code: code.into(),
                message: "fake host rejection".into(),
            }),
        },
    };
    let mut bytes = serde_json::to_vec(&response).unwrap();
    bytes.push(b'\n');
    writer.write_all(&bytes).await.unwrap();
}

async fn accept_auth(
    listener: &TcpListener,
    endpoint: &BridgeEndpoint,
    accepted: bool,
) -> (RequestLines, OwnedWriteHalf) {
    let (stream, _) = tokio::time::timeout(DEADLINE, listener.accept())
        .await
        .expect("host connection deadline")
        .unwrap();
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();
    let auth = request(&mut lines).await;
    assert_eq!(auth.method, "auth");
    assert_eq!(auth.params["token"], endpoint.token);
    assert_eq!(auth.params["generation"], endpoint.generation);
    response(
        &mut writer,
        auth.id,
        if accepted {
            Ok(json!({"authenticated": true, "features": [
                "client_resume", "mcp_lifecycle", "connection_targets"
            ]}))
        } else {
            Err("authentication_failed")
        },
    )
    .await;
    (lines, writer)
}

async fn attach(
    lines: &mut RequestLines,
    writer: &mut OwnedWriteHalf,
    presented: Option<&str>,
    result: Result<&str, &str>,
) {
    let attach = request(lines).await;
    assert_eq!(attach.method, "client.attach");
    assert_eq!(attach.params["resumeToken"].as_str(), presented);
    response(
        writer,
        attach.id,
        result.map(|token| json!({"resumeToken": token})),
    )
    .await;
}

async fn execute(lines: &mut RequestLines, writer: &mut OwnedWriteHalf, value: &str) {
    let execute = request(lines).await;
    assert_eq!(execute.method, "capability.execute");
    response(writer, execute.id, Ok(json!({"value": value}))).await;
}

fn endpoint(listener: &TcpListener, name: &str) -> BridgeEndpoint {
    BridgeEndpoint {
        token: format!("test-token-{name}"),
        generation: format!("test-generation-{name}"),
        ..BridgeEndpoint::for_test(listener.local_addr().unwrap().port())
    }
}

fn write_discovery(path: &Path, endpoint: &BridgeEndpoint) {
    let document = DiscoveryDocument {
        version: PROTOCOL_VERSION,
        pid: 0,
        host: endpoint.host.clone(),
        port: endpoint.port,
        token: endpoint.token.clone(),
        generation: endpoint.generation.clone(),
        permission_mode: "full_access".into(),
    };
    std::fs::write(path, serde_json::to_vec(&document).unwrap()).unwrap();
}

async fn call(client: &BridgeClient) -> Result<Value, RpcError> {
    client
        .call("get_environment", json!({}), CancellationToken::new())
        .await
}

enum ResumeFailure {
    Timeout,
    Disconnect,
    IdMismatch,
    InvalidToken,
    Malformed,
    Rejected,
    FreshRejected,
}

async fn resume_failure(failure: ResumeFailure, expected: &str) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = BridgeEndpoint::for_test_with_timeout(
        listener.local_addr().unwrap().port(),
        Duration::from_millis(100),
    );
    let client = BridgeClient::new(endpoint.clone());
    let host = tokio::spawn(async move {
        let token = "a".repeat(64);
        let (mut lines, mut writer) = accept_auth(&listener, &endpoint, true).await;
        attach(&mut lines, &mut writer, None, Ok(&token)).await;
        // Lose the first response to leave an owner recovery token and force
        // the next caller to reconnect, without replaying the operation.
        assert_eq!(request(&mut lines).await.method, "capability.execute");
        drop((lines, writer));

        let (mut lines, mut writer) = accept_auth(&listener, &endpoint, true).await;
        let resumed = request(&mut lines).await;
        assert_eq!(resumed.method, "client.attach");
        assert_eq!(resumed.params["resumeToken"], token);
        match failure {
            ResumeFailure::Timeout => {}
            ResumeFailure::Disconnect => writer.shutdown().await.unwrap(),
            // A resume_failed code in an invalid envelope must not authorize
            // fallback: it is a connection/protocol failure, not a rejection.
            ResumeFailure::IdMismatch => {
                response(&mut writer, resumed.id + 1, Err("resume_failed")).await;
            }
            ResumeFailure::InvalidToken => {
                response(&mut writer, resumed.id, Ok(json!({"resumeToken": "bad"}))).await;
            }
            ResumeFailure::Malformed => writer.write_all(b"not-json\n").await.unwrap(),
            ResumeFailure::Rejected => {
                response(&mut writer, resumed.id, Err("authentication_failed")).await;
            }
            ResumeFailure::FreshRejected => {
                response(&mut writer, resumed.id, Err("resume_failed")).await;
                attach(&mut lines, &mut writer, None, Err("owner_limit")).await;
            }
        }
        expect_closed(&mut lines).await;

        let (mut lines, mut writer) = accept_auth(&listener, &endpoint, true).await;
        attach(&mut lines, &mut writer, Some(&token), Ok(&token)).await;
        execute(&mut lines, &mut writer, "recovered").await;
    });
    assert_eq!(call(&client).await.unwrap_err().code, "bridge_disconnected");
    assert_eq!(call(&client).await.unwrap_err().code, expected);
    {
        let state = client.state.lock().await;
        assert!(state.connection.is_none());
        assert_eq!(
            state.resume_token.as_ref().unwrap().expose_secret(),
            "a".repeat(64)
        );
    }
    assert!(!client.negotiated.load(std::sync::atomic::Ordering::Acquire));
    assert_eq!(call(&client).await.unwrap()["value"], "recovered");
    host.await.unwrap();
}

#[tokio::test]
async fn resume_timeout_does_not_attach_twice_and_next_call_recovers() {
    resume_failure(ResumeFailure::Timeout, "bridge_timeout").await;
}

#[tokio::test]
async fn resume_disconnect_does_not_reuse_the_socket() {
    resume_failure(ResumeFailure::Disconnect, "bridge_disconnected").await;
}

#[tokio::test]
async fn resume_error_with_wrong_response_id_does_not_authorize_fresh_attach() {
    resume_failure(ResumeFailure::IdMismatch, "bridge_disconnected").await;
}

#[tokio::test]
async fn invalid_resume_result_does_not_authorize_fresh_attach() {
    resume_failure(ResumeFailure::InvalidToken, "bridge_protocol_error").await;
}

#[tokio::test]
async fn malformed_resume_response_does_not_authorize_fresh_attach() {
    resume_failure(ResumeFailure::Malformed, "bridge_disconnected").await;
}

#[tokio::test]
async fn explicit_resume_rejection_other_than_resume_failed_is_propagated() {
    resume_failure(ResumeFailure::Rejected, "authentication_failed").await;
}

#[tokio::test]
async fn failed_fresh_attach_is_not_retried() {
    resume_failure(ResumeFailure::FreshRejected, "owner_limit").await;
}

#[tokio::test]
async fn resume_failed_without_a_previous_owner_does_not_retry_attach() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = endpoint(&listener, "first");
    let client = BridgeClient::new(endpoint.clone());
    let host = tokio::spawn(async move {
        let (mut lines, mut writer) = accept_auth(&listener, &endpoint, true).await;
        attach(&mut lines, &mut writer, None, Err("resume_failed")).await;
        expect_closed(&mut lines).await;
    });
    assert_eq!(call(&client).await.unwrap_err().code, "resume_failed");
    assert!(!client.supports_lifecycle());
    assert!(!client.supports_connection_targets());
    host.await.unwrap();
}

enum DiscoveryFailure {
    Missing,
    Malformed,
    Unreadable,
    Unsafe,
}

async fn fallback_after_rotation(failure: DiscoveryFailure) {
    let root = TestTempDir::new("nyaterm-mcp-last-endpoint");
    std::fs::create_dir_all(root.path()).unwrap();
    let path = root.join("discovery.json");
    let first = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let second = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let first_endpoint = endpoint(&first, "first");
    let second_endpoint = endpoint(&second, "second");
    write_discovery(&path, &first_endpoint);
    let client = BridgeClient::new(first_endpoint.clone()).with_discovery_path(path.clone());
    let clone = client.clone();
    let second_document = second_endpoint.clone();
    let hosts = tokio::spawn(async move {
        let first_token = "a".repeat(64);
        let second_token = "b".repeat(64);
        let (mut lines, mut writer) = accept_auth(&first, &first_endpoint, true).await;
        attach(&mut lines, &mut writer, None, Ok(&first_token)).await;
        execute(&mut lines, &mut writer, "first").await;
        drop((first, lines, writer));

        let (mut lines, mut writer) = accept_auth(&second, &second_endpoint, true).await;
        attach(
            &mut lines,
            &mut writer,
            Some(&first_token),
            Err("resume_failed"),
        )
        .await;
        attach(&mut lines, &mut writer, None, Ok(&second_token)).await;
        execute(&mut lines, &mut writer, "second").await;
        drop((lines, writer));

        let (mut lines, mut writer) = accept_auth(&second, &second_endpoint, true).await;
        attach(
            &mut lines,
            &mut writer,
            Some(&second_token),
            Ok(&second_token),
        )
        .await;
        execute(&mut lines, &mut writer, "fallback").await;
    });
    assert_eq!(call(&client).await.unwrap()["value"], "first");
    write_discovery(&path, &second_document);
    assert_eq!(call(&client).await.unwrap_err().code, "bridge_disconnected");
    assert_eq!(call(&client).await.unwrap()["value"], "second");
    match failure {
        DiscoveryFailure::Missing => std::fs::remove_file(&path).unwrap(),
        DiscoveryFailure::Malformed => std::fs::write(&path, b"{").unwrap(),
        DiscoveryFailure::Unreadable => {
            // A directory is unreadable as a document on every supported OS,
            // including when tests run as root (chmod would not guarantee it).
            std::fs::remove_file(&path).unwrap();
            std::fs::create_dir(&path).unwrap();
        }
        DiscoveryFailure::Unsafe => {
            let unsafe_endpoint = BridgeEndpoint {
                host: "192.0.2.1".into(),
                ..second_document
            };
            write_discovery(&path, &unsafe_endpoint);
        }
    }
    assert_eq!(call(&clone).await.unwrap_err().code, "bridge_disconnected");
    assert_eq!(call(&clone).await.unwrap()["value"], "fallback");
    hosts.await.unwrap();
}

#[tokio::test]
async fn missing_discovery_uses_the_rotated_endpoint_shared_by_client_clones() {
    fallback_after_rotation(DiscoveryFailure::Missing).await;
}

#[tokio::test]
async fn malformed_discovery_uses_the_last_successful_endpoint() {
    fallback_after_rotation(DiscoveryFailure::Malformed).await;
}

#[tokio::test]
async fn unreadable_discovery_uses_the_last_successful_endpoint() {
    fallback_after_rotation(DiscoveryFailure::Unreadable).await;
}

#[tokio::test]
async fn unsafe_discovery_uses_the_last_successful_endpoint() {
    fallback_after_rotation(DiscoveryFailure::Unsafe).await;
}

enum CandidateFailure {
    Authentication,
    Attach,
    Identity,
}

async fn failed_candidate(failure: CandidateFailure, expected: &str) {
    let root = TestTempDir::new("nyaterm-mcp-failed-candidate");
    std::fs::create_dir_all(root.path()).unwrap();
    let path = root.join("discovery.json");
    let first = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let second = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let first_endpoint = endpoint(&first, "first");
    let second_endpoint = endpoint(&second, "second");
    write_discovery(&path, &first_endpoint);
    let client = BridgeClient::new(first_endpoint.clone()).with_discovery_path(path.clone());
    let first_host = tokio::spawn(async move {
        let token = "a".repeat(64);
        for index in 0..2 {
            let (mut lines, mut writer) = accept_auth(&first, &first_endpoint, true).await;
            attach(
                &mut lines,
                &mut writer,
                (index == 1).then_some(token.as_str()),
                Ok(&token),
            )
            .await;
            let identify = request(&mut lines).await;
            assert_eq!(identify.method, "client.identify");
            assert_eq!(identify.params["name"], "candidate-test");
            response(&mut writer, identify.id, Ok(json!({"accepted": true}))).await;
            execute(
                &mut lines,
                &mut writer,
                if index == 0 { "first" } else { "fallback" },
            )
            .await;
        }
    });
    client.identify("candidate-test".into(), None).await;
    assert_eq!(call(&client).await.unwrap()["value"], "first");
    write_discovery(&path, &second_endpoint);
    assert_eq!(call(&client).await.unwrap_err().code, "bridge_disconnected");
    let second_host = tokio::spawn(async move {
        let accepted = !matches!(failure, CandidateFailure::Authentication);
        let (mut lines, mut writer) = accept_auth(&second, &second_endpoint, accepted).await;
        match failure {
            CandidateFailure::Authentication => {}
            CandidateFailure::Attach => {
                attach(
                    &mut lines,
                    &mut writer,
                    Some(&"a".repeat(64)),
                    Err("invalid_argument"),
                )
                .await;
            }
            CandidateFailure::Identity => {
                attach(
                    &mut lines,
                    &mut writer,
                    Some(&"a".repeat(64)),
                    Err("resume_failed"),
                )
                .await;
                attach(&mut lines, &mut writer, None, Ok(&"b".repeat(64))).await;
                let identify = request(&mut lines).await;
                assert_eq!(identify.method, "client.identify");
                response(&mut writer, identify.id, Err("identity_rejected")).await;
            }
        }
        expect_closed(&mut lines).await;
    });
    assert_eq!(call(&client).await.unwrap_err().code, expected);
    second_host.await.unwrap();
    // No automatic fallback connection was made during the failed call. Only
    // the next caller, after discovery disappears, may resume the first owner.
    std::fs::remove_file(path).unwrap();
    assert_eq!(call(&client.clone()).await.unwrap()["value"], "fallback");
    first_host.await.unwrap();
}

#[tokio::test]
async fn failed_candidate_authentication_keeps_the_previous_endpoint_and_owner() {
    failed_candidate(CandidateFailure::Authentication, "bridge_disconnected").await;
}

#[tokio::test]
async fn failed_candidate_attach_keeps_the_previous_endpoint_and_owner() {
    failed_candidate(CandidateFailure::Attach, "invalid_argument").await;
}

#[tokio::test]
async fn failed_candidate_identity_replay_does_not_publish_its_new_owner() {
    failed_candidate(CandidateFailure::Identity, "identity_rejected").await;
}

async fn cancellation_during_rotation(reject_resume: bool) {
    let root = TestTempDir::new("nyaterm-mcp-cancel-rotation");
    std::fs::create_dir_all(root.path()).unwrap();
    let path = root.join("discovery.json");
    let first = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let second = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let first_endpoint = endpoint(&first, "first");
    let second_endpoint = endpoint(&second, "second");
    write_discovery(&path, &first_endpoint);
    let client = BridgeClient::new(first_endpoint.clone()).with_discovery_path(path.clone());
    let caller = client.clone();
    let (execute_tx, execute_rx) = oneshot::channel();
    let (cancel_auth_tx, cancel_auth_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let (cancel_seen_tx, cancel_seen_rx) = oneshot::channel();
    let first_host = tokio::spawn(async move {
        let token = "a".repeat(64);
        let (mut lines, mut writer) = accept_auth(&first, &first_endpoint, true).await;
        attach(&mut lines, &mut writer, None, Ok(&token)).await;
        let execute = request(&mut lines).await;
        assert_eq!(execute.method, "capability.execute");
        let request_id = execute.params["requestId"].as_str().unwrap().to_string();
        execute_tx.send(()).unwrap();

        let (mut cancel_lines, mut cancel_writer) =
            accept_auth(&first, &first_endpoint, true).await;
        cancel_auth_tx.send(()).unwrap();
        tokio::time::timeout(DEADLINE, release_rx)
            .await
            .unwrap()
            .unwrap();
        // By now the shared client has attached to B with a different owner.
        // Cancellation must still present the token captured for execution A.
        attach(
            &mut cancel_lines,
            &mut cancel_writer,
            Some(&token),
            if reject_resume {
                Err("resume_failed")
            } else {
                Ok(&token)
            },
        )
        .await;
        if reject_resume {
            expect_closed(&mut cancel_lines).await;
        } else {
            let cancel = request(&mut cancel_lines).await;
            assert_eq!(cancel.method, "request.cancel");
            assert_eq!(cancel.params["requestId"], request_id);
            response(
                &mut cancel_writer,
                cancel.id,
                Ok(json!({"cancelled": true})),
            )
            .await;
        }
        cancel_seen_tx.send(()).unwrap();
    });
    let cancellation = CancellationToken::new();
    let task_token = cancellation.clone();
    let call_task =
        tokio::spawn(async move { caller.call("get_environment", json!({}), task_token).await });
    tokio::time::timeout(DEADLINE, execute_rx)
        .await
        .unwrap()
        .unwrap();
    write_discovery(&path, &second_endpoint);
    cancellation.cancel();
    assert_eq!(
        tokio::time::timeout(DEADLINE, call_task)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err()
            .code,
        "cancelled"
    );
    tokio::time::timeout(DEADLINE, cancel_auth_rx)
        .await
        .unwrap()
        .unwrap();

    let second_host = tokio::spawn(async move {
        let (mut lines, mut writer) = accept_auth(&second, &second_endpoint, true).await;
        attach(
            &mut lines,
            &mut writer,
            Some(&"a".repeat(64)),
            Err("resume_failed"),
        )
        .await;
        attach(&mut lines, &mut writer, None, Ok(&"b".repeat(64))).await;
        execute(&mut lines, &mut writer, "second").await;
        // B must receive only the explicitly submitted successor call.
        assert!(
            tokio::time::timeout(Duration::from_millis(25), second.accept())
                .await
                .is_err()
        );
    });
    assert_eq!(call(&client).await.unwrap()["value"], "second");
    release_tx.send(()).unwrap();
    tokio::time::timeout(DEADLINE, cancel_seen_rx)
        .await
        .unwrap()
        .unwrap();
    first_host.await.unwrap();
    second_host.await.unwrap();
}

#[tokio::test]
async fn cancellation_retains_execution_endpoint_and_owner_across_discovery_rotation() {
    cancellation_during_rotation(false).await;
}

#[tokio::test]
async fn cancellation_resume_failure_does_not_attach_a_replacement_owner() {
    cancellation_during_rotation(true).await;
}

#[tokio::test]
async fn cancellation_without_an_owner_never_creates_one() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = endpoint(&listener, "cancel");
    let cancel_endpoint = endpoint.clone();
    let host = tokio::spawn(async move {
        let (mut lines, _writer) = accept_auth(&listener, &endpoint, true).await;
        expect_closed(&mut lines).await;
    });
    assert!(
        cancel_request(&cancel_endpoint, "request-id", None)
            .await
            .is_err()
    );
    host.await.unwrap();
}
