use nyaterm_core::plugins::invocation::{
    ActionInput, ActionResult, ParameterValue, validate_result,
};
use nyaterm_core::plugins::manifest::PluginManifest;
use nyaterm_core::plugins::preferences::PluginPreferences;
use nyaterm_core::plugins::template::{expand, validate_template};
use nyaterm_core::plugins::{ErrorCode, validate_id, validate_resource_path};

const HOST: &str = "2.0.0-preview.4";
const V2: &str = include_str!("fixtures/plugins/plugin-v2.toml");

#[test]
fn v2_fixture_loads_and_versions_are_separate() {
    let manifest = PluginManifest::parse(V2, HOST).unwrap();
    assert_eq!(
        manifest.contribution_id(&manifest.actions[0]),
        "diagnostics:inspect"
    );
    let preferences: PluginPreferences =
        serde_json::from_str(include_str!("fixtures/plugins/preferences-v1.json")).unwrap();
    preferences.validate().unwrap();
    assert!(preferences.plugins["diagnostics"].enabled);
    for (from, to) in [
        ("schema_version = 2", "schema_version = 1"),
        ("api_version = \"2.0.0\"", "api_version = \"1.0.0\""),
        (">=2.0.0-preview.4", ">=3.0.0"),
    ] {
        assert_eq!(
            PluginManifest::parse(&V2.replace(from, to), HOST)
                .unwrap_err()
                .code,
            ErrorCode::Incompatible
        );
    }
}

#[test]
fn unknown_fields_capabilities_duplicates_and_bad_metadata_are_rejected() {
    assert!(PluginManifest::parse(&format!("unknown = true\n{V2}"), HOST).is_err());
    assert!(
        PluginManifest::parse(
            &V2.replace(
                "api_version = \"2.0.0\"",
                "api_version = \"2.0.0\"\ncapabilities = [\"unknown_capability\"]"
            ),
            HOST
        )
        .is_err()
    );
    let mut manifest = PluginManifest::parse(V2, HOST).unwrap();
    manifest.actions.push(manifest.actions[0].clone());
    assert_eq!(
        manifest.validate(HOST).unwrap_err().code,
        ErrorCode::Conflict
    );
    manifest.actions.pop();
    manifest.version = "not-semver".into();
    assert!(manifest.validate(HOST).is_err());
}

#[test]
fn paths_reject_cross_platform_traversal_aliases_and_devices() {
    for path in [
        "../a",
        "a/../b",
        "a//b",
        "/root",
        "C:/x",
        "a\\b",
        "a:stream",
        "CON",
        "nul.txt",
        "a/COM1.txt",
        "a/LPT9",
        "a.",
        "a ",
        "\\\\server\\x",
        "x?",
        "x\0",
    ] {
        assert!(validate_resource_path(path).is_err(), "accepted {path:?}");
    }
    for id in ["", "UPPER", "a:b", "0abc", "con", "../a"] {
        assert!(validate_id(id).is_err());
    }
    validate_resource_path("templates/host-check.txt").unwrap();
}

#[test]
fn templates_have_explicit_quoting_and_no_implicit_evaluation() {
    let manifest = PluginManifest::parse(V2, HOST).unwrap();
    let action = &manifest.actions[0];
    let mut input = ActionInput::default();
    input
        .parameters
        .insert("host".into(), ParameterValue::String("a'$(whoami)".into()));
    let result = expand(action, "ping ${host}", &input).unwrap();
    assert!(
        matches!(result, ActionResult::Command { command, .. } if command == "ping 'a'\\''$(whoami)'")
    );
    assert!(validate_template(action, "${unknown}").is_err());
    assert!(validate_template(action, "${host").is_err());
    assert!(expand(action, "ping ${host}", &ActionInput::default()).is_err());
    input
        .parameters
        .insert("host".into(), ParameterValue::String("\x1b[31m".into()));
    assert!(expand(action, "ping ${host}", &input).is_err());
    assert!(validate_result(action.result, &ActionResult::Text("wrong kind".into())).is_err());
}
