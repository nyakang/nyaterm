//! Acquire import files before invoking core parsers.
use crate::models::QuickCommandImportPathPromptKind;
use nyaterm_core::quick_command_import::{
    ImportConfig, decode_text, ensure_quick_command_import_size, parse_nyaterm_import,
    parse_windterm_quickbar, parse_xshell_quick_buttons_content,
};
use std::io::Read;

pub(super) fn parse_quick_commands_from_path(
    kind: QuickCommandImportPathPromptKind,
    path: &std::path::Path,
) -> Result<ImportConfig, String> {
    let import_config = match kind {
        QuickCommandImportPathPromptKind::NyatermJson => {
            let raw = read_quick_command_import_text(path)?;
            parse_nyaterm_import(&raw)?
        }
        QuickCommandImportPathPromptKind::WindTermQuickbar => {
            let raw = read_quick_command_import_text(path)?;
            parse_windterm_quickbar(&raw)?
        }
        QuickCommandImportPathPromptKind::XshellXts => parse_xshell_xts_quick_buttons(path)?,
    };
    if import_config.is_empty() {
        return Err("No valid quick commands found in import file".to_string());
    }
    Ok(import_config)
}

pub(super) fn parse_xshell_xts_quick_buttons(
    path: &std::path::Path,
) -> Result<ImportConfig, String> {
    let metadata = std::fs::metadata(path).map_err(|error| error.to_string())?;
    ensure_quick_command_import_size(metadata.len(), "import file")?;
    let file = std::fs::File::open(path)
        .map_err(|error| format!("Cannot open Xshell XTS file: {error}"))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|error| format!("Invalid ZIP/XTS file: {error}"))?;

    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| format!("ZIP entry error: {error}"))?;
        let entry_path = decode_text(entry.name_raw()).replace('\\', "/");
        let normalized_path = entry_path.trim_start_matches("./").trim_start_matches('/');
        let lookup_path = normalized_path.to_ascii_lowercase();
        if lookup_path != "xsl/quickbutton files/commands.qbl"
            && !lookup_path.ends_with("/xsl/quickbutton files/commands.qbl")
        {
            continue;
        }

        ensure_quick_command_import_size(entry.size(), &entry_path)?;
        let mut raw = Vec::new();
        entry
            .read_to_end(&mut raw)
            .map_err(|error| format!("Failed to read {entry_path}: {error}"))?;
        return Ok(parse_xshell_quick_buttons_content(&decode_text(&raw)));
    }

    Err("Xshell quick button file not found: xsl/QuickButton Files/commands.qbl".to_string())
}

fn read_quick_command_import_text(path: &std::path::Path) -> Result<String, String> {
    let metadata = std::fs::metadata(path).map_err(|error| error.to_string())?;
    ensure_quick_command_import_size(metadata.len(), "import file")?;
    std::fs::read_to_string(path).map_err(|error| error.to_string())
}
