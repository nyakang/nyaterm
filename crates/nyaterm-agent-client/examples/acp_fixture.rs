//! Local acceptance fixture: no terminal execution, network access, or credentials.
use serde_json::{Value, json};
use std::io::{BufRead, Write};
fn main() {
    if let Ok(path) = std::env::var("ACCEPTANCE_FORBIDDEN_START") {
        std::fs::write(path, "forbidden test server launched").unwrap();
    }
    for line in std::io::stdin().lock().lines().map_while(Result::ok) {
        let Ok(request) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if request.get("id").is_none() {
            continue;
        }
        let result = match request["method"].as_str().unwrap_or_default() {
            "initialize" => {
                json!({"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"nyaterm-acceptance","version":"1"}})
            }
            "tools/list" => {
                json!({"tools":[{"name":"acceptance_ping","description":"Return the fixed acceptance marker; no side effects","inputSchema":{"type":"object","properties":{},"additionalProperties":false}}]})
            }
            "tools/call" => {
                json!({"content":[{"type":"text","text":format!("NYATERM_MCP_ACCEPTANCE_OK:{}",std::env::var("ACCEPTANCE_REQUEST_ID").unwrap_or_default())}]})
            }
            _ => json!({}),
        };
        println!(
            "{}",
            json!({"jsonrpc":"2.0","id":request["id"],"result":result})
        );
        let _ = std::io::stdout().flush();
    }
}
