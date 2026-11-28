//! Deterministic ACP peer. No model, credentials, network, or tools are used.
use serde_json::{Value, json};
use std::io::{BufRead, Write};

fn send(value: Value) {
    println!("{value}");
    std::io::stdout().flush().unwrap();
}
fn result(id: &Value, result: Value) {
    send(json!({"jsonrpc":"2.0", "id":id, "result":result}));
}
fn delta(session: &str, text: &str) {
    send(
        json!({"jsonrpc":"2.0", "method":"session/update", "params":{
        "sessionId":session,"update":{"sessionUpdate":"agent_message_chunk",
        "content":{"type":"text","text":text}}}}),
    );
}
fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    assert_eq!(
        args.first().map(String::as_str),
        Some("fixture argument with spaces")
    );
    let scenario = args[1].as_str();
    let mut prompt = Value::Null;
    let mut replies = 0;
    let mut token = String::new();
    for line in std::io::stdin().lock().lines().map_while(Result::ok) {
        let request: Value = serde_json::from_str(&line).unwrap();
        let id = &request["id"];
        let params = &request["params"];
        let response = match request["method"].as_str() {
            Some("initialize") => {
                assert_ne!(params["clientCapabilities"]["terminal"], true);
                assert_ne!(params["clientCapabilities"]["fs"]["readTextFile"], true);
                assert_ne!(params["clientCapabilities"]["fs"]["writeTextFile"], true);
                if scenario == "timeout" {
                    continue;
                }
                json!({"protocolVersion":if scenario == "bad-version" { 99 } else { 1 },
                    "agentCapabilities":{"loadSession":scenario != "no-resume"},
                    "authMethods":[{"id":"login","name":"Fixture login"}]})
            }
            Some("authenticate") => {
                assert_eq!(params["methodId"], "login");
                json!({})
            }
            Some("session/new" | "session/load") => {
                token = params["mcpServers"][0]["env"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|row| row["name"] == "REQUEST_TOKEN")
                    .unwrap()["value"]
                    .as_str()
                    .unwrap()
                    .into();
                if request["method"] == "session/load" {
                    delta("fixture-session", "REPLAY");
                }
                let mut response = json!({"sessionId":"fixture-session"});
                if scenario == "legacy-model" {
                    response["models"] = json!({"currentModelId":"fixture", "availableModels":[{"modelId":"fixture","name":"Fixture"}]});
                } else {
                    response["configOptions"] = json!([{"id":"model","name":"Model","category":"model","type":"select","currentValue":"fixture", "options":[{"value":"fixture","name":"Fixture"}]}]);
                }
                response
            }
            Some("session/set_config_option" | "session/set_model") => {
                if scenario == "legacy-model" {
                    assert_eq!(request["method"], "session/set_model");
                    assert_eq!(params["modelId"], "fixture");
                } else {
                    assert_eq!(params["value"], "fixture");
                }
                json!({"configOptions":[]})
            }
            Some("session/prompt") => {
                prompt = id.clone();
                if scenario == "crash" {
                    return;
                }
                if matches!(
                    scenario,
                    "permissions" | "deny" | "cancel" | "foreign" | "no-once"
                ) {
                    let count = if scenario == "permissions" { 2 } else { 1 };
                    for n in 0..count {
                        let options = if scenario == "no-once" {
                            json!([{"optionId":"always","name":"Always","kind":"allow_always"}])
                        } else {
                            json!([{"optionId":"yes","name":"Once","kind":"allow_once"},{"optionId":"no","name":"Reject","kind":"reject_once"}])
                        };
                        send(
                            json!({"jsonrpc":"2.0","id":format!("permission-{n}"),"method":"session/request_permission", "params":{
                            "sessionId":if scenario == "foreign" {"foreign"} else {"fixture-session"},
                            "toolCall":{"toolCallId":format!("tool-{n}"),"title":"Fixture local tool","kind":"execute","rawInput":{"command":"fixture only"}},"options":options}}),
                        );
                    }
                    // Delivery must continue while permission responses are pending.
                    delta("fixture-session", "waiting;");
                    continue;
                }
                delta("foreign", "FOREIGN");
                delta("fixture-session", &token);
                json!({"stopReason":"end_turn"})
            }
            Some("session/cancel") => {
                result(&prompt, json!({"stopReason":"cancelled"}));
                continue;
            }
            None => {
                replies += 1;
                let outcome = &request["result"]["outcome"];
                if scenario == "no-once" {
                    assert_eq!(outcome["outcome"], "cancelled");
                } else {
                    assert_eq!(
                        outcome["optionId"],
                        if scenario == "permissions" {
                            "yes"
                        } else {
                            "no"
                        }
                    );
                }
                if replies == if scenario == "permissions" { 2 } else { 1 } {
                    delta("fixture-session", "done");
                    result(&prompt, json!({"stopReason":"end_turn"}));
                }
                continue;
            }
            _ => {
                send(
                    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"unsupported"}}),
                );
                continue;
            }
        };
        result(id, response);
    }
}
