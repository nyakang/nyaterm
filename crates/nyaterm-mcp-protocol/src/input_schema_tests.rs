use serde_json::Value;

use crate::{MCP_TOOL_REGISTRY, tool_input_schema, tool_input_schema_for_features};

fn assert_inline(value: &Value, path: &str) {
    match value {
        Value::Object(fields) => {
            for (key, value) in fields {
                assert!(
                    !matches!(key.as_str(), "$ref" | "$defs" | "definitions"),
                    "tool parameters must be inline: {path}/{key}"
                );
                assert_inline(value, &format!("{path}/{key}"));
            }
        }
        Value::Array(items) => {
            for (index, value) in items.iter().enumerate() {
                assert_inline(value, &format!("{path}/{index}"));
            }
        }
        _ => {}
    }
}

#[test]
fn all_tool_inputs_are_inline_for_native_providers_and_both_host_versions() {
    for definition in MCP_TOOL_REGISTRY {
        let name = definition.tool;
        assert_inline(&tool_input_schema(name).unwrap(), name);
        for targets in [false, true] {
            assert_inline(
                &tool_input_schema_for_features(name, targets).unwrap(),
                &format!("{name}/connection_targets={targets}"),
            );
        }
    }
}
