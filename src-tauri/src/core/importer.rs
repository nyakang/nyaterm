//! Import sessions from Xshell (.xts), MobaXterm (.mxtsessions), and WindTerm (.sessions) files.

use crate::config::{self, Group, SavedConnection};
use crate::error::{AppError, AppResult};
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use tauri::Emitter;

struct ImportedSession {
    name: String,
    host: String,
    port: u16,
    username: String,
    auth_type: String,
    /// Hierarchical group path segments, e.g. ["亚鸿", "湖北"].
    group_path: Option<Vec<String>>,
}

/// Detect BOM (UTF-8/UTF-16) and decode accordingly; fall back to GBK.
fn decode_bytes(raw: &[u8]) -> String {
    if let Some((enc, bom_len)) = encoding_rs::Encoding::for_bom(raw) {
        let (decoded, _, _) = enc.decode(&raw[bom_len..]);
        return decoded.into_owned();
    }
    match std::str::from_utf8(raw) {
        Ok(s) => s.to_string(),
        Err(_) => {
            let (decoded, _, _) = encoding_rs::GBK.decode(raw);
            decoded.into_owned()
        }
    }
}

// ── Xshell (.xts) ──────────────────────────────────────────────────────────

fn parse_xshell(path: &str) -> AppResult<Vec<ImportedSession>> {
    let file = std::fs::File::open(path)
        .map_err(|e| AppError::Config(format!("Cannot open file: {e}")))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| AppError::Config(format!("Invalid ZIP/XTS file: {e}")))?;

    let mut sessions = Vec::new();

    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| AppError::Config(format!("ZIP entry error: {e}")))?;

        // ZIP filenames on Chinese Windows are typically GBK-encoded
        let entry_path_raw = entry.name_raw().to_vec();
        let entry_path = decode_bytes(&entry_path_raw);
        if !entry_path.ends_with(".xsh") {
            continue;
        }

        let mut raw = Vec::new();
        entry
            .read_to_end(&mut raw)
            .map_err(|e| AppError::Config(format!("Failed to read {entry_path}: {e}")))?;

        let content = decode_bytes(&raw);

        if let Some(sess) = parse_xsh_content(&content, &entry_path) {
            sessions.push(sess);
        }
    }

    Ok(sessions)
}

fn parse_xsh_content(content: &str, entry_path: &str) -> Option<ImportedSession> {
    let sections = parse_ini_sections(content);

    let conn = sections.get("CONNECTION")?;
    let protocol = conn.get("Protocol").map(String::as_str).unwrap_or("");
    if !protocol.eq_ignore_ascii_case("SSH") {
        return None;
    }

    let host = conn.get("Host")?.clone();
    if host.is_empty() {
        return None;
    }

    let port: u16 = conn.get("Port").and_then(|p| p.parse().ok()).unwrap_or(22);

    let auth = sections.get("CONNECTION:AUTHENTICATION");
    let username = auth
        .and_then(|a| a.get("UserName"))
        .cloned()
        .unwrap_or_else(|| "root".to_string());

    let has_user_key = auth
        .and_then(|a| a.get("UserKey"))
        .is_some_and(|k| !k.is_empty());
    let auth_type = if has_user_key { "key" } else { "password" }.to_string();

    let path_obj = Path::new(entry_path);
    let name = path_obj
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Unnamed")
        .to_string();

    let group_path = path_obj.parent().and_then(|p| {
        let p_str = p.to_str().unwrap_or("");
        let stripped = p_str
            .strip_prefix("Xshell/Sessions/")
            .or_else(|| p_str.strip_prefix("Xshell/"))
            .unwrap_or(p_str);
        if stripped.is_empty() {
            None
        } else {
            let segments: Vec<String> = stripped
                .split('/')
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect();
            if segments.is_empty() {
                None
            } else {
                Some(segments)
            }
        }
    });

    Some(ImportedSession {
        name,
        host,
        port,
        username,
        auth_type,
        group_path,
    })
}

fn parse_ini_sections(content: &str) -> HashMap<String, HashMap<String, String>> {
    let mut sections: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut current_section = String::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            current_section = line[1..line.len() - 1].to_string();
            sections.entry(current_section.clone()).or_default();
        } else if let Some((key, value)) = line.split_once('=') {
            if let Some(section) = sections.get_mut(&current_section) {
                section.insert(key.trim().to_string(), value.trim().to_string());
            }
        }
    }

    sections
}

// ── MobaXterm (.mxtsessions) ────────────────────────────────────────────────

fn parse_mobaxterm(path: &str) -> AppResult<Vec<ImportedSession>> {
    let raw =
        std::fs::read(path).map_err(|e| AppError::Config(format!("Cannot read file: {e}")))?;
    let content = decode_bytes(&raw);

    let sections = parse_ini_sections(&content);
    let mut sessions = Vec::new();

    for (section_name, entries) in &sections {
        if !section_name.starts_with("Bookmarks") {
            continue;
        }

        let group_path = entries.get("SubRep").and_then(|s| {
            let s = s.trim();
            if s.is_empty() {
                None
            } else {
                let segments: Vec<String> = s
                    .split('\\')
                    .filter(|seg| !seg.is_empty())
                    .map(|seg| seg.trim().to_string())
                    .collect();
                if segments.is_empty() {
                    None
                } else {
                    Some(segments)
                }
            }
        });

        for (entry_name, value) in entries {
            if entry_name == "SubRep" || entry_name == "ImgNum" {
                continue;
            }

            if let Some(sess) = parse_moba_entry(entry_name, value, &group_path) {
                sessions.push(sess);
            }
        }
    }

    Ok(sessions)
}

fn parse_moba_entry(
    name: &str,
    value: &str,
    group_path: &Option<Vec<String>>,
) -> Option<ImportedSession> {
    // Format: #<type>#<subtype>%host%port%username%...
    let hash_parts: Vec<&str> = value.splitn(2, '#').skip(1).collect::<Vec<_>>();
    if hash_parts.is_empty() {
        return None;
    }

    let after_hash = hash_parts.join("#");
    let type_and_rest: Vec<&str> = after_hash.splitn(2, '%').collect();
    if type_and_rest.len() < 2 {
        return None;
    }

    // Type 109 = SSH
    let type_marker = type_and_rest[0];
    if !type_marker.starts_with("109") {
        return None;
    }

    let fields: Vec<&str> = type_and_rest[1].split('%').collect();
    if fields.len() < 3 {
        return None;
    }

    let host = fields[0].to_string();
    if host.is_empty() {
        return None;
    }

    let port: u16 = fields[1].parse().unwrap_or(22);
    let username = if fields[2].is_empty() {
        "root".to_string()
    } else {
        fields[2].to_string()
    };

    Some(ImportedSession {
        name: name.to_string(),
        host,
        port,
        username,
        auth_type: "password".to_string(),
        group_path: group_path.clone(),
    })
}

// ── WindTerm (user.sessions) ────────────────────────────────────────────────

fn parse_windterm(path: &str) -> AppResult<Vec<ImportedSession>> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| AppError::Config(format!("Cannot read file: {e}")))?;

    let entries: Vec<serde_json::Value> = serde_json::from_str(&content)
        .map_err(|e| AppError::Config(format!("Invalid WindTerm JSON: {e}")))?;

    let mut sessions = Vec::new();

    for entry in &entries {
        let protocol = entry
            .get("session.protocol")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if !protocol.eq_ignore_ascii_case("SSH") {
            continue;
        }

        let host = entry
            .get("session.target")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if host.is_empty() {
            continue;
        }

        let name = entry
            .get("session.label")
            .and_then(|v| v.as_str())
            .unwrap_or(&host)
            .to_string();

        let port: u16 = entry
            .get("session.port")
            .and_then(|v| v.as_u64())
            .map_or(22, |p| p as u16);

        let group_path = entry
            .get("session.group")
            .and_then(|v| v.as_str())
            .and_then(|s| {
                let s = s.trim();
                if s.is_empty() {
                    None
                } else {
                    let segments: Vec<String> = s
                        .split('>')
                        .filter(|seg| !seg.is_empty())
                        .map(|seg| seg.trim().to_string())
                        .collect();
                    if segments.is_empty() {
                        None
                    } else {
                        Some(segments)
                    }
                }
            });

        sessions.push(ImportedSession {
            name,
            host,
            port,
            username: "root".to_string(),
            auth_type: "password".to_string(),
            group_path,
        });
    }

    Ok(sessions)
}

// ── Tauri Command ───────────────────────────────────────────────────────────

pub fn import_sessions(app: tauri::AppHandle, file_path: String) -> AppResult<usize> {
    let lower = file_path.to_lowercase();
    let imported = if lower.ends_with(".xts") {
        parse_xshell(&file_path)?
    } else if lower.ends_with(".mxtsessions") {
        parse_mobaxterm(&file_path)?
    } else if lower.ends_with(".sessions") {
        parse_windterm(&file_path)?
    } else {
        return Err(AppError::Config(
            "Unsupported file format. Please use .xts (Xshell), .mxtsessions (MobaXterm), or .sessions (WindTerm)."
                .to_string(),
        ));
    };

    if imported.is_empty() {
        return Ok(0);
    }

    let mut cfg = config::load_config(&app)?;
    let count = imported.len();

    // Build a path -> id map of existing groups for dedup.
    // Key: full path segments from root to this group.
    let mut path_map: HashMap<Vec<String>, String> = HashMap::new();

    // Reconstruct paths for existing groups by walking parent_id chains
    fn build_path(groups: &[Group], id: &str) -> Vec<String> {
        let mut segments = Vec::new();
        let mut current = id;
        loop {
            if let Some(g) = groups.iter().find(|g| g.id == current) {
                segments.push(g.name.clone());
                if let Some(ref pid) = g.parent_id {
                    current = pid;
                } else {
                    break;
                }
            } else {
                break;
            }
        }
        segments.reverse();
        segments
    }

    for g in &cfg.groups {
        let path = build_path(&cfg.groups, &g.id);
        path_map.insert(path, g.id.clone());
    }

    let mut next_sort = cfg.groups.iter().map(|g| g.sort_order).max().unwrap_or(0) + 1;

    for sess in imported {
        let group_id = sess.group_path.map(|segments| {
            let mut leaf_id = String::new();
            for depth in 1..=segments.len() {
                let prefix: Vec<String> = segments[..depth].to_vec();
                if let Some(existing) = path_map.get(&prefix) {
                    leaf_id = existing.clone();
                } else {
                    let id = uuid::Uuid::new_v4().to_string();
                    let parent_id = if depth > 1 {
                        let parent_prefix: Vec<String> = segments[..depth - 1].to_vec();
                        path_map.get(&parent_prefix).cloned()
                    } else {
                        None
                    };
                    cfg.groups.push(Group {
                        id: id.clone(),
                        name: segments[depth - 1].clone(),
                        parent_id,
                        sort_order: next_sort,
                    });
                    next_sort += 1;
                    path_map.insert(prefix, id.clone());
                    leaf_id = id;
                }
            }
            leaf_id
        });

        cfg.connections.push(SavedConnection {
            id: uuid::Uuid::new_v4().to_string(),
            name: sess.name,
            group_id,
            description: None,
            host: sess.host,
            port: sess.port,
            username: sess.username,
            auth_type: sess.auth_type,
            password: None,
            password_id: None,
            key_id: None,
            sort_order: 0,
            icon: None,
            proxy_id: None,
            otp_id: None,
            auto_fill_otp: false,
            network: Default::default(),
        });
    }

    config::save_config(&app, &cfg)?;
    let _ = app.emit("connections-changed", ());
    Ok(count)
}
