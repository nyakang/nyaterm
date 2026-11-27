use nyaterm_mcp_protocol::http_control::{CONTROL_VERSION, HttpBootstrap, HttpControlEvent};
use std::{process::Stdio, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

async fn start(port: u16) -> (tokio::process::Child, String) {
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_nyaterm-mcp"))
        .args(["--transport", "streamable-http", "--managed"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let bootstrap = HttpBootstrap {
        version: CONTROL_VERSION,
        port,
        allowed_origins: vec![],
        bridge_port: 1,
        bridge_token: "fixture".into(),
        bridge_generation: "fixture".into(),
    };
    let mut bytes = serde_json::to_vec(&bootstrap).unwrap();
    bytes.push(b'\n');
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(&bytes)
        .await
        .unwrap();
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(5), reader.read_line(&mut line))
        .await
        .unwrap()
        .unwrap();
    match serde_json::from_str(&line).unwrap() {
        HttpControlEvent::Ready {
            version,
            port: actual,
            token,
        } => {
            assert_eq!(version, CONTROL_VERSION);
            assert_eq!(actual, port);
            (child, token)
        }
        _ => panic!("helper did not become ready"),
    }
}
async fn stop(mut child: tokio::process::Child) {
    drop(child.stdin.take());
    assert!(
        tokio::time::timeout(Duration::from_secs(5), child.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
}

#[tokio::test]
async fn managed_helper_releases_port_on_eof_and_rotates_credentials() {
    let reserve = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = reserve.local_addr().unwrap().port();
    drop(reserve);
    let (first, old) = start(port).await;
    stop(first).await;
    let (second, new) = start(port).await;
    assert_ne!(old, new);
    let client = reqwest::Client::new();
    let url = format!("http://127.0.0.1:{port}/mcp");
    assert_eq!(
        client
            .get(&url)
            .bearer_auth(old)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        client
            .get(&url)
            .bearer_auth(new)
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    stop(second).await;
    assert!(std::net::TcpListener::bind(("127.0.0.1", port)).is_ok());
}

#[tokio::test]
async fn managed_helper_rejects_occupied_port_and_malformed_bootstrap() {
    let occupied = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    for config in [serde_json::json!({"version":1,"port":occupied.local_addr().unwrap().port(),"allowed_origins":[],"bridge_port":1,"bridge_token":"fixture","bridge_generation":"fixture"}).to_string(),"{}".into()] {
        let mut child=tokio::process::Command::new(env!("CARGO_BIN_EXE_nyaterm-mcp"))
            .args(["--transport","streamable-http","--managed"]).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).kill_on_drop(true).spawn().unwrap();
        child.stdin.as_mut().unwrap().write_all(format!("{config}\n").as_bytes()).await.unwrap();
        // Keep stdin open: listener/validation failure must not hang in a Tokio stdin worker.
        assert!(!tokio::time::timeout(Duration::from_secs(5),child.wait()).await.unwrap().unwrap().success());
    }
}
