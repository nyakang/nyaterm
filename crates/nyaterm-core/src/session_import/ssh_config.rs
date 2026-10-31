use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use ssh_key::PrivateKey;

use super::{
    AppError, AppResult, PreparedSessionConnection, PreparedSessionImport, read_file_limited,
};
use crate::{
    AiExecutionProfile, ConnectionAuth, ConnectionNetwork, ConnectionType, SavedConnection,
    SecretString, SshKey,
};

const MAX_CONFIG_FILES: usize = 256;
const MAX_IMPORT_BYTES: usize = 64 * 1024 * 1024;
const MAX_ROUTE_NODES: usize = 4096;
const MAX_RECURSION_DEPTH: usize = 64;

#[derive(Clone, Default)]
struct HostBlock {
    patterns: Vec<String>,
    host_name: Option<String>,
    port: Option<u16>,
    user: Option<String>,
    identity_files: Vec<String>,
    proxy_jump: Option<String>,
    host_key_alias: Option<String>,
}

#[derive(Default)]
struct ParseState {
    blocks: Vec<HostBlock>,
    current: HostBlock,
    saw_host: bool,
    files_read: usize,
    bytes_read: usize,
}

#[derive(Clone)]
struct ResolvedHost {
    alias: String,
    host: String,
    port: u16,
    user: String,
    identity_files: Vec<String>,
    proxy_jump: Option<String>,
    host_key_alias: Option<String>,
}

pub(super) fn prepare_ssh_config_import(path: &Path) -> AppResult<PreparedSessionImport> {
    let config = ParsedConfig::load(path)?;
    let mut builder = ImportBuilder::new(&config);
    builder.materialize_all()?;
    Ok(PreparedSessionImport {
        warnings: Vec::new(),
        custom_icons: Vec::new(),
        groups: Vec::new(),
        passwords: Vec::new(),
        ssh_keys: builder.keys,
        connections: builder.connections,
    })
}

struct ParsedConfig {
    blocks: Vec<HostBlock>,
}

impl ParsedConfig {
    fn load(path: &Path) -> AppResult<Self> {
        let mut state = ParseState::default();
        parse_file(path, true, &mut HashSet::new(), &mut state)?;
        finish_block(&mut state);
        Ok(Self {
            blocks: state.blocks,
        })
    }

    fn aliases(&self) -> Vec<String> {
        let mut seen = HashSet::new();
        self.blocks
            .iter()
            .flat_map(|block| &block.patterns)
            .filter(|pattern| !pattern.starts_with('!') && !pattern.contains(['*', '?']))
            .filter(|pattern| seen.insert((*pattern).clone()))
            .cloned()
            .collect()
    }

    fn resolve(&self, alias: &str) -> ResolvedHost {
        let mut host_name = None;
        let mut port = None;
        let mut user = None;
        let mut proxy_jump = None;
        let mut host_key_alias = None;
        let mut identity_files = Vec::new();
        for block in &self.blocks {
            if !patterns_match(&block.patterns, alias) {
                continue;
            }
            set_first(&mut host_name, block.host_name.clone());
            set_first(&mut port, block.port);
            set_first(&mut user, block.user.clone());
            set_first(&mut proxy_jump, block.proxy_jump.clone());
            set_first(&mut host_key_alias, block.host_key_alias.clone());
            identity_files.extend(block.identity_files.iter().cloned());
        }
        let proxy_jump = proxy_jump.filter(|value| !value.eq_ignore_ascii_case("none"));
        ResolvedHost {
            alias: alias.to_string(),
            host: host_name.unwrap_or_else(|| alias.to_string()),
            port: port.unwrap_or(22),
            user: user.unwrap_or_else(current_username),
            identity_files,
            proxy_jump,
            host_key_alias,
        }
    }
}

fn set_first<T>(target: &mut Option<T>, value: Option<T>) {
    if target.is_none() {
        *target = value;
    }
}

fn parse_file(
    path: &Path,
    required: bool,
    visited: &mut HashSet<PathBuf>,
    state: &mut ParseState,
) -> AppResult<()> {
    let canonical = match fs::canonicalize(path) {
        Ok(path) => path,
        Err(error) if required => {
            return Err(AppError::Config(format!(
                "Cannot open SSH config {}: {error}",
                path.display()
            )));
        }
        Err(_) => return Ok(()),
    };
    if !visited.insert(canonical.clone()) {
        return Err(AppError::Config("SSH config Include cycle detected".into()));
    }
    if visited.len() > MAX_RECURSION_DEPTH || state.files_read >= MAX_CONFIG_FILES {
        return Err(AppError::Config("SSH config Include limit exceeded".into()));
    }
    let bytes = read_file_limited(&canonical, "SSH config", 16 * 1024 * 1024)?;
    state.files_read += 1;
    state.bytes_read = state.bytes_read.saturating_add(bytes.len());
    if state.bytes_read > MAX_IMPORT_BYTES {
        return Err(AppError::Config("SSH config size limit exceeded".into()));
    }
    let content = String::from_utf8(bytes)
        .map_err(|error| AppError::Config(format!("SSH config is not valid UTF-8: {error}")))?;
    let base = canonical.parent().unwrap_or_else(|| Path::new("."));
    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((keyword, values)) = parse_directive(line)? else {
            continue;
        };
        let value = values.join(" ");
        match keyword.to_ascii_lowercase().as_str() {
            "host" => {
                finish_block(state);
                state.saw_host = true;
                state.current = HostBlock {
                    patterns: values,
                    ..Default::default()
                };
            }
            "include" => {
                for pattern in values {
                    let expanded = expand_tilde(&pattern);
                    let path = if expanded.is_absolute() {
                        expanded
                    } else {
                        base.join(expanded)
                    };
                    for included in glob_paths(&path)? {
                        parse_file(&included, false, visited, state)?;
                    }
                }
            }
            "hostname" => set_first(&mut state.current.host_name, Some(value.to_string())),
            "port" => {
                let port = value.parse::<u16>().map_err(|_| {
                    AppError::Config(format!(
                        "Invalid SSH Port '{value}' in {}",
                        canonical.display()
                    ))
                })?;
                if port == 0 {
                    return Err(AppError::Config("SSH Port must not be zero".into()));
                }
                set_first(&mut state.current.port, Some(port));
            }
            "user" => set_first(&mut state.current.user, Some(value.to_string())),
            "identityfile" => state
                .current
                .identity_files
                .push(expand_tilde(&value).to_string_lossy().into_owned()),
            "proxyjump" => set_first(&mut state.current.proxy_jump, Some(value.to_string())),
            "hostkeyalias" => set_first(&mut state.current.host_key_alias, Some(value.to_string())),
            "match" => {
                return Err(AppError::Config(
                    "SSH config Match blocks are not supported for import".into(),
                ));
            }
            _ => {}
        }
    }
    visited.remove(&canonical);
    Ok(())
}

fn finish_block(state: &mut ParseState) {
    if state.current.patterns.is_empty() {
        if !state.saw_host && block_has_options(&state.current) {
            state.current.patterns.push("*".into());
        } else {
            return;
        }
    }
    state.blocks.push(std::mem::take(&mut state.current));
}

fn block_has_options(block: &HostBlock) -> bool {
    block.host_name.is_some()
        || block.port.is_some()
        || block.user.is_some()
        || !block.identity_files.is_empty()
        || block.proxy_jump.is_some()
        || block.host_key_alias.is_some()
}

fn parse_directive(line: &str) -> AppResult<Option<(String, Vec<String>)>> {
    let mut tokens = shell_tokens(line)?;
    if tokens.is_empty() {
        return Ok(None);
    }
    let first = tokens.remove(0);
    let (keyword, attached) = first
        .split_once('=')
        .map_or((first.as_str(), None), |(key, value)| (key, Some(value)));
    if keyword.is_empty() {
        return Ok(None);
    }
    if tokens.first().is_some_and(|token| token == "=") {
        tokens.remove(0);
    }
    if let Some(value) = attached.filter(|value| !value.is_empty()) {
        tokens.insert(0, value.to_string());
    }
    Ok((!tokens.is_empty()).then(|| (keyword.to_string(), tokens)))
}

fn shell_tokens(line: &str) -> AppResult<Vec<String>> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\' && quote != Some('\'') {
            match chars.peek().copied() {
                Some(next) if next.is_whitespace() || matches!(next, '\\' | '\'' | '"' | '#') => {
                    current.push(chars.next().expect("peeked character exists"));
                }
                _ => current.push(ch),
            }
            continue;
        }
        if let Some(active) = quote {
            if ch == active {
                quote = None;
            } else {
                current.push(ch);
            }
            continue;
        }
        match ch {
            '\'' | '"' => quote = Some(ch),
            '#' => break,
            value if value.is_whitespace() => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(ch),
        }
    }
    if quote.is_some() {
        return Err(AppError::Config(
            "SSH config contains an unterminated quote".into(),
        ));
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    Ok(tokens)
}

fn patterns_match(patterns: &[String], alias: &str) -> bool {
    let mut matched = false;
    for pattern in patterns {
        if let Some(negative) = pattern.strip_prefix('!') {
            if glob_match(negative, alias) {
                return false;
            }
        } else if glob_match(pattern, alias) {
            matched = true;
        }
    }
    matched
}

fn glob_match(pattern: &str, text: &str) -> bool {
    let pattern = pattern.chars().collect::<Vec<_>>();
    let text = text.chars().collect::<Vec<_>>();
    let (mut p, mut t, mut star, mut retry) = (0, 0, None, 0);
    while t < text.len() {
        if p < pattern.len() && (pattern[p] == '?' || pattern[p] == text[t]) {
            p += 1;
            t += 1;
        } else if p < pattern.len() && pattern[p] == '*' {
            star = Some(p);
            p += 1;
            retry = t;
        } else if let Some(index) = star {
            retry += 1;
            t = retry;
            p = index + 1;
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == '*' {
        p += 1;
    }
    p == pattern.len()
}

fn glob_paths(pattern: &Path) -> AppResult<Vec<PathBuf>> {
    let mut paths = vec![PathBuf::new()];
    for component in pattern.components() {
        let name = component.as_os_str().to_string_lossy();
        if matches!(component, std::path::Component::Normal(_)) && name.contains(['*', '?', '[']) {
            let matcher = glob::Pattern::new(&name)
                .map_err(|_| AppError::Config("Invalid SSH Include glob pattern".into()))?;
            let options = glob::MatchOptions {
                case_sensitive: !cfg!(windows),
                require_literal_separator: true,
                require_literal_leading_dot: true,
            };
            let mut expanded = Vec::new();
            for base in paths {
                let directory = if base.as_os_str().is_empty() {
                    Path::new(".")
                } else {
                    &base
                };
                for entry in fs::read_dir(directory)
                    .ok()
                    .into_iter()
                    .flatten()
                    .filter_map(Result::ok)
                {
                    if matcher.matches_with(&entry.file_name().to_string_lossy(), options) {
                        expanded.push(entry.path());
                        if expanded.len() > MAX_CONFIG_FILES {
                            return Err(AppError::Config(
                                "SSH config Include glob limit exceeded".into(),
                            ));
                        }
                    }
                }
            }
            paths = expanded;
        } else {
            for path in &mut paths {
                path.push(component.as_os_str());
            }
        }
    }
    paths.retain(|path| path.is_file());
    paths.sort();
    Ok(paths)
}

fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\"))
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest);
    }
    PathBuf::from(path)
}

fn current_username() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "root".into())
}

fn parse_jump(spec: &str) -> AppResult<(Option<String>, String, Option<u16>)> {
    let (user, host_port) = spec
        .rsplit_once('@')
        .map_or((None, spec), |(user, rest)| (Some(user.to_string()), rest));
    if host_port.is_empty() {
        return Err(AppError::Config(format!("Invalid ProxyJump hop '{spec}'")));
    }
    if let Some(rest) = host_port.strip_prefix('[') {
        let (host, suffix) = rest
            .split_once(']')
            .ok_or_else(|| AppError::Config(format!("Invalid ProxyJump hop '{spec}'")))?;
        let port = suffix
            .strip_prefix(':')
            .map(|value| value.parse::<u16>())
            .transpose()
            .map_err(|_| AppError::Config(format!("Invalid ProxyJump port in '{spec}'")))?;
        if host.is_empty() || (!suffix.is_empty() && !suffix.starts_with(':')) || port == Some(0) {
            return Err(AppError::Config(format!("Invalid ProxyJump hop '{spec}'")));
        }
        return Ok((user, host.to_string(), port));
    }
    let (host, port) = match host_port.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') => (
            host,
            Some(
                port.parse()
                    .map_err(|_| AppError::Config(format!("Invalid ProxyJump port in '{spec}'")))?,
            ),
        ),
        _ => (host_port, None),
    };
    if host.is_empty() || port == Some(0) {
        return Err(AppError::Config(format!("Invalid ProxyJump hop '{spec}'")));
    }
    Ok((user, host.to_string(), port))
}

#[derive(Clone)]
struct ImportNode {
    key: String,
    name: String,
    entry: ResolvedHost,
}

struct ImportBuilder<'a> {
    config: &'a ParsedConfig,
    keys: Vec<SshKey>,
    key_ids: HashMap<PathBuf, String>,
    connections: Vec<PreparedSessionConnection>,
    key_bytes_read: usize,
}

impl<'a> ImportBuilder<'a> {
    fn new(config: &'a ParsedConfig) -> Self {
        Self {
            config,
            keys: Vec::new(),
            key_ids: HashMap::new(),
            connections: Vec::new(),
            key_bytes_read: 0,
        }
    }

    fn materialize_all(&mut self) -> AppResult<()> {
        let mut nodes = HashMap::<String, ImportNode>::new();
        let mut links = HashMap::<String, String>::new();
        let mut active = HashSet::new();
        let mut completed = HashSet::new();
        for alias in self.config.aliases() {
            let entry = self.config.resolve(&alias);
            let target = regular_node(entry.clone());
            materialize_route(
                self.config,
                target,
                None,
                &mut nodes,
                &mut links,
                &mut active,
                &mut completed,
            )?;
        }
        ensure_acyclic(&links)?;

        let mut ordered = nodes.into_values().collect::<Vec<_>>();
        ordered.sort_by(|left, right| {
            route_depth(&links, &left.key)
                .cmp(&route_depth(&links, &right.key))
                .then(left.name.cmp(&right.name))
        });
        let ids = ordered
            .iter()
            .map(|node| (node.key.clone(), uuid::Uuid::new_v4().to_string()))
            .collect::<HashMap<_, _>>();
        for node in ordered {
            let proxy_jump_id = links.get(&node.key).and_then(|key| ids.get(key)).cloned();
            let key_id = self.import_identity(&node.entry)?;
            let auth = ConnectionAuth {
                mode: if key_id.is_some() { "key" } else { "agent" }.into(),
                key_id,
                ..ConnectionAuth::default()
            };
            let network =
                (proxy_jump_id.is_some() || node.entry.host_key_alias.is_some()).then(|| {
                    ConnectionNetwork {
                        proxy_id: None,
                        proxy_jump_id,
                        host_key_alias: node.entry.host_key_alias.clone(),
                    }
                });
            let id = ids[&node.key].clone();
            let saved = saved_connection(id, node.name.clone(), &node.entry, auth.clone(), network);
            self.connections.push(PreparedSessionConnection {
                saved: Some(saved),
                name: node.name,
                config: ssh_type(&node.entry),
                group_path: None,
                description: None,
                sort_order: 0,
                icon: None,
                auth: Some(auth),
            });
        }
        Ok(())
    }

    fn import_identity(&mut self, entry: &ResolvedHost) -> AppResult<Option<String>> {
        let paths = entry
            .identity_files
            .iter()
            .map(|path| fs::canonicalize(path).unwrap_or_else(|_| PathBuf::from(path)))
            .collect::<HashSet<_>>();
        let mut ids = Vec::new();
        let mut ordered = paths.iter().collect::<Vec<_>>();
        ordered.sort();
        for path in ordered {
            if let Some(id) = self.import_identity_file(path)? {
                ids.push(id);
            }
        }
        Ok((paths.len() == 1 && ids.len() == 1).then(|| ids.remove(0)))
    }

    fn import_identity_file(&mut self, path: &Path) -> AppResult<Option<String>> {
        if let Some(id) = self.key_ids.get(path) {
            return Ok(Some(id.clone()));
        }
        let Ok(bytes) = read_file_limited(path, "SSH IdentityFile", 16 * 1024 * 1024) else {
            return Ok(None);
        };
        self.key_bytes_read = self.key_bytes_read.saturating_add(bytes.len());
        if self.key_bytes_read > MAX_IMPORT_BYTES {
            return Err(AppError::Config(
                "SSH IdentityFile size limit exceeded".into(),
            ));
        }
        let content = match String::from_utf8(bytes).ok() {
            Some(content) if PrivateKey::from_openssh(&content).is_ok() => content,
            _ => return Ok(None),
        };
        let id = uuid::Uuid::new_v4().to_string();
        self.key_ids.insert(path.to_path_buf(), id.clone());
        self.keys.push(SshKey {
            sort_order: 0,
            id: id.clone(),
            name: path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            key: Some(SecretString::from(content)),
            cert: None,
            passphrase: None,
            key_file_path: None,
            cert_file_path: None,
            has_key_data: false,
            has_cert_data: false,
        });
        Ok(Some(id))
    }
}

fn materialize_route(
    config: &ParsedConfig,
    node: ImportNode,
    preceding: Option<&str>,
    nodes: &mut HashMap<String, ImportNode>,
    links: &mut HashMap<String, String>,
    active: &mut HashSet<String>,
    completed: &mut HashSet<String>,
) -> AppResult<()> {
    if nodes.len() >= MAX_ROUTE_NODES || active.len() >= MAX_RECURSION_DEPTH {
        return Err(AppError::Config(
            "SSH ProxyJump route limit exceeded".into(),
        ));
    }
    nodes.entry(node.key.clone()).or_insert(node.clone());
    if let Some(preceding) = preceding {
        set_link(links, &node.key, preceding)?;
        completed.insert(node.key);
        return Ok(());
    }
    if completed.contains(&node.key) {
        return Ok(());
    }
    if !active.insert(node.key.clone()) {
        return Err(AppError::Config("SSH ProxyJump cycle detected".into()));
    }
    if let Some(route) = node.entry.proxy_jump.as_deref() {
        let mut previous: Option<String> = None;
        let mut prior_specs = Vec::new();
        for raw_spec in route.split(',').map(str::trim) {
            if prior_specs.len() >= MAX_RECURSION_DEPTH {
                return Err(AppError::Config(
                    "SSH ProxyJump route limit exceeded".into(),
                ));
            }
            if raw_spec.is_empty() {
                return Err(AppError::Config("ProxyJump contains an empty hop".into()));
            }
            let jump = jump_node(config, raw_spec, &prior_specs)?;
            materialize_route(
                config,
                jump.clone(),
                previous.as_deref(),
                nodes,
                links,
                active,
                completed,
            )?;
            prior_specs.push(raw_spec.to_string());
            previous = Some(jump.key);
        }
        if let Some(previous) = previous {
            set_link(links, &node.key, &previous)?;
        }
    }
    active.remove(&node.key);
    completed.insert(node.key);
    Ok(())
}

fn route_depth(links: &HashMap<String, String>, start: &str) -> usize {
    let mut depth = 0;
    let mut current = start;
    while let Some(next) = links.get(current) {
        depth += 1;
        current = next;
    }
    depth
}

fn regular_node(entry: ResolvedHost) -> ImportNode {
    ImportNode {
        key: format!("alias:{}", entry.alias),
        name: entry.alias.clone(),
        entry,
    }
}

fn jump_node(
    config: &ParsedConfig,
    raw_spec: &str,
    prior_specs: &[String],
) -> AppResult<ImportNode> {
    let (user, alias, port) = parse_jump(raw_spec)?;
    let mut entry = config.resolve(&alias);
    let has_override = user.is_some() || port.is_some();
    if let Some(user) = user {
        entry.user = user;
    }
    if let Some(port) = port {
        entry.port = port;
    }
    if !has_override && prior_specs.is_empty() {
        return Ok(regular_node(entry));
    }
    let route = prior_specs.join(",");
    let name = if route.is_empty() {
        format!("{alias} (ProxyJump: {raw_spec})")
    } else if raw_spec == alias {
        format!("{alias} (ProxyJump via: {route})")
    } else {
        format!("{alias} (ProxyJump via: {route}; spec: {raw_spec})")
    };
    Ok(ImportNode {
        key: format!("jump:{raw_spec}:via:{route}"),
        name,
        entry,
    })
}

fn set_link(links: &mut HashMap<String, String>, node: &str, previous: &str) -> AppResult<()> {
    if let Some(existing) = links.insert(node.to_string(), previous.to_string())
        && existing != previous
    {
        return Err(AppError::Config(format!(
            "ProxyJump host '{node}' has conflicting routes"
        )));
    }
    Ok(())
}

fn ensure_acyclic(links: &HashMap<String, String>) -> AppResult<()> {
    for start in links.keys() {
        let mut seen = HashSet::new();
        let mut current = start.as_str();
        while let Some(next) = links.get(current) {
            if !seen.insert(current.to_string()) {
                return Err(AppError::Config(format!(
                    "ProxyJump cycle contains '{start}'"
                )));
            }
            current = next;
        }
    }
    Ok(())
}

fn ssh_type(entry: &ResolvedHost) -> ConnectionType {
    ConnectionType::Ssh {
        host: entry.host.clone(),
        port: entry.port,
        username: entry.user.clone(),
        backspace_mode: "del".into(),
        ai_execution_profile: AiExecutionProfile::Auto,
        x11_forwarding: false,
        auth_agent_endpoint: None,
        agent_forwarding_config: None,
        legacy_agent_forwarding: None,
        encoding: String::new(),
        dynamic_tab_title: false,
    }
}

fn saved_connection(
    id: String,
    name: String,
    entry: &ResolvedHost,
    auth: ConnectionAuth,
    network: Option<ConnectionNetwork>,
) -> SavedConnection {
    SavedConnection {
        id,
        name,
        config: ssh_type(entry),
        group_id: None,
        description: None,
        sort_order: 0,
        icon: None,
        icon_auto_detect: None,
        auth: Some(auth),
        network,
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
        extensions: Default::default(),
        tags: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::prepare_ssh_config_import;
    use crate::ConnectionType;
    use std::fs;

    const PRIVATE_KEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----\n\
b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW\n\
QyNTUxOQAAACCzPq7zfqLffKoBDe/eo04kH2XxtSmk9D7RQyf1xUqrYgAAAJgAIAxdACAM\n\
XQAAAAtzc2gtZWQyNTUxOQAAACCzPq7zfqLffKoBDe/eo04kH2XxtSmk9D7RQyf1xUqrYg\n\
AAAEC2BsIi0QwW2uFscKTUUXNHLsYX4FxlaSDSblbAj7WR7bM+rvN+ot98qgEN796jTiQf\n\
ZfG1KaT0PtFDJ/XFSqtiAAAAEHVzZXJAZXhhbXBsZS5jb20BAgMEBQ==\n\
-----END OPENSSH PRIVATE KEY-----";
    const ENCRYPTED_KEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----\n\
b3BlbnNzaC1rZXktdjEAAAAACmFlczI1Ni1jYmMAAAAGYmNyeXB0AAAAGAAAABDLGyfA39\n\
J2FcJygtYqi5ISAAAAEAAAAAEAAAAzAAAAC3NzaC1lZDI1NTE5AAAAIN+Wjn4+4Fcvl2Jl\n\
KpggT+wCRxpSvtqqpVrQrKN1/A22AAAAkOHDLnYZvYS6H9Q3S3Nk4ri3R2jAZlQlBbUos5\n\
FkHpYgNw65KCWCTXtP7ye2czMC3zjn2r98pJLobsLYQgRiHIv/CUdAdsqbvMPECB+wl/UQ\n\
e+JpiSq66Z6GIt0801skPh20jxOO3F52SoX1IeO5D5PXfZrfSZlw6S8c7bwyp2FHxDewRx\n\
7/wNsnDM0T7nLv/Q==\n\
-----END OPENSSH PRIVATE KEY-----";

    struct Temp(crate::test_support::TestTempDir);
    impl Temp {
        fn new() -> Self {
            let path = crate::test_support::TestTempDir::new("nyaterm-ssh-config");
            fs::create_dir(path.path()).unwrap();
            Self(path)
        }
    }

    #[test]
    fn includes_first_value_patterns_alias_and_multi_hop_are_prepared() {
        let temp = Temp::new();
        fs::create_dir(temp.0.join("conf.d")).unwrap();
        fs::write(temp.0.join("config"), "Include conf.d/*.conf\nHost *\n User global\nHost prod !prod-bad\n User ignored\n HostName prod.example\n HostKeyAlias stable\n ProxyJump jump1,jump2\n").unwrap();
        fs::write(
            temp.0.join("conf.d/jumps.conf"),
            "Host jump1\n HostName one\nHost jump2\n HostName two\n",
        )
        .unwrap();
        let prepared = prepare_ssh_config_import(&temp.0.join("config")).unwrap();
        let prod = prepared
            .connections
            .iter()
            .find_map(|entry| entry.saved.as_ref().filter(|saved| saved.name == "prod"))
            .unwrap();
        assert_eq!(
            prod.network
                .as_ref()
                .and_then(|network| network.host_key_alias.as_deref()),
            Some("stable")
        );
        assert!(
            prod.network
                .as_ref()
                .and_then(|network| network.proxy_jump_id.as_ref())
                .is_some()
        );
        let crate::ConnectionType::Ssh { username, .. } = &prod.config else {
            panic!()
        };
        assert_eq!(username, "global");
    }

    #[test]
    fn include_cycle_is_rejected_without_exposing_paths() {
        let temp = Temp::new();
        fs::write(temp.0.join("config"), "Include nested.conf\n").unwrap();
        fs::write(temp.0.join("nested.conf"), "Include config\n").unwrap();

        let error = prepare_ssh_config_import(&temp.0.join("config"))
            .unwrap_err()
            .to_string();
        assert_eq!(error, "SSH config Include cycle detected");
        assert!(!error.contains(temp.0.to_string_lossy().as_ref()));
    }

    #[test]
    fn proxy_jump_cycle_is_rejected() {
        let temp = Temp::new();
        fs::write(
            temp.0.join("config"),
            "Host a\n ProxyJump b\nHost b\n ProxyJump a\n",
        )
        .unwrap();
        assert!(
            prepare_ssh_config_import(&temp.0.join("config"))
                .unwrap_err()
                .to_string()
                .contains("cycle")
        );
    }

    #[test]
    fn repeated_aliases_negation_and_first_value_wins_are_resolved() {
        let temp = Temp::new();
        fs::write(
            temp.0.join("config"),
            "User global\nHost prod !prod-bad\n User ignored\nHost prod\n Port 2200\n User later\nHost prod-bad\n HostName blocked.example\n",
        )
        .unwrap();
        let prepared = prepare_ssh_config_import(&temp.0.join("config")).unwrap();
        assert_eq!(
            prepared
                .connections
                .iter()
                .filter(|entry| entry.name == "prod")
                .count(),
            1
        );
        let prod = prepared
            .connections
            .iter()
            .find(|entry| entry.name == "prod")
            .unwrap();
        let ConnectionType::Ssh { port, username, .. } = &prod.config else {
            panic!("SSH connection expected")
        };
        assert_eq!(*port, 2200);
        assert_eq!(username, "global");
    }

    #[test]
    fn proxy_jump_ipv6_overrides_and_three_hop_order_are_materialized() {
        let temp = Temp::new();
        fs::write(
            temp.0.join("config"),
            "Host jump1\n HostName one\nHost jump2\n HostName two\nHost jump3\n HostName three\nHost target\n ProxyJump jump1,alice@[2001:db8::2]:2202,jump3\n",
        )
        .unwrap();
        let prepared = prepare_ssh_config_import(&temp.0.join("config")).unwrap();
        let by_name = prepared
            .connections
            .iter()
            .filter_map(|entry| {
                entry
                    .saved
                    .as_ref()
                    .map(|saved| (saved.name.as_str(), saved))
            })
            .collect::<std::collections::HashMap<_, _>>();
        let jump1 = by_name["jump1"];
        let jump2 = by_name["2001:db8::2 (ProxyJump via: jump1; spec: alice@[2001:db8::2]:2202)"];
        let jump3 = by_name["jump3 (ProxyJump via: jump1,alice@[2001:db8::2]:2202)"];
        assert_eq!(
            jump2
                .network
                .as_ref()
                .and_then(|value| value.proxy_jump_id.as_deref()),
            Some(jump1.id.as_str())
        );
        assert_eq!(
            jump3
                .network
                .as_ref()
                .and_then(|value| value.proxy_jump_id.as_deref()),
            Some(jump2.id.as_str())
        );
        let ConnectionType::Ssh {
            host,
            port,
            username,
            ..
        } = &jump2.config
        else {
            panic!("SSH connection expected")
        };
        assert_eq!(host, "2001:db8::2");
        assert_eq!(*port, 2202);
        assert_eq!(username, "alice");
    }

    #[test]
    fn identity_files_import_one_key_dedupe_and_keep_multiple_on_agent() {
        let temp = Temp::new();
        let key_dir = temp.0.join("keys with spaces");
        fs::create_dir(&key_dir).unwrap();
        let plain = key_dir.join("plain key");
        let encrypted = key_dir.join("encrypted key");
        fs::write(&plain, PRIVATE_KEY).unwrap();
        fs::write(&encrypted, ENCRYPTED_KEY).unwrap();
        fs::write(
            temp.0.join("config"),
            format!(
                "Host one two\n IdentityFile \"{}\" # comment\nHost encrypted\n IdentityFile \"{}\"\nHost multiple\n IdentityFile \"{}\"\n IdentityFile \"{}\"\n",
                plain.display(),
                encrypted.display(),
                plain.display(),
                encrypted.display()
            ),
        )
        .unwrap();
        let prepared = prepare_ssh_config_import(&temp.0.join("config")).unwrap();
        assert_eq!(prepared.ssh_keys.len(), 2);
        let one_key = prepared
            .connections
            .iter()
            .find(|entry| entry.name == "one")
            .and_then(|entry| entry.auth.as_ref())
            .and_then(|auth| auth.key_id.as_ref())
            .unwrap();
        let two_key = prepared
            .connections
            .iter()
            .find(|entry| entry.name == "two")
            .and_then(|entry| entry.auth.as_ref())
            .and_then(|auth| auth.key_id.as_ref())
            .unwrap();
        assert_eq!(one_key, two_key);
        assert_eq!(
            prepared
                .connections
                .iter()
                .find(|entry| entry.name == "multiple")
                .and_then(|entry| entry.auth.as_ref())
                .map(|auth| auth.mode.as_str()),
            Some("agent")
        );
    }

    #[test]
    fn implicit_overridden_hops_expand_their_own_configured_route_before_targets() {
        let temp = Temp::new();
        fs::write(temp.0.join("config"), "Host target\n ProxyJump alice@hidden-hop\nHost hidden-*\n HostName gateway.invalid\n ProxyJump edge\nHost edge\n HostName edge.invalid\n").unwrap();
        let prepared = prepare_ssh_config_import(&temp.0.join("config")).unwrap();
        let edge = prepared
            .connections
            .iter()
            .position(|entry| entry.name == "edge")
            .unwrap();
        let hop = prepared
            .connections
            .iter()
            .position(|entry| entry.name.starts_with("hidden-hop ("))
            .unwrap();
        let target = prepared
            .connections
            .iter()
            .position(|entry| entry.name == "target")
            .unwrap();
        assert!(edge < hop && hop < target);
        let entries = prepared
            .connections
            .iter()
            .map(|entry| entry.saved.as_ref().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            entries[hop]
                .network
                .as_ref()
                .unwrap()
                .proxy_jump_id
                .as_deref(),
            Some(entries[edge].id.as_str())
        );
        assert_eq!(
            entries[target]
                .network
                .as_ref()
                .unwrap()
                .proxy_jump_id
                .as_deref(),
            Some(entries[hop].id.as_str())
        );
    }

    #[test]
    fn implicit_overridden_hop_cycles_are_rejected() {
        let temp = Temp::new();
        fs::write(
            temp.0.join("config"),
            "Host target\n ProxyJump alice@hidden-a\nHost hidden-*\n ProxyJump alice@hidden-a\n",
        )
        .unwrap();
        assert!(
            prepare_ssh_config_import(&temp.0.join("config"))
                .unwrap_err()
                .to_string()
                .contains("cycle")
        );
    }

    #[test]
    fn include_globs_expand_directory_components_and_replay_in_distinct_host_blocks() {
        let temp = Temp::new();
        fs::create_dir_all(temp.0.join("parts/a")).unwrap();
        fs::create_dir_all(temp.0.join("parts/b")).unwrap();
        fs::write(
            temp.0.join("parts/a/host.conf"),
            "Host a\n HostName a.invalid\n",
        )
        .unwrap();
        fs::write(
            temp.0.join("parts/b/host.conf"),
            "Host b\n HostName b.invalid\n",
        )
        .unwrap();
        fs::write(temp.0.join("options"), "User included\n").unwrap();
        fs::write(
            temp.0.join("config"),
            "Include parts/*/*.conf\nHost one\n Include options\nHost two\n Include options\n",
        )
        .unwrap();
        let prepared = prepare_ssh_config_import(&temp.0.join("config")).unwrap();
        assert_eq!(prepared.connections.len(), 4);
        for alias in ["one", "two"] {
            let entry = prepared
                .connections
                .iter()
                .find(|entry| entry.name == alias)
                .unwrap();
            let ConnectionType::Ssh { username, .. } = &entry.config else {
                panic!("SSH expected");
            };
            assert_eq!(username, "included");
        }
    }

    #[test]
    fn multi_identity_only_hosts_import_all_keys_without_changing_agent_fallback() {
        let temp = Temp::new();
        let plain = temp.0.join("plain");
        let encrypted = temp.0.join("encrypted");
        fs::write(&plain, PRIVATE_KEY).unwrap();
        fs::write(&encrypted, ENCRYPTED_KEY).unwrap();
        fs::write(temp.0.join("config"), format!("Host multiple\n IdentityFile \"{}\"\n IdentityFile \"{}\"\nHost duplicate\n IdentityFile \"{}\"\n IdentityFile \"{}\"\n", plain.display(), encrypted.display(), plain.display(), plain.display())).unwrap();
        let prepared = prepare_ssh_config_import(&temp.0.join("config")).unwrap();
        assert_eq!(prepared.ssh_keys.len(), 2);
        assert_eq!(
            prepared
                .connections
                .iter()
                .find(|entry| entry.name == "multiple")
                .unwrap()
                .auth
                .as_ref()
                .unwrap()
                .mode,
            "agent"
        );
        assert_eq!(
            prepared
                .connections
                .iter()
                .find(|entry| entry.name == "duplicate")
                .unwrap()
                .auth
                .as_ref()
                .unwrap()
                .mode,
            "key"
        );
    }

    #[test]
    fn excessive_include_depth_and_unsupported_match_conditions_fail_safely() {
        let temp = Temp::new();
        for index in 0..66 {
            fs::write(
                temp.0.join(format!("config{index}")),
                format!("Include config{}\n", index + 1),
            )
            .unwrap();
        }
        assert!(
            prepare_ssh_config_import(&temp.0.join("config0"))
                .unwrap_err()
                .to_string()
                .contains("limit")
        );
        fs::write(
            temp.0.join("config"),
            "Host one\nMatch exec secret-condition\n User privileged\n",
        )
        .unwrap();
        let error = prepare_ssh_config_import(&temp.0.join("config"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("Match"));
        assert!(!error.contains("secret-condition"));
    }

    #[test]
    fn unreadable_public_and_invalid_identity_files_fall_back_to_agent() {
        let temp = Temp::new();
        let public = temp.0.join("id.pub");
        let invalid = temp.0.join("invalid");
        fs::write(&public, "ssh-ed25519 AAAA test").unwrap();
        fs::write(&invalid, "not a private key").unwrap();
        fs::write(
            temp.0.join("config"),
            format!(
                "Host public\n IdentityFile \"{}\"\nHost invalid\n IdentityFile \"{}\"\nHost missing\n IdentityFile \"{}\"\n",
                public.display(), invalid.display(), temp.0.join("missing").display()
            ),
        )
        .unwrap();
        let prepared = prepare_ssh_config_import(&temp.0.join("config")).unwrap();
        assert!(prepared.ssh_keys.is_empty());
        assert!(prepared.connections.iter().all(|entry| {
            entry
                .auth
                .as_ref()
                .is_some_and(|auth| auth.mode == "agent" && auth.key_id.is_none())
        }));
    }
}
