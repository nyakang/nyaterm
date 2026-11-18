use crate::QuickCommandsConfig;

use super::merge::merge_import;
use super::sources::{
    MAX_QUICK_COMMAND_IMPORT_BYTES, decode_text, ensure_quick_command_import_size,
    parse_nyaterm_import, parse_windterm_quickbar, parse_xshell_quick_buttons_content,
};
use crate::quick_command_script::plan_quick_command;

#[test]
fn literal_json_export_import_preserves_mode_and_exact_command_text() {
    let mut config = QuickCommandsConfig::default();
    let text = "  {{变量}} C:\\temp\\r\\n\\t\\p\\\\\n下一行  ";
    let raw = serde_json::json!({"commands": [
        {"id":"literal", "label":"Literal", "command":text, "raw_send":true, "execution_mode":"append"},
        {"id":"legacy", "label":"Legacy", "command":"pwd"},
        {"id":"null", "label":"Null", "command":"pwd", "raw_send":null},
        {"id":"off", "label":"Off", "command":"pwd", "raw_send":false}
    ]}).to_string();
    merge_import(&mut config, parse_nyaterm_import(&raw).unwrap()).unwrap();
    assert_eq!(config.commands[0].command, text);
    assert_eq!(config.commands[0].raw_send, Some(true));
    assert_eq!(config.commands[0].execution_mode.as_deref(), Some("append"));
    assert!(
        config.commands[1..]
            .iter()
            .all(|command| !command.raw_send.unwrap_or_default())
    );

    let exported = crate::models::quick_commands::export_quick_commands_json(config).unwrap();
    let mut restored = QuickCommandsConfig::default();
    merge_import(&mut restored, parse_nyaterm_import(&exported).unwrap()).unwrap();
    assert_eq!(restored.commands[0].command, text);
    assert_eq!(restored.commands[0].raw_send, Some(true));
}

#[test]
fn literal_json_import_rejects_invalid_switch_types_and_blank_commands() {
    assert!(
        parse_nyaterm_import(r#"[{"label":"Bad","command":"pwd","raw_send":"true"}]"#).is_err()
    );
    let import =
        parse_nyaterm_import(r#"[{"label":"Blank","command":" \n ","raw_send":true}]"#).unwrap();
    assert!(merge_import(&mut QuickCommandsConfig::default(), import).is_err());
}

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
fn windterm_bare_carriage_returns_become_visible_script_escapes() {
    // #1083: WindTerm keeps every line of a multi-line Send Text in one string
    // separated by bare carriage returns. Stored as-is they leave the script
    // box showing nothing to edit, so they are imported as `\r` escapes,
    // which send exactly the same bytes.
    let import_config = parse_windterm_quickbar(
        r#"[{
            "quick.group": "快速",
            "quick.label": "NTP-HW",
            "quick.text": "clock timezone Asia/Chongqing add 8\rsys\rntp-service unicast-server 192.168.254.100\rqui\rsa\ry\r",
            "quick.type": "Send Text"
        }]"#,
    )
    .expect("windterm quickbar parses");
    let mut config = QuickCommandsConfig::default();
    merge_import(&mut config, import_config).expect("merge succeeds");

    let command = &config.commands[0];
    assert!(!command.command.contains('\r'));
    assert_eq!(
        command.command,
        r"clock timezone Asia/Chongqing add 8\rsys\rntp-service unicast-server 192.168.254.100\rqui\rsa\ry"
    );
    assert_eq!(command.execution_mode.as_deref(), Some("execute"));

    let sent: Vec<u8> = plan_quick_command(&command.command, true)
        .into_iter()
        .flat_map(|chunk| chunk.bytes)
        .collect();
    assert_eq!(
        sent,
        b"clock timezone Asia/Chongqing add 8\rsys\rntp-service unicast-server 192.168.254.100\rqui\rsa\ry\r"
    );
}

#[test]
fn windterm_keeps_crlf_and_lf_line_breaks_as_lines() {
    let import_config = parse_windterm_quickbar(
        r#"[{"quick.label":"Lines","quick.text":"echo one\r\necho two\necho three","quick.type":"Send Text"}]"#,
    )
    .expect("windterm quickbar parses");

    assert_eq!(
        import_config.commands[0].command,
        "echo one\r\necho two\necho three"
    );
}

#[test]
fn windterm_bare_returns_preserve_execution_after_backslash_runs() {
    for backslashes in 0..=3 {
        for line_ending in ["\r", "\r\n", "\n"] {
            for execute in [false, true] {
                let original = format!(
                    "echo {}\rnext{line_ending}echo {}{line_ending}last\\pdone",
                    "\\".repeat(backslashes),
                    "\\".repeat(backslashes),
                );
                let raw = serde_json::json!([{
                    "quick.label": "Backslashes",
                    "quick.text": if execute {
                        format!("{original}\r")
                    } else {
                        original.clone()
                    },
                    "quick.type": "Send Text",
                }])
                .to_string();
                let mut config = QuickCommandsConfig::default();
                merge_import(&mut config, parse_windterm_quickbar(&raw).unwrap()).unwrap();
                let command = &config.commands[0];
                assert_eq!(
                    command.execution_mode.as_deref(),
                    Some(if execute { "execute" } else { "append" })
                );
                assert_eq!(
                    plan_quick_command(&command.command, execute),
                    plan_quick_command(&original, execute),
                    "backslashes={backslashes}, line_ending={line_ending:?}, execute={execute}"
                );
            }
        }
    }
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
