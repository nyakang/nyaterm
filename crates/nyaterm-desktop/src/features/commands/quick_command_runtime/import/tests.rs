use nyaterm_core::QuickCommandsConfig;

use super::merge::merge_import;
use super::sources::{
    MAX_QUICK_COMMAND_IMPORT_BYTES, decode_text, ensure_quick_command_import_size,
    parse_nyaterm_import, parse_windterm_quickbar, parse_xshell_quick_buttons_content,
};

#[test]
fn imports_config_and_array_shapes() {
    let config = parse_nyaterm_import(
        r#"{"categories":[{"id":"ops","name":"Ops"}],"commands":[{"id":"c1","label":"List","command":"ls","category_id":"ops"}]}"#,
    )
    .expect("config import parses");
    assert_eq!(config.categories.len(), 1);
    assert_eq!(config.commands.len(), 1);

    let config =
        parse_nyaterm_import(r#"[{"label":"Pwd","command":"pwd"}]"#).expect("array import parses");
    assert_eq!(config.categories.len(), 0);
    assert_eq!(config.commands.len(), 1);
}

#[test]
fn macro_import_preserves_script_whitespace_source_and_literal_escapes() {
    let import_config = parse_nyaterm_import(
        r#"{"commands":[{"id":"macro-1","label":"Recorded","command":"echo {{name}} \\x1B\\r ","execution_mode":"append","source":"macro"}]}"#,
    )
    .unwrap();
    let mut config = QuickCommandsConfig::default();
    merge_import(&mut config, import_config).unwrap();
    assert_eq!(config.commands[0].source.as_deref(), Some("macro"));
    assert_eq!(config.commands[0].execution_mode.as_deref(), Some("append"));
    assert_eq!(config.commands[0].command, "echo {{name}} \\x1B\\r ");
    let invalid = parse_nyaterm_import(
        r#"{"commands":[{"label":"Unsupported","command":"pwd","source":"unknown"}]}"#,
    )
    .unwrap();
    assert!(merge_import(&mut config, invalid).is_err());
}

#[test]
fn merge_import_updates_commands_and_creates_named_categories() {
    let mut config = QuickCommandsConfig::default();
    let import_config = parse_nyaterm_import(
        r#"{"commands":[{"id":"c1","label":"List","command":"ls","category":"Ops"},{"id":"c1","label":"Dupe","command":"pwd"}]}"#,
    )
    .expect("json parses");
    let error = merge_import(&mut config, import_config).expect_err("duplicate ids fail");
    assert!(error.contains("Duplicate command id"));

    let import_config = parse_nyaterm_import(
        r#"{"commands":[{"id":"c1","label":"List","command":"ls","category":"Ops"},{"id":"c2","label":"Pwd","command":"pwd","execution_mode":"append"}]}"#,
    )
    .expect("json parses");
    let summary = merge_import(&mut config, import_config).expect("merge succeeds");
    assert_eq!(summary.imported_commands, 2);
    assert_eq!(summary.imported_categories, 1);
    assert_eq!(config.categories[0].name, "Ops");
    assert_eq!(config.commands[1].execution_mode.as_deref(), Some("append"));

    let import_config =
        parse_nyaterm_import(r#"[{"id":"c1","label":"List all","command":"ls -la"}]"#)
            .expect("json parses");
    let summary = merge_import(&mut config, import_config).expect("update succeeds");
    assert_eq!(summary.updated_commands, 1);
    assert_eq!(config.commands[0].label, "List all");
}

#[test]
fn imports_windterm_quickbar_json() {
    let import_config = parse_windterm_quickbar(
        r#"[{
            "quick.group": "快速",
            "quick.icon": "session::docker-blue",
            "quick.label": "miniconda3 安装",
            "quick.text": "echo install",
            "quick.type": "Send Text",
            "quick.uuid": "70127d80-24b8-46eb-958d-f944c5e423dd"
        }]"#,
    )
    .expect("windterm quickbar parses");
    let mut config = QuickCommandsConfig::default();

    let summary = merge_import(&mut config, import_config).expect("merge succeeds");

    assert_eq!(summary.imported_commands, 1);
    assert_eq!(summary.imported_categories, 1);
    assert_eq!(config.categories[0].name, "快速");
    assert_eq!(
        config.commands[0].id,
        "70127d80-24b8-46eb-958d-f944c5e423dd"
    );
    assert_eq!(config.commands[0].label, "miniconda3 安装");
    assert_eq!(config.commands[0].command, "echo install");
    assert_eq!(config.commands[0].execution_mode.as_deref(), Some("append"));
    assert_eq!(config.commands[0].source.as_deref(), Some("manual"));
    assert_eq!(config.commands[0].icon_tag.as_deref(), Some("docker"));
    assert_eq!(config.commands[0].pinned, Some(false));
}

#[test]
fn windterm_import_preserves_trailing_command_whitespace_through_merge() {
    let import_config = parse_windterm_quickbar(
        r#"[{
            "quick.label": "Prompt",
            "quick.text": "echo ready ",
            "quick.type": "Send Text"
        }]"#,
    )
    .expect("windterm quickbar parses");
    assert_eq!(import_config.commands[0].command, "echo ready ");

    let mut config = QuickCommandsConfig::default();
    merge_import(&mut config, import_config).expect("merge succeeds");

    assert_eq!(config.commands[0].command, "echo ready ");
    assert_eq!(config.commands[0].execution_mode.as_deref(), Some("append"));
}

#[test]
fn windterm_defaults_execute_and_skips_empty_entries() {
    let import_config = parse_windterm_quickbar(
        r#"[
            {"quick.label":"","quick.text":"echo no"},
            {"quick.label":"No text"},
            {"quick.label":"Version","quick.text":"rustc --version","quick.type":"Run Command","quick.icon":"Typescript"}
        ]"#,
    )
    .expect("windterm quickbar parses");

    assert_eq!(import_config.commands.len(), 1);
    assert_eq!(import_config.commands[0].label, "Version");
    assert_eq!(
        import_config.commands[0].execution_mode.as_deref(),
        Some("execute")
    );
    assert_eq!(import_config.commands[0].icon_tag.as_deref(), Some("ts"));
}

#[test]
fn windterm_strips_real_and_escaped_terminal_newlines_and_executes() {
    let import_config = parse_windterm_quickbar(
        r#"[
            {"quick.label":"Escaped newline","quick.text":"echo one\\n","quick.type":"Send Text"},
            {"quick.label":"Real newline","quick.text":"echo two\n","quick.type":"Send Text"}
        ]"#,
    )
    .expect("windterm quickbar parses");

    assert_eq!(import_config.commands.len(), 2);
    assert_eq!(import_config.commands[0].command, "echo one");
    assert_eq!(
        import_config.commands[0].execution_mode.as_deref(),
        Some("execute")
    );
    assert_eq!(import_config.commands[1].command, "echo two");
    assert_eq!(
        import_config.commands[1].execution_mode.as_deref(),
        Some("execute")
    );
}

#[test]
fn imports_xshell_quick_buttons_type_one_only() {
    let import_config = parse_xshell_quick_buttons_content(
        r#"[Info]
Version=8.2
Count=3
Expanded=1
[QuickButton]
Button_0_Name=测试
Button_1_Name=Pwd
Button_2_Name=Ignored
Button_0_Type=1
Button_1_Type=1
Button_2_Type=2
Button_0_Action=ls -la
Button_1_Action=pwd
Button_2_Action=whoami
Button_0_Desc=List files
"#,
    );
    let mut config = QuickCommandsConfig::default();

    let summary = merge_import(&mut config, import_config).expect("merge succeeds");

    assert_eq!(summary.imported_commands, 2);
    assert_eq!(config.commands[0].label, "测试");
    assert_eq!(config.commands[0].command, "ls -la");
    assert_eq!(
        config.commands[0].description.as_deref(),
        Some("List files")
    );
    assert_eq!(config.commands[0].execution_mode.as_deref(), Some("append"));
    assert_eq!(config.commands[0].source.as_deref(), Some("manual"));
    assert_eq!(config.commands[1].label, "Pwd");
    assert_eq!(config.commands[1].command, "pwd");
}

#[test]
fn decodes_xshell_text_with_utf_bom_and_gbk_fallback() {
    let utf16_le = [0xff, 0xfe, b'T', 0, b'E', 0, b'S', 0, b'T', 0];
    assert_eq!(decode_text(&utf16_le), "TEST");

    let gbk = [0xb2, 0xe2, 0xca, 0xd4];
    assert_eq!(decode_text(&gbk), "测试");
}

#[test]
fn rejects_import_payloads_over_size_budget() {
    assert!(ensure_quick_command_import_size(MAX_QUICK_COMMAND_IMPORT_BYTES, "import").is_ok());
    let error = ensure_quick_command_import_size(MAX_QUICK_COMMAND_IMPORT_BYTES + 1, "import")
        .expect_err("oversized import fails");
    assert!(error.contains("too large to import"));
}
