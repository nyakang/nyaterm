use nyaterm_mcp_protocol::{MCP_TOOL_REGISTRY, tool};
use serde_json::{Value, json};

use crate::terminal_start::input_schema;

#[test]
fn all_advertised_tool_inputs_are_valid_json_schemas() {
    for definition in MCP_TOOL_REGISTRY {
        for targets in [false, true] {
            let schema = input_schema(definition.tool, targets).unwrap();
            jsonschema::validator_for(&schema).unwrap();
        }
    }
}

#[test]
fn inline_targets_preserve_required_fields_variants_and_exclusivity() {
    for targets in [false, true] {
        let schema = input_schema(tool::SFTP_LIST, targets).unwrap();
        let validator = jsonschema::validator_for(&schema).unwrap();
        assert!(validator.is_valid(&json!({"sessionId":"s", "path":"/tmp"})));
        assert!(!validator.is_valid(&json!({"path":"/tmp"})));
        assert!(!validator.is_valid(&json!({"sessionId":"s"})));
        for target in [
            json!({"type":"connection", "connectionId":"c"}),
            json!({"type":"session", "sessionId":"s"}),
        ] {
            assert_eq!(
                validator.is_valid(&json!({"target":target, "path":"/tmp"})),
                targets
            );
            assert!(!validator.is_valid(&json!({
                "target":target, "sessionId":"s", "path":"/tmp"
            })));
        }
        for target in [
            json!({"type":"connection"}),
            json!({"type":"session"}),
            json!({"type":"connection", "sessionId":"s"}),
            json!({"type":"session", "connectionId":"c"}),
            json!({"type":"unknown", "connectionId":"c"}),
            json!({"type":"connection", "connectionId":"c", "extra":true}),
        ] {
            assert!(!validator.is_valid(&json!({"target":target, "path":"/tmp"})));
        }
    }
}

#[test]
fn inline_enums_preserve_defaults_nullability_and_allowed_values() {
    for targets in [false, true] {
        let schema = input_schema(tool::SESSION_OPEN, targets).unwrap();
        let validator = jsonschema::validator_for(&schema).unwrap();
        assert_eq!(schema["properties"]["mode"]["default"], "create");
        assert!(validator.is_valid(&json!({"connectionId":"c"})));
        for mode in ["create", "reuse_or_create"] {
            assert!(validator.is_valid(&json!({"connectionId":"c", "mode":mode})));
        }
        for mode in [Value::Null, json!("invalid"), json!(1)] {
            assert!(!validator.is_valid(&json!({"connectionId":"c", "mode":mode})));
        }

        let schema = input_schema(tool::TERMINAL_START, targets).unwrap();
        let validator = jsonschema::validator_for(&schema).unwrap();
        assert_eq!(
            schema["properties"]["backend"]["default"],
            if targets {
                Value::Null
            } else {
                json!("inline_pty")
            }
        );
        assert!(validator.is_valid(&json!({"command":"pwd"})));
        assert!(!validator.is_valid(&json!({})));
        for backend in ["inline_pty", "ssh_exec"] {
            assert!(validator.is_valid(&json!({"command":"pwd", "backend":backend})));
        }
        assert_eq!(
            validator.is_valid(&json!({"command":"pwd", "backend":null})),
            targets
        );
        for backend in [json!("invalid"), json!(1)] {
            assert!(!validator.is_valid(&json!({"command":"pwd", "backend":backend})));
        }
    }
}
