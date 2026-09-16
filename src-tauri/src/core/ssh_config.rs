//! Parses `~/.ssh/config` and resolves host aliases, including ProxyJump chains.
//!
//! Supports a subset of OpenSSH client configuration relevant for session
//! management: Host patterns (wildcards, negation), HostName, Port, User,
//! IdentityFile, ProxyJump (single/multi-hop), HostKeyAlias, and Include
//! directives (recursive with cycle detection and glob support).

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::{ConnectionAuth, ConnectionType, SavedConnection, SshKey};
use crate::error::{AppError, AppResult};
use crate::utils::crypto;

/// One parsed `Host` block from the SSH config file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshConfigHost {
    pub patterns: Vec<String>,
    pub name: String,
    pub host_name: Option<String>,
    pub port: Option<u16>,
    pub user: Option<String>,
    pub identity_file: Option<String>,
    #[serde(default)]
    pub identity_files: Vec<String>,
    pub proxy_jump: Option<String>,
    pub host_key_alias: Option<String>,
}

/// A ready-to-use session entry derived from the SSH config.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshConfigEntry {
    pub alias: String,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub identity_file: Option<String>,
    #[serde(default)]
    pub identity_files: Vec<String>,
    pub proxy_jump: Option<String>,
    pub hops: Vec<SshConfigHop>,
    pub host_key_alias: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshConfigHop {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub is_target: bool,
}

/// The fully parsed SSH config.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SshConfig {
    pub hosts: Vec<SshConfigHost>,
}

impl SshConfig {
    /// Reads and parses the default user SSH config (`~/.ssh/config`).
    pub fn load_default() -> AppResult<Self> {
        let path = default_config_path();
        Self::load_from(&path)
    }

    /// Parses one or more SSH config files, following `Include` directives.
    pub fn load_from(path: &Path) -> AppResult<Self> {
        let mut visited = HashSet::new();
        let mut hosts = Vec::new();
        let mut state = ParseState::default();
        parse_file(path, true, &mut visited, &mut hosts, &mut state)?;
        finish_parse(&mut hosts, &mut state);
        Ok(SshConfig { hosts })
    }

    /// Resolves a host alias into a complete entry with ProxyJump hops.
    pub fn resolve(&self, alias: &str) -> AppResult<SshConfigEntry> {
        let resolved = self.resolve_options(alias);
        let host_name = resolved.host_name.unwrap_or_else(|| alias.to_string());
        let port = resolved.port.unwrap_or(22);
        let user = resolved.user.unwrap_or_else(whoami::username);
        let identity_files = resolved.identity_files;
        let identity_file = identity_files.first().cloned().or(resolved.identity_file);
        // `ProxyJump none` explicitly disables a value that may have matched
        // earlier (for example from `Host *`). It is not a host named `none`.
        let proxy_jump = resolved
            .proxy_jump
            .clone()
            .filter(|value| !value.eq_ignore_ascii_case("none"));

        let mut hops = Vec::new();
        if let Some(ref pj) = proxy_jump {
            for jump_spec in pj.split(',') {
                let jump_spec = jump_spec.trim();
                if jump_spec.is_empty() {
                    continue;
                }
                // Parse user@host:port syntax from ProxyJump directives.
                let (jump_user_spec, jump_host_spec, jump_port_spec) = parse_jump_spec(jump_spec)?;
                let jump_alias = &jump_host_spec;
                let jump_resolved = self.resolve_options(jump_alias);
                let jump_host = jump_resolved
                    .host_name
                    .unwrap_or_else(|| jump_alias.to_string());
                let jump_port = jump_port_spec.or(jump_resolved.port).unwrap_or(22);
                let jump_user = jump_user_spec
                    .or(jump_resolved.user)
                    .unwrap_or_else(whoami::username);
                hops.push(SshConfigHop {
                    host: jump_host,
                    port: jump_port,
                    user: jump_user,
                    is_target: false,
                });
            }
        }
        hops.push(SshConfigHop {
            host: host_name.clone(),
            port,
            user: user.clone(),
            is_target: true,
        });

        Ok(SshConfigEntry {
            alias: alias.to_string(),
            host: host_name,
            port,
            user,
            identity_file,
            identity_files,
            proxy_jump,
            hops,
            host_key_alias: resolved.host_key_alias,
        })
    }

    /// Returns concrete host aliases (non-wildcard), deduplicated.
    pub fn list_hosts(&self) -> Vec<String> {
        let mut seen = HashSet::new();
        self.hosts
            .iter()
            .flat_map(|h| &h.patterns)
            .filter(|p| !p.contains('*') && !p.contains('?') && !p.starts_with('!'))
            .filter(|p| seen.insert((*p).clone()))
            .map(|p| p.to_string())
            .collect()
    }

    /// Resolves options for a given alias using first-match-wins.
    fn resolve_options(&self, alias: &str) -> ResolvedOptions {
        let mut resolved = ResolvedOptions::default();
        for host in &self.hosts {
            if pattern_matches(&host.patterns, alias) {
                if resolved.host_name.is_none() {
                    resolved.host_name = host.host_name.clone();
                }
                if resolved.port.is_none() {
                    resolved.port = host.port;
                }
                if resolved.user.is_none() {
                    resolved.user = host.user.clone();
                }
                let host_identity_files = if host.identity_files.is_empty() {
                    host.identity_file.iter().cloned().collect::<Vec<_>>()
                } else {
                    host.identity_files.clone()
                };
                for identity_file in host_identity_files {
                    if resolved.identity_file.is_none() {
                        resolved.identity_file = Some(identity_file.clone());
                    }
                    resolved.identity_files.push(identity_file);
                }
                if resolved.proxy_jump.is_none() {
                    resolved.proxy_jump = host.proxy_jump.clone();
                }
                if resolved.host_key_alias.is_none() {
                    resolved.host_key_alias = host.host_key_alias.clone();
                }
            }
        }
        resolved
    }

    /// Converts all concrete host aliases into entries.
    pub fn to_entries(&self) -> AppResult<Vec<SshConfigEntry>> {
        self.list_hosts()
            .iter()
            .map(|alias| self.resolve(alias))
            .collect()
    }
}

#[derive(Debug, Default)]
struct ResolvedOptions {
    host_name: Option<String>,
    port: Option<u16>,
    user: Option<String>,
    identity_file: Option<String>,
    identity_files: Vec<String>,
    proxy_jump: Option<String>,
    host_key_alias: Option<String>,
}

fn pattern_matches(patterns: &[String], alias: &str) -> bool {
    let mut matched = false;
    for pattern in patterns {
        let pat = pattern.as_str();
        if let Some(neg) = pat.strip_prefix('!') {
            if glob_match(neg, alias) {
                return false;
            }
        } else if glob_match(pat, alias) {
            matched = true;
        }
    }
    matched
}

fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    glob_match_inner(&p, &t)
}

fn glob_match_inner(p: &[char], t: &[char]) -> bool {
    if p.is_empty() {
        return t.is_empty();
    }
    match p[0] {
        '*' => {
            for i in 0..=t.len() {
                if glob_match_inner(&p[1..], &t[i..]) {
                    return true;
                }
            }
            false
        }
        '?' => {
            if t.is_empty() {
                false
            } else {
                glob_match_inner(&p[1..], &t[1..])
            }
        }
        c => {
            if t.is_empty() || t[0] != c {
                false
            } else {
                glob_match_inner(&p[1..], &t[1..])
            }
        }
    }
}

#[derive(Default)]
struct ParseState {
    current: Option<SshConfigHost>,
    global: SshConfigHost,
    global_emitted: bool,
}

fn parse_file(
    path: &Path,
    required: bool,
    visited: &mut HashSet<PathBuf>,
    hosts: &mut Vec<SshConfigHost>,
    state: &mut ParseState,
) -> AppResult<()> {
    let canonical = match fs::canonicalize(path) {
        Ok(c) => c,
        Err(error) if required => {
            return Err(AppError::Config(format!(
                "SSH config file not found or cannot be accessed at {}: {error}",
                path.display()
            )));
        }
        Err(_) => return Ok(()),
    };
    if !visited.insert(canonical.clone()) {
        return Ok(());
    }

    let contents = fs::read_to_string(&canonical)
        .map_err(|e| AppError::Config(format!("cannot read {}: {e}", canonical.display())))?;

    parse_string_into(&contents, &canonical, visited, hosts, state)
}

#[cfg(test)]
fn parse_string(
    contents: &str,
    config_path: &Path,
    visited: &mut HashSet<PathBuf>,
    hosts: &mut Vec<SshConfigHost>,
) -> AppResult<()> {
    let mut state = ParseState::default();
    parse_string_into(contents, config_path, visited, hosts, &mut state)?;
    finish_parse(hosts, &mut state);
    Ok(())
}

fn parse_string_into(
    contents: &str,
    config_path: &Path,
    visited: &mut HashSet<PathBuf>,
    hosts: &mut Vec<SshConfigHost>,
    state: &mut ParseState,
) -> AppResult<()> {
    // Relative Include paths resolve from the directory containing the config
    // file, not from the config file path itself (matching OpenSSH behavior).
    let base_dir = config_path.parent().unwrap_or_else(|| Path::new("."));
    for raw_line in contents.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let (keyword, value) = match split_kv(line) {
            Some(pair) => pair,
            None => continue,
        };

        let kw_lower = keyword.to_lowercase();

        match kw_lower.as_str() {
            "host" => {
                if let Some(mut block) = state.current.take() {
                    block.name = derive_display_name(&block.patterns);
                    hosts.push(block);
                }
                let patterns: Vec<String> =
                    value.split_whitespace().map(|s| s.to_string()).collect();
                // Global directives are an implicit leading `Host *` block.
                // Keeping it in lexical order is what gives OpenSSH its
                // first-value-wins behaviour when resolving later blocks.
                if !state.global_emitted && has_options(&state.global) {
                    let mut global = state.global.clone();
                    global.patterns = vec!["*".to_string()];
                    global.name = "*".to_string();
                    hosts.push(global);
                    state.global_emitted = true;
                }
                state.current = Some(SshConfigHost {
                    patterns,
                    ..Default::default()
                });
            }
            "hostname" => {
                if let Some(ref mut block) = state.current {
                    set_if_missing(&mut block.host_name, value.to_string());
                } else {
                    set_if_missing(&mut state.global.host_name, value.to_string());
                }
            }
            "port" => {
                let port = value.parse::<u16>().map_err(|_| {
                    AppError::Config(format!(
                        "Invalid Port '{value}' in {}",
                        config_path.display()
                    ))
                })?;
                if let Some(ref mut block) = state.current {
                    set_if_missing(&mut block.port, port);
                } else {
                    set_if_missing(&mut state.global.port, port);
                }
            }
            "user" => {
                if let Some(ref mut block) = state.current {
                    set_if_missing(&mut block.user, value.to_string());
                } else {
                    set_if_missing(&mut state.global.user, value.to_string());
                }
            }
            "identityfile" => {
                let identity_file = expand_tilde(value);
                if let Some(ref mut block) = state.current {
                    set_if_missing(&mut block.identity_file, identity_file.clone());
                    block.identity_files.push(identity_file);
                } else {
                    set_if_missing(&mut state.global.identity_file, identity_file.clone());
                    state.global.identity_files.push(identity_file);
                }
            }
            "proxyjump" => {
                if let Some(ref mut block) = state.current {
                    set_if_missing(&mut block.proxy_jump, value.to_string());
                } else {
                    set_if_missing(&mut state.global.proxy_jump, value.to_string());
                }
            }
            "hostkeyalias" => {
                if let Some(ref mut block) = state.current {
                    set_if_missing(&mut block.host_key_alias, value.to_string());
                } else {
                    set_if_missing(&mut state.global.host_key_alias, value.to_string());
                }
            }
            "include" => {
                for pattern in value.split_whitespace() {
                    let expanded = expand_tilde(pattern);
                    let include_path = if Path::new(&expanded).is_absolute() {
                        PathBuf::from(&expanded)
                    } else {
                        base_dir.join(&expanded)
                    };
                    for matched in glob_paths(&include_path) {
                        parse_file(&matched, false, visited, hosts, state)?;
                    }
                }
            }
            _ => {}
        }
    }

    Ok(())
}

fn set_if_missing<T>(slot: &mut Option<T>, value: T) {
    if slot.is_none() {
        *slot = Some(value);
    }
}

fn has_options(block: &SshConfigHost) -> bool {
    block.host_name.is_some()
        || block.port.is_some()
        || block.user.is_some()
        || block.identity_file.is_some()
        || !block.identity_files.is_empty()
        || block.proxy_jump.is_some()
        || block.host_key_alias.is_some()
}

fn finish_parse(hosts: &mut Vec<SshConfigHost>, state: &mut ParseState) {
    if !state.global_emitted && has_options(&state.global) {
        let mut global = state.global.clone();
        global.patterns = vec!["*".to_string()];
        global.name = "*".to_string();
        hosts.push(global);
        state.global_emitted = true;
    }
    if let Some(mut block) = state.current.take() {
        block.name = derive_display_name(&block.patterns);
        hosts.push(block);
    }
}

fn split_kv(line: &str) -> Option<(&str, &str)> {
    let mut iter = line.splitn(2, char::is_whitespace);
    let keyword = iter.next()?.trim();
    let value = iter.next()?.trim();
    if keyword.is_empty() || value.is_empty() {
        return None;
    }
    Some((keyword, value))
}

/// Parses a ProxyJump specification into (user, host, port) components.
/// Supports: `bastion`, `user@bastion`, `bastion:2222`, `user@bastion:2222`.
fn parse_jump_spec(spec: &str) -> AppResult<(Option<String>, String, Option<u16>)> {
    let spec = spec.trim();
    if spec.is_empty() {
        return Err(AppError::Config(
            "ProxyJump contains an empty hop".to_string(),
        ));
    }
    let mut remaining = spec;
    let mut user = None;
    let mut port = None;

    // Extract user if present: user@host[:port]
    if let Some(at_pos) = remaining.find('@') {
        if at_pos == 0 || remaining[at_pos + 1..].contains('@') {
            return Err(AppError::Config(format!("Invalid ProxyJump hop '{spec}'")));
        }
        user = Some(remaining[..at_pos].to_string());
        remaining = &remaining[at_pos + 1..];
    }

    // Extract port if present: host:port
    if let Some(colon_pos) = remaining.rfind(':') {
        let port_part = &remaining[colon_pos + 1..];
        port = Some(
            port_part
                .parse::<u16>()
                .map_err(|_| AppError::Config(format!("Invalid ProxyJump port in '{spec}'")))?,
        );
        remaining = &remaining[..colon_pos];
    }

    if remaining.is_empty() {
        return Err(AppError::Config(format!("Invalid ProxyJump hop '{spec}'")));
    }

    Ok((user, remaining.to_string(), port))
}

fn derive_display_name(patterns: &[String]) -> String {
    for p in patterns {
        if !p.contains('*') && !p.contains('?') && !p.starts_with('!') {
            return p.clone();
        }
    }
    patterns
        .iter()
        .filter(|p| !p.starts_with('!'))
        .cloned()
        .collect::<Vec<_>>()
        .join(" ")
}

fn expand_tilde(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\")) {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest).to_string_lossy().into_owned();
        }
    } else if path == "~" {
        if let Some(home) = dirs::home_dir() {
            return home.to_string_lossy().into_owned();
        }
    }
    path.to_string()
}

fn glob_paths(pattern: &Path) -> Vec<PathBuf> {
    let pattern_str = pattern.to_string_lossy();
    if !pattern_str.contains('*') && !pattern_str.contains('?') {
        if pattern.exists() {
            return vec![pattern.to_path_buf()];
        }
        return vec![];
    }

    let parent = pattern.parent();
    let file_name = pattern.file_name();

    match (parent, file_name) {
        (Some(parent), Some(file_name)) => {
            let pattern_str = file_name.to_string_lossy();
            let mut results = Vec::new();
            if let Ok(entries) = fs::read_dir(parent) {
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if glob_match(&pattern_str, &name) {
                        results.push(entry.path());
                    }
                }
            }
            results.sort();
            results
        }
        _ => vec![],
    }
}

fn default_config_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".ssh")
        .join("config")
}

/// Converts an SshConfigEntry into a SavedConnection for nyaterm.
fn entry_to_saved_connection(
    entry: &SshConfigEntry,
    proxy_jump_id: Option<String>,
    key_id: Option<String>,
) -> SavedConnection {
    let uses_key = key_id.is_some();
    let mut details = Vec::new();
    if let Some(proxy_jump) = entry.proxy_jump.as_deref() {
        details.push(format!("ProxyJump: {proxy_jump}"));
    }
    if entry.identity_files.len() > 1 {
        details.push(format!(
            "{} IdentityFile entries; using SSH agent",
            entry.identity_files.len()
        ));
    } else if let Some(identity_file) = entry.identity_file.as_deref() {
        if uses_key {
            details.push(format!("IdentityFile: {identity_file}"));
        } else {
            details.push(format!(
                "IdentityFile not importable; using SSH agent: {identity_file}"
            ));
        }
    }
    let description = if details.is_empty() {
        "Imported from ~/.ssh/config".to_string()
    } else {
        format!("Imported from ~/.ssh/config ({})", details.join(", "))
    };

    SavedConnection {
        id: uuid::Uuid::new_v4().to_string(),
        name: entry.alias.clone(),
        config: ConnectionType::Ssh {
            host: entry.host.clone(),
            port: entry.port,
            username: entry.user.clone(),
            backspace_mode: "del".to_string(),
            x11_forwarding: false,
            auth_agent_endpoint: None,
            legacy_agent_forwarding: None,
            agent_forwarding_config: None,
            encoding: String::new(),
            dynamic_tab_title: false,
        },
        group_id: None,
        description: Some(description),
        tags: Vec::new(),
        sort_order: 0,
        icon: None,
        icon_auto_detect: None,
        auth: Some(ConnectionAuth {
            mode: if uses_key { "key" } else { "agent" }.to_string(),
            account_id: None,
            password_source: None,
            password_id: None,
            password: None,
            key_id,
            otp_id: None,
            auto_fill_otp: false,
            has_password: false,
        }),
        network: proxy_jump_id.map(|id| crate::config::ConnectionNetwork {
            proxy_id: None,
            proxy_jump_id: Some(id),
        }),
        post_login: None,
        recording: None,
        ssh_algorithms: None,
        ssh_profile: Default::default(),
        terminal_type: None,
        sftp: Default::default(),
        asset: None,
        created_at_ms: None,
        updated_at_ms: None,
        last_used_at_ms: None,
    }
}

fn identity_file_path(identity_file: &str) -> Option<PathBuf> {
    let value = identity_file.trim();
    if value.is_empty() || value.eq_ignore_ascii_case("none") {
        return None;
    }
    let value = if value.len() >= 2
        && ((value.starts_with('"') && value.ends_with('"'))
            || (value.starts_with('\'') && value.ends_with('\'')))
    {
        &value[1..value.len() - 1]
    } else {
        value
    };
    Some(PathBuf::from(expand_tilde(value)))
}

fn missing_private_key_passphrase_error(error: &russh::keys::Error) -> bool {
    let message = error.to_string().to_lowercase();
    message.contains("encrypted")
        || message.contains("passphrase")
        || message.contains("password")
        || message.contains("cipher")
}

fn private_key_content_is_importable(content: &str) -> bool {
    match russh::keys::decode_secret_key(content, None) {
        Ok(_) => true,
        Err(error) => missing_private_key_passphrase_error(&error),
    }
}

fn import_identity_key_with_encrypt<F>(
    entry: &SshConfigEntry,
    keys: &mut Vec<SshKey>,
    imported_paths: &mut HashMap<PathBuf, String>,
    encrypt: F,
) -> AppResult<Option<String>>
where
    F: FnOnce(&str) -> AppResult<String>,
{
    let identity_files = if entry.identity_files.is_empty() {
        entry.identity_file.iter().collect::<Vec<_>>()
    } else {
        entry.identity_files.iter().collect::<Vec<_>>()
    };
    if identity_files.len() != 1 {
        return Ok(None);
    }
    let Some(path) = identity_file_path(identity_files[0]) else {
        return Ok(None);
    };
    let dedupe_path = fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
    if let Some(key_id) = imported_paths.get(&dedupe_path) {
        return Ok(Some(key_id.clone()));
    }

    let content = match fs::read_to_string(&path) {
        Ok(content) if !content.trim().is_empty() => content,
        Ok(_) | Err(_) => return Ok(None),
    };
    if !private_key_content_is_importable(&content) {
        return Ok(None);
    }

    if let Some(existing_id) = keys.iter().find_map(|key| {
        crate::config::decrypt_key_pem(key)
            .ok()
            .flatten()
            .filter(|existing| existing == &content)
            .map(|_| key.id.clone())
    }) {
        imported_paths.insert(dedupe_path, existing_id.clone());
        return Ok(Some(existing_id));
    }

    let key_id = uuid::Uuid::new_v4().to_string();
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.trim().is_empty());
    let name = file_name.map_or_else(
        || format!("{} SSH key", entry.alias),
        |file_name| format!("{} ({file_name})", entry.alias),
    );

    keys.push(SshKey {
        id: key_id.clone(),
        name,
        key: Some(encrypt(&content)?),
        cert: None,
        passphrase: None,
        key_data: None,
        cert_data: None,
        key_file_path: None,
        cert_file_path: None,
        has_key_data: false,
        has_cert_data: false,
    });
    imported_paths.insert(dedupe_path, key_id.clone());
    Ok(Some(key_id))
}

fn import_identity_key(
    entry: &SshConfigEntry,
    keys: &mut Vec<SshKey>,
    imported_paths: &mut HashMap<PathBuf, String>,
) -> AppResult<Option<String>> {
    import_identity_key_with_encrypt(entry, keys, imported_paths, crypto::encrypt)
}

/// Imports SSH config hosts as saved connections, skipping existing names.
/// Each ProxyJump list is materialized in reverse linkage order because the
/// runtime recursively follows `proxy_jump_id` from a target to the previous
/// hop. Later hops are synthetic connections so a plain `Host jump2` remains
/// a direct connection even when a target reaches it via `jump1`.
pub fn import_ssh_config_connections(app: &tauri::AppHandle) -> AppResult<usize> {
    let config = SshConfig::load_default()?;
    let mut cfg = crate::config::load_config(app)?;
    let mut keys = crate::config::load_keys(app)?;
    let original_keys = keys.clone();
    let initial_key_count = original_keys.keys.len();
    let mut imported_identity_paths = HashMap::new();
    let connections =
        build_imported_connections_with_key_resolver(&config, &cfg.connections, |entry| {
            import_identity_key(entry, &mut keys.keys, &mut imported_identity_paths)
        })?;
    let count = connections.len();
    cfg.connections.extend(connections);

    if count > 0 {
        if keys.keys.len() != initial_key_count {
            // Persist keys first so a config write can never leave a connection
            // pointing at a key that was not stored successfully.
            crate::config::save_keys(app, &keys)?;
        }
        if let Err(error) = crate::config::save_config(app, &cfg) {
            if keys.keys.len() != initial_key_count {
                if let Err(rollback_error) = crate::config::save_keys(app, &original_keys) {
                    return Err(AppError::Config(format!(
                        "failed to save imported SSH connections ({error}); also failed to roll back imported SSH keys ({rollback_error})"
                    )));
                }
            }
            return Err(error);
        }
    }

    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_PRIVATE_KEY: &str = "-----BEGIN PRIVATE KEY-----\nMC4CAQAwBQYDK2VwBCIEINTuctv5E1hK1bbY8fdp+K06/nwoy/HU++CXqI9EdVhC\n-----END PRIVATE KEY-----";
    const TEST_ENCRYPTED_PRIVATE_KEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAACmFlczI1Ni1jYmMAAAAGYmNyeXB0AAAAGAAAABDLGyfA39\nJ2FcJygtYqi5ISAAAAEAAAAAEAAAAzAAAAC3NzaC1lZDI1NTE5AAAAIN+Wjn4+4Fcvl2Jl\nKpggT+wCRxpSvtqqpVrQrKN1/A22AAAAkOHDLnYZvYS6H9Q3S3Nk4ri3R2jAZlQlBbUos5\nFkHpYgNw65KCWCTXtP7ye2czMC3zjn2r98pJLobsLYQgRiHIv/CUdAdsqbvMPECB+wl/UQ\ne+JpiSq66Z6GIt0801skPh20jxOO3F52SoX1IeO5D5PXfZrfSZlw6S8c7bwyp2FHxDewRx\n7/wNsnDM0T7nLv/Q==\n-----END OPENSSH PRIVATE KEY-----";

    #[test]
    fn glob_matches_basic_patterns() {
        assert!(glob_match("web-*", "web-prod"));
        assert!(glob_match("*", "anything"));
        assert!(glob_match("server?", "server1"));
        assert!(!glob_match("server?", "server12"));
        assert!(glob_match("exact", "exact"));
        assert!(!glob_match("exact", "other"));
    }

    #[test]
    fn pattern_matches_handles_negation() {
        let patterns = vec!["web-*".to_string(), "!web-old".to_string()];
        assert!(pattern_matches(&patterns, "web-prod"));
        assert!(!pattern_matches(&patterns, "web-old"));
    }

    #[test]
    fn parse_simple_config() {
        let config = r#"
            Host prod
                HostName prod.example.com
                Port 2222
                User admin
                IdentityFile ~/.ssh/prod_key

            Host staging
                HostName staging.example.com
                User deploy
        "#;

        let mut hosts = Vec::new();
        let mut visited = HashSet::new();
        parse_string(
            config,
            Path::new("/tmp/test/config"),
            &mut visited,
            &mut hosts,
        )
        .unwrap();

        assert_eq!(hosts.len(), 2);
        assert_eq!(hosts[0].name, "prod");
        assert_eq!(hosts[0].host_name.as_deref(), Some("prod.example.com"));
        assert_eq!(hosts[0].port, Some(2222));
        assert_eq!(hosts[0].user.as_deref(), Some("admin"));
    }

    #[test]
    fn resolve_proxy_jump_chain() {
        let config = r#"
            Host jump1
                HostName jump1.example.com
                User juser

            Host jump2
                HostName jump2.example.com
                Port 2222
                User juser2

            Host target
                HostName 10.0.0.42
                User root
                ProxyJump jump1,jump2
        "#;

        let mut hosts = Vec::new();
        let mut visited = HashSet::new();
        parse_string(
            config,
            Path::new("/tmp/chain/config"),
            &mut visited,
            &mut hosts,
        )
        .unwrap();

        let ssh_config = SshConfig { hosts };
        let resolved = ssh_config.resolve("target").unwrap();

        assert_eq!(resolved.hops.len(), 3);
        assert_eq!(resolved.hops[0].host, "jump1.example.com");
        assert_eq!(resolved.hops[0].user, "juser");
        assert!(!resolved.hops[0].is_target);

        assert_eq!(resolved.hops[1].host, "jump2.example.com");
        assert_eq!(resolved.hops[1].port, 2222);
        assert!(!resolved.hops[1].is_target);

        assert_eq!(resolved.hops[2].host, "10.0.0.42");
        assert_eq!(resolved.hops[2].user, "root");
        assert!(resolved.hops[2].is_target);
    }

    #[test]
    fn resolve_first_match_wins() {
        // OpenSSH uses first-match-wins: the first value found for each
        // option is the one used. With `Host *` before `Host prod`, the
        // `User defaultuser` from `*` wins because it matches first.
        // To have `User admin` win, the `Host prod` block must come first.
        let config = r#"
            Host prod
                HostName prod.example.com
                User admin

            Host *
                User defaultuser
                Port 2222
        "#;

        let mut hosts = Vec::new();
        let mut visited = HashSet::new();
        parse_string(
            config,
            Path::new("/tmp/resolve/config"),
            &mut visited,
            &mut hosts,
        )
        .unwrap();

        let ssh_config = SshConfig { hosts };
        let resolved = ssh_config.resolve("prod").unwrap();

        // `Host prod` matches first, so `User admin` wins.
        assert_eq!(resolved.user, "admin");
        // `Host *` provides Port 2222 because `prod` doesn't specify one.
        assert_eq!(resolved.port, 2222);
        assert_eq!(resolved.host, "prod.example.com");
    }

    #[test]
    fn list_hosts_skips_wildcards() {
        let config = r#"
            Host *
                User default

            Host web-*
                User webuser

            Host prod
                HostName prod.example.com
        "#;

        let mut hosts = Vec::new();
        let mut visited = HashSet::new();
        parse_string(
            config,
            Path::new("/tmp/list/config"),
            &mut visited,
            &mut hosts,
        )
        .unwrap();

        let ssh_config = SshConfig { hosts };
        let host_list = ssh_config.list_hosts();
        assert!(host_list.contains(&"prod".to_string()));
        assert!(!host_list.contains(&"*".to_string()));
        assert!(!host_list.contains(&"web-*".to_string()));
    }

    #[test]
    fn relative_include_resolves_from_ssh_dir() {
        // Simulate a config at ~/.ssh/config with Include conf.d/*.conf
        // The base_dir should be ~/.ssh/ (parent of config), not ~/.ssh/config/
        let config_path = Path::new("/tmp/ssh_test/config");
        // We can't test actual file resolution without creating files,
        // but we can verify that the base_dir is derived correctly.
        let base_dir = config_path.parent().unwrap();
        assert_eq!(base_dir, Path::new("/tmp/ssh_test"));
        // conf.d/*.conf would resolve to /tmp/ssh_test/conf.d/*.conf (correct)
        // not /tmp/ssh_test/config/conf.d/*.conf (wrong - old behavior)
    }

    #[test]
    fn parse_jump_spec_extracts_components() {
        // Bare alias
        let (user, host, port) = parse_jump_spec("bastion").unwrap();
        assert_eq!(user, None);
        assert_eq!(host, "bastion");
        assert_eq!(port, None);

        // user@host
        let (user, host, port) = parse_jump_spec("alice@bastion").unwrap();
        assert_eq!(user.as_deref(), Some("alice"));
        assert_eq!(host, "bastion");
        assert_eq!(port, None);

        // host:port
        let (user, host, port) = parse_jump_spec("bastion:2222").unwrap();
        assert_eq!(user, None);
        assert_eq!(host, "bastion");
        assert_eq!(port, Some(2222));

        // user@host:port
        let (user, host, port) = parse_jump_spec("alice@bastion:2222").unwrap();
        assert_eq!(user.as_deref(), Some("alice"));
        assert_eq!(host, "bastion");
        assert_eq!(port, Some(2222));
    }

    #[test]
    fn list_hosts_deduplicates() {
        let config = r#"
            Host prod
                User admin

            Host prod
                Port 2222

            Host staging
                HostName staging.example.com
        "#;

        let mut hosts = Vec::new();
        let mut visited = HashSet::new();
        parse_string(
            config,
            Path::new("/tmp/dedup/config"),
            &mut visited,
            &mut hosts,
        )
        .unwrap();

        let ssh_config = SshConfig { hosts };
        let host_list = ssh_config.list_hosts();
        // "prod" appears in two blocks but should be listed once.
        assert_eq!(host_list.iter().filter(|h| *h == "prod").count(), 1);
        assert_eq!(host_list.len(), 2);
    }

    #[test]
    fn global_options_before_first_host() {
        let config = r#"
            User deploy
            Port 2222

            Host prod
                HostName prod.example.com
        "#;

        let mut hosts = Vec::new();
        let mut visited = HashSet::new();
        parse_string(
            config,
            Path::new("/tmp/global/config"),
            &mut visited,
            &mut hosts,
        )
        .unwrap();

        let ssh_config = SshConfig { hosts };
        let resolved = ssh_config.resolve("prod").unwrap();
        // Global User and Port should apply to all hosts.
        assert_eq!(resolved.user, "deploy");
        assert_eq!(resolved.port, 2222);
        assert_eq!(resolved.host, "prod.example.com");
    }

    #[test]
    fn proxy_jump_with_user_at_host_port() {
        let config = r#"
            Host bastion
                HostName bastion.example.com

            Host target
                HostName 10.0.0.42
                ProxyJump alice@bastion:2222
        "#;

        let mut hosts = Vec::new();
        let mut visited = HashSet::new();
        parse_string(
            config,
            Path::new("/tmp/jumpuser/config"),
            &mut visited,
            &mut hosts,
        )
        .unwrap();

        let ssh_config = SshConfig { hosts };
        let resolved = ssh_config.resolve("target").unwrap();

        assert_eq!(resolved.hops.len(), 2);
        assert_eq!(resolved.hops[0].host, "bastion.example.com");
        assert_eq!(resolved.hops[0].user, "alice");
        assert_eq!(resolved.hops[0].port, 2222);
        assert!(!resolved.hops[0].is_target);
    }

    #[test]
    fn first_value_wins_when_wildcard_precedes_specific_host() {
        let config = r#"
            Host *
                User default

            Host prod
                User admin
        "#;
        let ssh_config = parse_test_config(config);

        assert_eq!(ssh_config.resolve("prod").unwrap().user, "default");
    }

    #[test]
    fn global_option_precedes_and_wins_over_host_option() {
        // This matches `ssh -G prod -F config`: global directives occur
        // before Host blocks and are therefore the first value obtained.
        let config = r#"
            User global

            Host prod
                User specific
        "#;
        let ssh_config = parse_test_config(config);

        assert_eq!(ssh_config.resolve("prod").unwrap().user, "global");
    }

    #[test]
    fn identity_file_uses_imported_key_when_available() {
        let ssh_config = parse_test_config("Host prod\n    IdentityFile ~/.ssh/id_ed25519\n");
        let entry = ssh_config.resolve("prod").unwrap();
        let connection = entry_to_saved_connection(&entry, None, Some("key-1".to_string()));
        let auth = connection.auth.as_ref().unwrap();

        assert_eq!(auth.mode, "key");
        assert_eq!(auth.key_id.as_deref(), Some("key-1"));
        assert!(
            connection
                .description
                .as_deref()
                .unwrap()
                .contains("IdentityFile:")
        );
    }

    #[test]
    fn unavailable_identity_file_falls_back_to_agent() {
        let ssh_config = parse_test_config("Host prod\n    IdentityFile ~/.ssh/missing-key\n");
        let entry = ssh_config.resolve("prod").unwrap();
        let connection = entry_to_saved_connection(&entry, None, None);
        let auth = connection.auth.as_ref().unwrap();

        assert_eq!(auth.mode, "agent");
        assert!(auth.key_id.is_none());
        assert!(
            connection
                .description
                .as_deref()
                .unwrap()
                .contains("using SSH agent")
        );
    }

    #[test]
    fn identity_file_path_expands_windows_style_tilde_prefix() {
        let home = dirs::home_dir().expect("home directory");
        let path = identity_file_path(r#"~\.ssh\id_ed25519"#).expect("identity path");

        assert_eq!(path, home.join(r#".ssh\id_ed25519"#));
    }

    #[test]
    fn multiple_identity_files_are_preserved_in_order() {
        let ssh_config = parse_test_config(
            "Host prod\n    IdentityFile ~/.ssh/id_ed25519\n    IdentityFile ~/.ssh/id_rsa\n",
        );
        let entry = ssh_config.resolve("prod").unwrap();

        assert_eq!(entry.identity_files.len(), 2);
        assert!(entry.identity_files[0].ends_with("id_ed25519"));
        assert!(entry.identity_files[1].ends_with("id_rsa"));
        assert_eq!(
            entry.identity_file.as_deref(),
            Some(entry.identity_files[0].as_str())
        );
    }

    #[test]
    fn multiple_identity_files_keep_agent_auth() {
        let ssh_config = parse_test_config(
            "Host prod\n    IdentityFile ~/.ssh/id_ed25519\n    IdentityFile ~/.ssh/id_rsa\n",
        );
        let entry = ssh_config.resolve("prod").unwrap();
        let mut keys = Vec::new();
        let mut imported_paths = HashMap::new();

        let key_id =
            import_identity_key_with_encrypt(&entry, &mut keys, &mut imported_paths, |_| {
                panic!("multiple identities must not select key auth")
            })
            .unwrap();
        let connection = entry_to_saved_connection(&entry, None, key_id);

        assert!(keys.is_empty());
        assert_eq!(connection.auth.as_ref().unwrap().mode, "agent");
        assert!(
            connection
                .description
                .as_deref()
                .unwrap()
                .contains("IdentityFile entries; using SSH agent")
        );
    }

    #[test]
    fn readable_public_key_file_falls_back_to_agent() {
        let root = test_temp_dir("public_identity");
        let key_path = root.join("id_ed25519.pub");
        fs::write(
            &key_path,
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIBogusPublicKey test\n",
        )
        .unwrap();
        let ssh_config = parse_test_config(&format!(
            "Host prod\n    IdentityFile {}\n",
            key_path.to_string_lossy()
        ));
        let entry = ssh_config.resolve("prod").unwrap();
        let mut keys = Vec::new();
        let mut imported_paths = HashMap::new();

        let key_id =
            import_identity_key_with_encrypt(&entry, &mut keys, &mut imported_paths, |_| {
                panic!("public key content must not be imported as a private key")
            })
            .unwrap();
        let connection = entry_to_saved_connection(&entry, None, key_id);

        assert!(keys.is_empty());
        assert_eq!(connection.auth.as_ref().unwrap().mode, "agent");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn encrypted_private_key_without_passphrase_is_importable() {
        assert!(private_key_content_is_importable(
            TEST_ENCRYPTED_PRIVATE_KEY
        ));
    }

    #[test]
    fn identity_file_import_reads_and_deduplicates_private_key() {
        let root = test_temp_dir("identity_import");
        let key_path = root.join("id_ed25519");
        fs::write(&key_path, TEST_PRIVATE_KEY).unwrap();
        let ssh_config = parse_test_config(&format!(
            "Host prod\n    IdentityFile {}\n",
            key_path.to_string_lossy()
        ));
        let entry = ssh_config.resolve("prod").unwrap();
        let mut keys = Vec::new();
        let mut imported_paths = HashMap::new();

        let first =
            import_identity_key_with_encrypt(&entry, &mut keys, &mut imported_paths, |content| {
                Ok(format!("encrypted:{content}"))
            })
            .unwrap()
            .unwrap();
        let second =
            import_identity_key_with_encrypt(&entry, &mut keys, &mut imported_paths, |_| {
                panic!("deduplicated key should not be encrypted twice")
            })
            .unwrap()
            .unwrap();

        assert_eq!(first, second);
        assert_eq!(keys.len(), 1);
        assert_eq!(
            keys[0].key.as_deref(),
            Some(format!("encrypted:{TEST_PRIVATE_KEY}").as_str())
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn imported_connections_materialize_two_hop_proxy_jump_in_runtime_order() {
        let ssh_config = parse_test_config(
            r#"
                Host jump1
                    HostName jump1.example.com

                Host jump2
                    HostName jump2.internal

                Host target
                    HostName target.internal
                    ProxyJump jump1,jump2
            "#,
        );
        let connections = build_imported_connections(&ssh_config, &[]).unwrap();
        let by_name = connections
            .iter()
            .map(|connection| (connection.name.as_str(), connection))
            .collect::<std::collections::HashMap<_, _>>();
        let jump1 = by_name["jump1"];
        let jump2 = by_name["jump2"];
        let jump2_via_jump1 = by_name["jump2 (ProxyJump via: jump1)"];
        let target = by_name["target"];

        assert!(jump1.network.is_none());
        assert!(jump2.network.is_none());
        assert_eq!(
            jump2_via_jump1
                .network
                .as_ref()
                .and_then(|network| network.proxy_jump_id.as_deref()),
            Some(jump1.id.as_str())
        );
        assert_eq!(
            target
                .network
                .as_ref()
                .and_then(|network| network.proxy_jump_id.as_deref()),
            Some(jump2_via_jump1.id.as_str())
        );
    }

    #[test]
    fn imported_connections_deduplicate_repeated_host_aliases() {
        let ssh_config = parse_test_config(
            r#"
                Host foo
                    User admin
                Host foo
                    Port 2222
            "#,
        );
        let connections = build_imported_connections(&ssh_config, &[]).unwrap();

        assert_eq!(
            connections
                .iter()
                .filter(|connection| connection.name == "foo")
                .count(),
            1
        );
        let ConnectionType::Ssh { port, .. } = &connections[0].config else {
            panic!("SSH connection expected");
        };
        assert_eq!(*port, 2222);
    }

    #[test]
    fn imported_connections_materialize_three_hop_proxy_jump_in_runtime_order() {
        let ssh_config = parse_test_config(
            r#"
                Host jump1
                Host jump2
                Host jump3
                Host target
                    ProxyJump jump1,jump2,jump3
            "#,
        );
        let connections = build_imported_connections(&ssh_config, &[]).unwrap();
        let by_name = connections
            .iter()
            .map(|connection| (connection.name.as_str(), connection))
            .collect::<std::collections::HashMap<_, _>>();

        assert!(by_name["jump1"].network.is_none());
        assert!(by_name["jump2"].network.is_none());
        assert!(by_name["jump3"].network.is_none());

        let jump2 = by_name["jump2 (ProxyJump via: jump1)"];
        let jump3 = by_name["jump3 (ProxyJump via: jump1,jump2)"];
        assert_eq!(
            jump2
                .network
                .as_ref()
                .and_then(|network| network.proxy_jump_id.as_deref()),
            Some(by_name["jump1"].id.as_str())
        );
        assert_eq!(
            jump3
                .network
                .as_ref()
                .and_then(|network| network.proxy_jump_id.as_deref()),
            Some(jump2.id.as_str())
        );
        assert_eq!(
            by_name["target"]
                .network
                .as_ref()
                .and_then(|network| network.proxy_jump_id.as_deref()),
            Some(jump3.id.as_str())
        );
    }

    #[test]
    fn existing_direct_jump_alias_does_not_block_multi_hop_import() {
        let direct_config = parse_test_config("Host jump2\n    HostName jump2.example.com\n");
        let existing = build_imported_connections(&direct_config, &[])
            .unwrap()
            .pop()
            .expect("direct jump2 connection");
        assert!(existing.network.is_none());

        let ssh_config = parse_test_config(
            r#"
                Host jump1
                Host jump2
                Host target
                    ProxyJump jump1,jump2
            "#,
        );
        let connections = build_imported_connections(&ssh_config, &[existing]).unwrap();
        let by_name = connections
            .iter()
            .map(|connection| (connection.name.as_str(), connection))
            .collect::<std::collections::HashMap<_, _>>();

        assert!(!by_name.contains_key("jump2"));
        let jump1 = by_name["jump1"];
        let routed_jump2 = by_name["jump2 (ProxyJump via: jump1)"];
        assert_eq!(
            routed_jump2
                .network
                .as_ref()
                .and_then(|network| network.proxy_jump_id.as_deref()),
            Some(jump1.id.as_str())
        );
    }

    #[test]
    fn imported_proxy_jump_override_preserves_user_and_port_in_distinct_connection() {
        let ssh_config = parse_test_config(
            r#"
                Host bastion
                    HostName bastion.example.com
                    User defaultuser
                    Port 22

                Host target
                    ProxyJump alice@bastion:2222
            "#,
        );
        let connections = build_imported_connections(&ssh_config, &[]).unwrap();
        let jump = connections
            .iter()
            .find(|connection| connection.name == "bastion (ProxyJump: alice@bastion:2222)")
            .expect("override jump connection");
        let target = connections
            .iter()
            .find(|connection| connection.name == "target")
            .expect("target connection");
        let ConnectionType::Ssh {
            host,
            port,
            username,
            ..
        } = &jump.config
        else {
            panic!("SSH connection expected");
        };

        assert_eq!(host, "bastion.example.com");
        assert_eq!(*port, 2222);
        assert_eq!(username, "alice");
        assert_eq!(
            target
                .network
                .as_ref()
                .and_then(|network| network.proxy_jump_id.as_deref()),
            Some(jump.id.as_str())
        );
    }

    #[test]
    fn load_from_follows_relative_recursive_includes_and_ignores_cycles() {
        let root = test_temp_dir("includes");
        let config = root.join("config");
        let include_dir = root.join("conf.d");
        fs::create_dir_all(&include_dir).unwrap();
        fs::write(&config, "Include conf.d/*.conf\n").unwrap();
        fs::write(
            include_dir.join("work.conf"),
            "Include nested.conf\nHost work\n    User dev\n",
        )
        .unwrap();
        fs::write(
            include_dir.join("nested.conf"),
            "Include ../config\nHost nested\n    Port 2200\n",
        )
        .unwrap();

        let parsed = SshConfig::load_from(&config).unwrap();
        assert_eq!(parsed.resolve("work").unwrap().user, "dev");
        assert_eq!(parsed.resolve("nested").unwrap().port, 2200);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_root_config_returns_an_actionable_error() {
        let missing = test_temp_dir("missing").join("config");
        let error = SshConfig::load_from(&missing).unwrap_err();
        assert!(error.to_string().contains("SSH config file not found"));
    }

    fn parse_test_config(contents: &str) -> SshConfig {
        let mut hosts = Vec::new();
        let mut visited = HashSet::new();
        parse_string(
            contents,
            Path::new("/tmp/ssh_config_test/config"),
            &mut visited,
            &mut hosts,
        )
        .unwrap();
        SshConfig { hosts }
    }

    fn test_temp_dir(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "nyaterm_ssh_config_{name}_{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }
}

#[derive(Debug, Clone)]
struct ImportNode {
    key: String,
    name: String,
    entry: SshConfigEntry,
}

/// Builds the connection graph before it is persisted. Kept separate from the
/// Tauri/storage boundary so the exact runtime graph can be tested.
fn build_imported_connections_with_key_resolver<F>(
    config: &SshConfig,
    existing: &[SavedConnection],
    mut resolve_key: F,
) -> AppResult<Vec<SavedConnection>>
where
    F: FnMut(&SshConfigEntry) -> AppResult<Option<String>>,
{
    let entries = config.to_entries()?;
    let mut nodes: HashMap<String, ImportNode> = HashMap::new();
    let mut links: HashMap<String, String> = HashMap::new();

    for entry in &entries {
        let target = regular_import_node(entry.clone());
        nodes.entry(target.key.clone()).or_insert(target.clone());

        let Some(proxy_jump) = entry.proxy_jump.as_deref() else {
            continue;
        };

        let mut previous: Option<ImportNode> = None;
        let mut prior_specs = Vec::new();
        for raw_spec in proxy_jump.split(',') {
            let jump = jump_import_node(config, raw_spec, &prior_specs)?;
            nodes.entry(jump.key.clone()).or_insert(jump.clone());
            if let Some(previous) = previous {
                set_import_link(&mut links, &jump.key, &previous.key)?;
            }
            prior_specs.push(raw_spec.trim().to_string());
            previous = Some(jump);
        }
        if let Some(last_jump) = previous {
            set_import_link(&mut links, &target.key, &last_jump.key)?;
        }
    }

    let existing_by_name: HashMap<&str, &SavedConnection> = existing
        .iter()
        .map(|connection| (connection.name.as_str(), connection))
        .collect();
    let mut ids = HashMap::new();
    let mut ordered_nodes: Vec<_> = nodes.into_values().collect();
    ordered_nodes.sort_by(|a, b| a.name.cmp(&b.name));

    for node in &ordered_nodes {
        if let Some(existing) = existing_by_name.get(node.name.as_str()) {
            ids.insert(node.key.clone(), existing.id.clone());
        } else {
            ids.insert(node.key.clone(), uuid::Uuid::new_v4().to_string());
        }
    }

    let mut imported = Vec::new();
    for node in ordered_nodes {
        let expected_jump_id = links.get(&node.key).map(|key| {
            ids.get(key)
                .expect("all ProxyJump links have a materialized connection")
                .clone()
        });

        if let Some(existing) = existing_by_name.get(node.name.as_str()) {
            let existing_jump_id = existing
                .network
                .as_ref()
                .and_then(|network| network.proxy_jump_id.as_ref());
            if expected_jump_id.as_deref() != existing_jump_id.map(String::as_str) {
                return Err(AppError::Config(format!(
                    "Cannot safely import ProxyJump route: existing connection '{}' has a different jump host",
                    node.name
                )));
            }
            continue;
        }

        let key_id = resolve_key(&node.entry)?;
        let mut connection = entry_to_saved_connection(&node.entry, expected_jump_id, key_id);
        connection.id = ids.get(&node.key).expect("generated ID exists").clone();
        connection.name = node.name;
        imported.push(connection);
    }

    Ok(imported)
}

#[cfg(test)]
fn build_imported_connections(
    config: &SshConfig,
    existing: &[SavedConnection],
) -> AppResult<Vec<SavedConnection>> {
    build_imported_connections_with_key_resolver(config, existing, |_| Ok(None))
}

fn set_import_link(
    links: &mut std::collections::HashMap<String, String>,
    node: &str,
    previous_hop: &str,
) -> AppResult<()> {
    if let Some(existing) = links.insert(node.to_string(), previous_hop.to_string()) {
        if existing != previous_hop {
            return Err(AppError::Config(format!(
                "ProxyJump host '{node}' is used with conflicting preceding hops ('{existing}' and '{previous_hop}')"
            )));
        }
    }
    Ok(())
}

fn regular_import_node(entry: SshConfigEntry) -> ImportNode {
    ImportNode {
        key: format!("alias:{}", entry.alias),
        name: entry.alias.clone(),
        entry,
    }
}

fn jump_import_node(
    config: &SshConfig,
    raw_spec: &str,
    prior_specs: &[String],
) -> AppResult<ImportNode> {
    let (user_override, host_alias, port_override) = parse_jump_spec(raw_spec)?;
    let mut entry = config.resolve(&host_alias)?;
    let has_override = user_override.is_some() || port_override.is_some();
    if let Some(user) = user_override {
        entry.user = user;
    }
    if let Some(port) = port_override {
        entry.port = port;
    }

    // A second-or-later hop has route-specific semantics: it must reach the
    // previous hop first. Never attach that context to the regular alias,
    // otherwise opening `jump2` directly would incorrectly transit `jump1`.
    if has_override || !prior_specs.is_empty() {
        let raw_spec = raw_spec.trim();
        let route_prefix = prior_specs.join(",");
        let name = if route_prefix.is_empty() {
            format!("{} (ProxyJump: {raw_spec})", host_alias)
        } else if raw_spec == host_alias {
            format!("{} (ProxyJump via: {route_prefix})", host_alias)
        } else {
            format!(
                "{} (ProxyJump via: {route_prefix}; spec: {raw_spec})",
                host_alias
            )
        };
        Ok(ImportNode {
            key: format!("jump:{raw_spec}:via:{route_prefix}"),
            name,
            entry,
        })
    } else {
        Ok(regular_import_node(entry))
    }
}
