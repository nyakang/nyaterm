use std::collections::{BTreeMap, HashMap};

use serde_json::Value;

use super::helpers::{map_windterm_icon, trim_optional};
use super::json::parse_import_value;
use super::{ImportCommand, ImportConfig};

pub const MAX_QUICK_COMMAND_IMPORT_BYTES: u64 = 4 * 1024 * 1024;

pub fn parse_nyaterm_import(raw: &str) -> Result<ImportConfig, String> {
    let value = serde_json::from_str::<Value>(raw).map_err(|error| error.to_string())?;
    parse_import_value(value)
}

pub fn parse_windterm_quickbar(raw: &str) -> Result<ImportConfig, String> {
    let entries = serde_json::from_str::<Vec<Value>>(raw)
        .map_err(|error| format!("Invalid WindTerm quickbar JSON: {error}"))?;
    let mut commands = Vec::new();

    for entry in entries {
        let label = entry
            .get("quick.label")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("");
        let raw_command = entry
            .get("quick.text")
            .and_then(Value::as_str)
            .unwrap_or("");
        if label.is_empty() || raw_command.trim().is_empty() {
            continue;
        }
        let (command, has_terminal_newline) = split_windterm_command(raw_command);

        let id = entry
            .get("quick.uuid")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string);
        let category = entry
            .get("quick.group")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string);
        let icon_tag = entry
            .get("quick.icon")
            .and_then(Value::as_str)
            .and_then(map_windterm_icon);
        let quick_type = entry
            .get("quick.type")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        let execution_mode = match (has_terminal_newline, quick_type) {
            (true, _) => "execute",
            (false, value) if value.eq_ignore_ascii_case("Send Text") => "append",
            _ => "execute",
        };

        commands.push(ImportCommand {
            id,
            label: label.to_string(),
            command: escape_windterm_bare_returns(command),
            preserve_command_text: true,
            category_id: None,
            category,
            description: None,
            color_tag: None,
            icon_tag,
            pinned: Some(false),
            execution_mode: Some(execution_mode.to_string()),
            raw_send: None,
            source: Some("manual".to_string()),
            risk_level: None,
            sort_order: None,
        });
    }

    Ok(ImportConfig {
        commands,
        categories: Vec::new(),
    })
}

fn split_windterm_command(raw: &str) -> (&str, bool) {
    const TERMINATORS: [&str; 6] = ["\\r\\n", "\\n", "\\r", "\r\n", "\n", "\r"];

    for terminator in TERMINATORS {
        if let Some(command) = raw.strip_suffix(terminator) {
            return (command, true);
        }
    }

    (raw, false)
}

/// WindTerm separates the lines of a multi-line Send Text with bare carriage
/// returns. The script box cannot show a lone `\r`, so the command looked empty
/// and could not be edited (#1083). Writing it as the `\r` script escape keeps
/// the bytes that get sent identical and leaves the text visible. `\r\n` and
/// `\n` are ordinary line breaks in the editor and stay as they are.
fn escape_windterm_bare_returns(command: &str) -> String {
    let mut escaped = String::with_capacity(command.len());
    let mut chars = command.chars().peekable();
    let mut backslashes = 0;
    while let Some(ch) = chars.next() {
        if ch == '\r' && chars.peek() != Some(&'\n') {
            // An unpaired backslash used to send both itself and the literal
            // CR. Pair it first so it cannot consume the new escape.
            if backslashes % 2 == 1 {
                escaped.push('\\');
            }
            escaped.push_str("\\r");
        } else {
            escaped.push(ch);
        }
        backslashes = if ch == '\\' { backslashes + 1 } else { 0 };
    }
    escaped
}

pub fn ensure_quick_command_import_size(size: u64, label: &str) -> Result<(), String> {
    if size > MAX_QUICK_COMMAND_IMPORT_BYTES {
        return Err(format!(
            "{label} is too large to import ({size} bytes > {MAX_QUICK_COMMAND_IMPORT_BYTES} bytes)"
        ));
    }
    Ok(())
}

pub fn parse_xshell_quick_buttons_content(raw: &str) -> ImportConfig {
    let sections = parse_ini_sections(raw);
    let Some(quick_button) = sections.get("QuickButton") else {
        return ImportConfig::default();
    };

    let mut buttons: BTreeMap<usize, HashMap<String, String>> = BTreeMap::new();
    for (key, value) in quick_button {
        let Some(rest) = key.strip_prefix("Button_") else {
            continue;
        };
        let Some((index, field)) = rest.split_once('_') else {
            continue;
        };
        let Ok(index) = index.parse::<usize>() else {
            continue;
        };

        buttons
            .entry(index)
            .or_default()
            .insert(field.to_string(), value.clone());
    }

    let commands = buttons
        .into_values()
        .filter_map(|fields| {
            let button_type = fields.get("Type").map(String::as_str).unwrap_or("");
            if button_type.trim() != "1" {
                return None;
            }

            let label = fields.get("Name").map(String::as_str).unwrap_or("").trim();
            let command = fields
                .get("Action")
                .map(String::as_str)
                .unwrap_or("")
                .trim();
            if label.is_empty() || command.is_empty() {
                return None;
            }

            Some(ImportCommand {
                id: None,
                label: label.to_string(),
                command: command.to_string(),
                preserve_command_text: false,
                category_id: None,
                category: None,
                description: trim_optional(fields.get("Desc").cloned()),
                color_tag: None,
                icon_tag: None,
                pinned: Some(false),
                execution_mode: Some("append".to_string()),
                raw_send: None,
                source: Some("manual".to_string()),
                risk_level: None,
                sort_order: None,
            })
        })
        .collect();

    ImportConfig {
        commands,
        categories: Vec::new(),
    }
}

pub(super) fn parse_ini_sections(raw: &str) -> HashMap<String, HashMap<String, String>> {
    let mut sections: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut current_section = String::new();

    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }

        if line.starts_with('[') && line.ends_with(']') {
            current_section = line[1..line.len() - 1].to_string();
            sections.entry(current_section.clone()).or_default();
            continue;
        }

        if let Some((key, value)) = line.split_once('=') {
            sections
                .entry(current_section.clone())
                .or_default()
                .insert(key.trim().to_string(), value.trim().to_string());
        }
    }

    sections
}

pub fn decode_text(raw: &[u8]) -> String {
    if let Some((encoding, bom_len)) = encoding_rs::Encoding::for_bom(raw) {
        let (decoded, _, _) = encoding.decode(&raw[bom_len..]);
        return decoded.into_owned();
    }

    match std::str::from_utf8(raw) {
        Ok(value) => value.to_string(),
        Err(_) => {
            let (decoded, _, _) = encoding_rs::GBK.decode(raw);
            decoded.into_owned()
        }
    }
}
