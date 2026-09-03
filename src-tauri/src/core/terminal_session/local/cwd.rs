use crate::core::{CwdPresentation, SessionCwdReplacement};
use crate::core::ssh::osc::parse_legacy_osc7_payload;
use std::collections::HashSet;

const MAX_LOCAL_CWD_PAYLOAD_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CwdPresentationUpdate {
    Ignore,
    Clear,
    Set(CwdPresentation),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperationalCwdUpdate {
    Ignore,
    Clear,
    Set(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalCwdReport {
    /// Released projection consumed by existing cwd/file-explorer features.
    pub legacy_path: Option<String>,
    /// True when the complete OSC 7 payload fits the operational safety cap.
    payload_within_operational_limit: bool,
    /// True when a standard OSC 7 location resolves to a UNC path without a
    /// session-bound producer. Such paths remain presentation-only.
    unauthenticated_unc: bool,
    /// Strict projection used to validate operational cwd and dynamic-title presentation.
    pub presentation: CwdPresentationUpdate,
}

#[derive(Debug, Clone)]
pub struct LocalCwdContext {
    session_token: String,
    local_host_aliases: HashSet<String>,
    home_paths: Vec<String>,
    windows_host: bool,
}

impl LocalCwdContext {
    pub fn new(session_token: &str) -> Self {
        Self {
            session_token: session_token.to_string(),
            local_host_aliases: collect_local_host_aliases(),
            home_paths: collect_home_paths(),
            windows_host: cfg!(target_os = "windows"),
        }
    }

    #[cfg(test)]
    fn for_test(
        session_token: &str,
        aliases: &[&str],
        home_paths: &[&str],
        windows_host: bool,
    ) -> Self {
        Self {
            session_token: session_token.to_string(),
            local_host_aliases: aliases
                .iter()
                .map(|value| normalize_host(value))
                .collect(),
            home_paths: home_paths.iter().map(|value| value.to_string()).collect(),
            windows_host,
        }
    }
}

pub fn operational_cwd_update(
    report: &LocalCwdReport,
    managed_integration: bool,
) -> OperationalCwdUpdate {
    if !report.payload_within_operational_limit {
        return OperationalCwdUpdate::Clear;
    }

    if managed_integration {
        match &report.presentation {
            CwdPresentationUpdate::Set(presentation) => {
                return presentation
                    .operational_path
                    .clone()
                    .map(OperationalCwdUpdate::Set)
                    .unwrap_or(OperationalCwdUpdate::Clear);
            }
            CwdPresentationUpdate::Clear | CwdPresentationUpdate::Ignore => {
                return OperationalCwdUpdate::Clear;
            }
        }
    }

    match &report.presentation {
        CwdPresentationUpdate::Clear => return OperationalCwdUpdate::Clear,
        CwdPresentationUpdate::Set(presentation)
            if presentation.copy_as_uri || report.unauthenticated_unc =>
        {
            return OperationalCwdUpdate::Clear;
        }
        CwdPresentationUpdate::Set(_) | CwdPresentationUpdate::Ignore => {}
    }

    match report.legacy_path.as_ref() {
        Some(path)
            if path.len() <= MAX_LOCAL_CWD_PAYLOAD_BYTES
                && !path.chars().any(is_forbidden_path_scalar) =>
        {
            OperationalCwdUpdate::Set(path.clone())
        }
        Some(_) => OperationalCwdUpdate::Clear,
        None => OperationalCwdUpdate::Ignore,
    }
}

pub fn cwd_state_replacement(
    report: &LocalCwdReport,
    managed_integration: bool,
) -> SessionCwdReplacement {
    let operational_path = match operational_cwd_update(report, managed_integration) {
        OperationalCwdUpdate::Set(path) => Some(path),
        OperationalCwdUpdate::Ignore | OperationalCwdUpdate::Clear => None,
    };
    let presentation = match &report.presentation {
        CwdPresentationUpdate::Set(presentation) => Some(presentation.clone()),
        // Invalid strict data must clear the previous strict snapshot so an
        // operational fallback cannot be paired with stale presentation.
        CwdPresentationUpdate::Ignore | CwdPresentationUpdate::Clear => None,
    };
    SessionCwdReplacement {
        legacy_path: report.legacy_path.clone(),
        operational_path,
        presentation,
    }
}

pub fn parse_local_cwd_report(payload: &str, context: &LocalCwdContext) -> LocalCwdReport {
    let legacy_path = parse_legacy_osc7_payload(payload);
    let payload_within_operational_limit = payload.len() <= MAX_LOCAL_CWD_PAYLOAD_BYTES;
    let unauthenticated_unc = standard_uri_is_windows_unc(payload, context);
    let presentation = if !payload_within_operational_limit {
        CwdPresentationUpdate::Ignore
    } else if payload == format!("nyaterm-clear://{}", context.session_token) {
        CwdPresentationUpdate::Clear
    } else if let Some(raw_path) = payload.strip_prefix(&format!(
        "nyaterm-cmd://{}/",
        context.session_token
    )) {
        parse_cmd_path(raw_path, context)
            .map(CwdPresentationUpdate::Set)
            .unwrap_or(CwdPresentationUpdate::Ignore)
    } else {
        parse_standard_cwd_uri(payload, context)
            .map(CwdPresentationUpdate::Set)
            .unwrap_or(CwdPresentationUpdate::Ignore)
    };

    LocalCwdReport {
        legacy_path,
        payload_within_operational_limit,
        unauthenticated_unc,
        presentation,
    }
}

fn standard_uri_is_windows_unc(payload: &str, context: &LocalCwdContext) -> bool {
    if !context.windows_host {
        return false;
    }
    let Some(after_scheme) = payload
        .strip_prefix("file://")
        .or_else(|| payload.strip_prefix("kitty-shell-cwd://"))
    else {
        return false;
    };
    if after_scheme.starts_with('/') {
        return after_scheme.starts_with("//")
            || percent_decode_strict(after_scheme).is_some_and(|path| path.starts_with(b"//"));
    }
    let Some((authority, encoded_path)) = after_scheme.split_once('/') else {
        return false;
    };
    (!authority.is_empty() && !is_local_authority(authority, context))
        || encoded_path.starts_with('/')
        || percent_decode_strict(&format!("/{encoded_path}"))
            .is_some_and(|path| path.starts_with(b"//"))
}

fn parse_standard_cwd_uri(
    payload: &str,
    context: &LocalCwdContext,
) -> Option<CwdPresentation> {
    let after_scheme = if let Some(rest) = payload.strip_prefix("file://") {
        rest
    } else if let Some(rest) = payload.strip_prefix("kitty-shell-cwd://") {
        rest
    } else {
        return None;
    };

    let (authority, encoded_path) = if after_scheme.starts_with('/') {
        ("", after_scheme)
    } else {
        let slash = after_scheme.find('/')?;
        (&after_scheme[..slash], &after_scheme[slash..])
    };

    if encoded_path.is_empty()
        || encoded_path.contains('?')
        || encoded_path.contains('#')
        || !valid_authority(authority)
    {
        return None;
    }

    let decoded = percent_decode_strict(encoded_path)?;
    if decoded.is_empty() || decoded.contains(&0) {
        return None;
    }
    let local_authority = is_local_authority(authority, context);
    let normalized_authority = normalize_host(authority);

    let location = if context.windows_host && !local_authority && !authority.is_empty() {
        decoded_unc_location(&normalized_authority, &decoded)?
    } else {
        if !local_authority {
            // A foreign Unix authority is a remote URI, not a local cwd.
            return None;
        }
        decoded_local_location(&decoded, context.windows_host)?
    };

    let unauthenticated_unc = location.flavor == PathFlavor::WindowsUnc;
    let mut presentation = build_presentation(
        location,
        if authority.is_empty() {
            "localhost"
        } else {
            authority
        },
        context,
    );
    // Standard OSC 7 has no session authentication. Keep valid UNC values for
    // title/display/copy compatibility, but never let terminal output trigger
    // an SMB access through File Explorer, AI, or another operational consumer.
    // Session-bound `nyaterm-cmd` reports remain eligible for UNC operations.
    if unauthenticated_unc {
        presentation.operational_path = None;
    }
    Some(presentation)
}

#[derive(Debug)]
struct DecodedLocation {
    native_utf8: Option<String>,
    title_bytes: Vec<u8>,
    encoded_path_bytes: Vec<u8>,
    uri_authority: String,
    flavor: PathFlavor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PathFlavor {
    Posix,
    WindowsDrive,
    WindowsUnc,
}

fn decoded_local_location(decoded: &[u8], windows_host: bool) -> Option<DecodedLocation> {
    if windows_host && decoded.starts_with(b"//") {
        let utf8 = std::str::from_utf8(decoded).ok()?;
        let mut parts = utf8[2..].split('/');
        let server = parts.next()?;
        let share = parts.next()?;
        if !valid_unc_component(server) || !valid_unc_component(share) {
            return None;
        }
        let native = format!("\\\\{}", utf8[2..].replace('/', "\\"));
        let authority_end = 2 + server.len();
        return Some(DecodedLocation {
            native_utf8: Some(native),
            title_bytes: decoded[authority_end..].to_vec(),
            encoded_path_bytes: decoded[authority_end..].to_vec(),
            uri_authority: server.to_string(),
            flavor: PathFlavor::WindowsUnc,
        });
    }

    if windows_host && is_slash_prefixed_drive(decoded) {
        let without_leading = &decoded[1..];
        return Some(DecodedLocation {
            native_utf8: std::str::from_utf8(without_leading)
                .ok()
                .map(|value| value.replace('/', "\\")),
            title_bytes: without_leading.to_vec(),
            encoded_path_bytes: decoded.to_vec(),
            uri_authority: String::new(),
            flavor: PathFlavor::WindowsDrive,
        });
    }

    if !decoded.starts_with(b"/") {
        return None;
    }

    Some(DecodedLocation {
        native_utf8: std::str::from_utf8(decoded).ok().map(str::to_string),
        title_bytes: decoded.to_vec(),
        encoded_path_bytes: decoded.to_vec(),
        uri_authority: String::new(),
        flavor: PathFlavor::Posix,
    })
}

fn decoded_unc_location(authority: &str, decoded: &[u8]) -> Option<DecodedLocation> {
    if authority.is_empty() || !decoded.starts_with(b"/") {
        return None;
    }
    let utf8_path = std::str::from_utf8(decoded).ok()?;
    let share = utf8_path[1..].split('/').next()?;
    if !valid_unc_component(authority) || !valid_unc_component(share) {
        return None;
    }

    Some(DecodedLocation {
        native_utf8: Some(format!(
            "\\\\{}\\{}",
            authority,
            utf8_path[1..].replace('/', "\\")
        )),
        title_bytes: decoded.to_vec(),
        encoded_path_bytes: decoded.to_vec(),
        uri_authority: authority.to_string(),
        flavor: PathFlavor::WindowsUnc,
    })
}

fn parse_cmd_path(raw_path: &str, context: &LocalCwdContext) -> Option<CwdPresentation> {
    if raw_path.is_empty() || raw_path.contains('\0') {
        return None;
    }

    let (flavor, encoded_path, authority) = if is_windows_drive_path(raw_path) {
        (
            PathFlavor::WindowsDrive,
            format!("/{}", raw_path.replace('\\', "/")).into_bytes(),
            String::new(),
        )
    } else if let Some(rest) = raw_path.strip_prefix("\\\\") {
        let mut parts = rest.split('\\');
        let server = parts.next()?;
        let share = parts.next()?;
        if !valid_unc_component(server) || !valid_unc_component(share) {
            return None;
        }
        let path_without_server = rest
            .strip_prefix(server)?
            .strip_prefix('\\')?;
        (
            PathFlavor::WindowsUnc,
            format!("/{}", path_without_server.replace('\\', "/")).into_bytes(),
            server.to_string(),
        )
    } else {
        return None;
    };

    let title_bytes = if flavor == PathFlavor::WindowsUnc {
        encoded_path.clone()
    } else {
        raw_path.replace('\\', "/").into_bytes()
    };
    Some(build_presentation(
        DecodedLocation {
            native_utf8: Some(raw_path.to_string()),
            title_bytes,
            encoded_path_bytes: encoded_path,
            uri_authority: authority.clone(),
            flavor,
        },
        if authority.is_empty() {
            "localhost"
        } else {
            &authority
        },
        context,
    ))
}

fn build_presentation(
    location: DecodedLocation,
    source_authority: &str,
    context: &LocalCwdContext,
) -> CwdPresentation {
    let authority = if location.flavor == PathFlavor::WindowsUnc {
        location.uri_authority.as_str()
    } else if source_authority.is_empty() {
        "localhost"
    } else {
        source_authority
    };
    let encoded_path = percent_encode_path(&location.encoded_path_bytes);
    let encoded_uri = format!("file://{authority}{encoded_path}");

    let safe_native = location.native_utf8.as_deref().is_some_and(|path| {
        !path.chars().any(is_forbidden_path_scalar)
            && match location.flavor {
                PathFlavor::WindowsDrive | PathFlavor::WindowsUnc => {
                    validate_windows_native_path(path, location.flavor)
                }
                PathFlavor::Posix => true,
            }
    });

    if safe_native {
        let native = location.native_utf8.expect("checked native path");
        let title = compact_path_title(&native, location.flavor, &context.home_paths);
        let operational_path = host_operational_path(
            &native,
            location.flavor,
            context.windows_host,
        );
        CwdPresentation {
            title,
            display_path: native.clone(),
            copy_value: native,
            operational_path,
            copy_as_uri: false,
        }
    } else {
        let encoded_title_source = std::str::from_utf8(&location.title_bytes)
            .ok()
            .map(|value| percent_encode_path(value.as_bytes()))
            .unwrap_or_else(|| percent_encode_path(&location.title_bytes));
        CwdPresentation {
            title: compact_encoded_title(
                &encoded_title_source,
                authority,
                location.flavor,
            ),
            display_path: encoded_uri.clone(),
            copy_value: encoded_uri,
            operational_path: None,
            copy_as_uri: true,
        }
    }
}

fn compact_path_title(path: &str, flavor: PathFlavor, homes: &[String]) -> String {
    let slash_path = path.replace('\\', "/");
    let home = homes.iter().find_map(|home| {
        let slash_home = home.replace('\\', "/");
        strip_path_prefix(&slash_path, &slash_home, flavor == PathFlavor::Posix)
            .map(|suffix| format!("~{suffix}"))
    });
    let value = home.as_deref().unwrap_or(&slash_path);
    compact_slash_path(value, flavor)
}

fn strip_path_prefix(path: &str, prefix: &str, case_sensitive: bool) -> Option<String> {
    let path_cmp = if case_sensitive {
        path.to_string()
    } else {
        path.to_ascii_lowercase()
    };
    let prefix_cmp = if case_sensitive {
        prefix.trim_end_matches('/').to_string()
    } else {
        prefix.trim_end_matches('/').to_ascii_lowercase()
    };
    if path_cmp == prefix_cmp {
        return Some(String::new());
    }
    let rest = path_cmp.strip_prefix(&prefix_cmp)?;
    if !rest.starts_with('/') {
        return None;
    }
    Some(path[prefix_cmp.len()..].to_string())
}

fn compact_slash_path(value: &str, flavor: PathFlavor) -> String {
    if value == "/" || value == "~" || is_drive_root(value) {
        return value.to_string();
    }

    let (root, body) = match flavor {
        _ if value.starts_with("~/") => ("~/", value.trim_start_matches("~/")),
        PathFlavor::WindowsUnc if value.starts_with("//") => {
            let mut segments = value[2..].split('/');
            let server = segments.next().unwrap_or_default();
            let share = segments.next().unwrap_or_default();
            let root_len = 2 + server.len() + 1 + share.len();
            (&value[..root_len], value[root_len..].trim_start_matches('/'))
        }
        PathFlavor::WindowsDrive if value.len() >= 3 => {
            (&value[..3], value[3..].trim_start_matches('/'))
        }
        PathFlavor::Posix if value.starts_with('/') => ("/", value.trim_start_matches('/')),
        _ => ("", value),
    };

    let parts: Vec<&str> = body.split('/').filter(|part| !part.is_empty()).collect();
    if parts.len() <= 2 {
        return value.to_string();
    }
    let separator = if root.is_empty() || root.ends_with('/') {
        ""
    } else {
        "/"
    };
    format!(
        "{root}{separator}…/{}",
        parts[parts.len() - 2..].join("/")
    )
}

fn compact_encoded_title(
    encoded_path: &str,
    authority: &str,
    flavor: PathFlavor,
) -> String {
    let value = if flavor == PathFlavor::WindowsUnc {
        format!("//{authority}{encoded_path}")
    } else if flavor == PathFlavor::WindowsDrive {
        encoded_path.trim_start_matches('/').to_string()
    } else {
        encoded_path.to_string()
    };
    compact_slash_path(&value, flavor)
}

fn percent_decode_strict(input: &str) -> Option<Vec<u8>> {
    let bytes = input.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                return None;
            }
            let high = hex_value(bytes[index + 1])?;
            let low = hex_value(bytes[index + 2])?;
            output.push(high * 16 + low);
            index += 3;
        } else {
            if bytes[index].is_ascii_control() {
                return None;
            }
            // Mature Bash/Zsh integrations commonly emit raw UTF-8 despite
            // URI recommendations. Preserve those bytes for compatibility;
            // managed NyaTerm producers still percent-encode them.
            output.push(bytes[index]);
            index += 1;
        }
    }
    Some(output)
}

fn percent_encode_path(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len());
    for &byte in bytes {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/' | b':')
        {
            output.push(byte as char);
        } else {
            use std::fmt::Write as _;
            let _ = write!(output, "%{byte:02X}");
        }
    }
    output
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn is_forbidden_path_scalar(ch: char) -> bool {
    matches!(
        ch,
        '\u{0000}'..='\u{001F}'
            | '\u{007F}'..='\u{009F}'
            | '\u{061C}'
            | '\u{200E}'
            | '\u{200F}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2066}'..='\u{206F}'
            | '\u{FFFD}'
    )
}

fn is_windows_drive_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    matches!(bytes, [drive, b':', slash, ..] if drive.is_ascii_alphabetic() && matches!(slash, b'/' | b'\\'))
}

fn is_slash_prefixed_drive(path: &[u8]) -> bool {
    matches!(path, [b'/', drive, b':', b'/', ..] if drive.is_ascii_alphabetic())
}

fn is_drive_root(path: &str) -> bool {
    let bytes = path.as_bytes();
    matches!(bytes, [drive, b':', b'/'] if drive.is_ascii_alphabetic())
}

fn host_operational_path(
    path: &str,
    flavor: PathFlavor,
    windows_host: bool,
) -> Option<String> {
    if !windows_host {
        return (flavor == PathFlavor::Posix).then(|| path.to_string());
    }

    match flavor {
        PathFlavor::WindowsDrive | PathFlavor::WindowsUnc => {
            validate_windows_native_path(path, flavor).then(|| path.to_string())
        }
        PathFlavor::Posix => map_msys_path_to_windows(path),
    }
}

fn map_msys_path_to_windows(path: &str) -> Option<String> {
    let slash_path = path.replace('\\', "/");
    let (drive, rest) = if let Some(rest) = slash_path.strip_prefix("/cygdrive/") {
        let bytes = rest.as_bytes();
        if bytes.is_empty() || !bytes[0].is_ascii_alphabetic() {
            return None;
        }
        (bytes[0] as char, &rest[1..])
    } else {
        let bytes = slash_path.as_bytes();
        if bytes.len() < 2 || bytes[0] != b'/' || !bytes[1].is_ascii_alphabetic() {
            return None;
        }
        (bytes[1] as char, &slash_path[2..])
    };
    if !rest.is_empty() && !rest.starts_with('/') {
        return None;
    }
    let suffix = rest.trim_start_matches('/').replace('/', "\\");
    let native = if suffix.is_empty() {
        format!("{}:\\", drive.to_ascii_uppercase())
    } else {
        format!("{}:\\{suffix}", drive.to_ascii_uppercase())
    };
    validate_windows_native_path(&native, PathFlavor::WindowsDrive).then_some(native)
}

fn validate_windows_native_path(path: &str, flavor: PathFlavor) -> bool {
    let slash_path = path.replace('\\', "/");
    if slash_path.starts_with("//?/") || slash_path.starts_with("//./") {
        return false;
    }

    let components: Vec<&str> = match flavor {
        PathFlavor::WindowsDrive => {
            let bytes = slash_path.as_bytes();
            if bytes.len() < 3
                || !bytes[0].is_ascii_alphabetic()
                || bytes[1] != b':'
                || bytes[2] != b'/'
            {
                return false;
            }
            slash_path[3..].split('/').filter(|part| !part.is_empty()).collect()
        }
        PathFlavor::WindowsUnc => {
            let Some(rest) = slash_path.strip_prefix("//") else {
                return false;
            };
            let parts: Vec<&str> = rest.split('/').collect();
            if parts.len() < 2
                || !valid_unc_component(parts[0])
                || !valid_unc_component(parts[1])
            {
                return false;
            }
            parts[2..].to_vec()
        }
        PathFlavor::Posix => return false,
    };
    components.into_iter().all(valid_windows_component)
}

fn valid_windows_component(value: &str) -> bool {
    if value.is_empty()
        || matches!(value, "." | "..")
        || value.ends_with(' ')
        || value.ends_with('.')
        || value.chars().any(|ch| {
            is_forbidden_path_scalar(ch)
                || matches!(ch, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')
        })
    {
        return false;
    }

    let stem = value
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    !matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        && !matches!(stem.as_bytes(), [b'C', b'O', b'M', b'1'..=b'9'])
        && !matches!(stem.as_bytes(), [b'L', b'P', b'T', b'1'..=b'9'])
}

fn valid_authority(authority: &str) -> bool {
    if authority.is_empty() {
        return true;
    }
    if matches!(authority, "." | "..") {
        return false;
    }
    if authority.starts_with('[') && authority.ends_with(']') {
        return authority[1..authority.len() - 1]
            .parse::<std::net::Ipv6Addr>()
            .is_ok();
    }
    !authority.contains(['@', ':', '%'])
        && authority
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
}

fn valid_unc_component(value: &str) -> bool {
    !value.is_empty()
        && !matches!(value, "." | "..")
        && !value.chars().any(|ch| {
            is_forbidden_path_scalar(ch)
                || matches!(ch, '/' | '\\' | ':' | '<' | '>' | '"' | '|' | '?' | '*')
        })
}

fn is_local_authority(authority: &str, context: &LocalCwdContext) -> bool {
    let host = normalize_host(authority);
    host.is_empty()
        || is_loopback_host(&host)
        || context.local_host_aliases.contains(&host)
}

fn is_loopback_host(host: &str) -> bool {
    matches!(
        normalize_host(host).as_str(),
        "localhost" | "127.0.0.1" | "::1"
    )
}

fn normalize_host(host: &str) -> String {
    host.trim()
        .trim_end_matches('\0')
        .trim_start_matches('[')
        .trim_end_matches(']')
        .trim_end_matches('.')
        .to_ascii_lowercase()
}

fn collect_local_host_aliases() -> HashSet<String> {
    let mut aliases = HashSet::new();
    for key in ["COMPUTERNAME", "HOSTNAME"] {
        if let Ok(value) = std::env::var(key) {
            add_host_alias(&mut aliases, &value);
        }
    }

    for value in os_hostnames() {
        add_host_alias(&mut aliases, &value);
    }
    aliases
}

fn add_host_alias(aliases: &mut HashSet<String>, value: &str) {
    let normalized = normalize_host(value);
    if normalized.is_empty() {
        return;
    }
    aliases.insert(normalized.clone());
    if let Some((short, _)) = normalized.split_once('.') {
        aliases.insert(short.to_string());
    }
}

#[cfg(target_os = "windows")]
fn os_hostnames() -> Vec<String> {
    use windows::Win32::System::SystemInformation::{
        COMPUTER_NAME_FORMAT, ComputerNameDnsFullyQualified, ComputerNameDnsHostname,
        GetComputerNameExW,
    };
    use windows::core::PWSTR;

    fn query(format: COMPUTER_NAME_FORMAT) -> Option<String> {
        let mut len = 0_u32;
        // The first call reports the required UTF-16 capacity.
        let _ = unsafe { GetComputerNameExW(format, None, &mut len) };
        if len == 0 {
            return None;
        }
        let mut buffer = vec![0_u16; len as usize];
        unsafe { GetComputerNameExW(format, Some(PWSTR(buffer.as_mut_ptr())), &mut len) }.ok()?;
        String::from_utf16(&buffer[..len as usize]).ok()
    }

    [ComputerNameDnsHostname, ComputerNameDnsFullyQualified]
        .into_iter()
        .filter_map(query)
        .collect()
}

#[cfg(unix)]
fn os_hostnames() -> Vec<String> {
    let mut buffer = [0_u8; 256];
    let result = unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) };
    if result != 0 {
        return Vec::new();
    }
    let end = buffer
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(buffer.len());
    std::str::from_utf8(&buffer[..end])
        .ok()
        .filter(|value| !value.is_empty())
        .map(|value| vec![value.to_string()])
        .unwrap_or_default()
}

#[cfg(not(any(target_os = "windows", unix)))]
fn os_hostnames() -> Vec<String> {
    Vec::new()
}

fn collect_home_paths() -> Vec<String> {
    let mut paths = Vec::new();
    for key in ["HOME", "USERPROFILE"] {
        if let Ok(value) = std::env::var(key) {
            let value = value.trim();
            if !value.is_empty() && !paths.iter().any(|existing| existing == value) {
                paths.push(value.to_string());
            }
        }
    }
    paths
}

#[cfg(test)]
mod cwd_tests {
    use super::{
        CwdPresentationUpdate, LocalCwdContext, MAX_LOCAL_CWD_PAYLOAD_BYTES,
        OperationalCwdUpdate, cwd_state_replacement, operational_cwd_update,
        parse_local_cwd_report,
    };

    fn unix_context() -> LocalCwdContext {
        LocalCwdContext::for_test("session-1", &["workstation"], &["/home/alice"], false)
    }

    fn windows_context() -> LocalCwdContext {
        LocalCwdContext::for_test(
            "session-1",
            &["workstation"],
            &[r"C:\Users\Alice"],
            true,
        )
    }

    #[test]
    fn mature_raw_utf8_osc7_remains_compatible() {
        let report = parse_local_cwd_report(
            "kitty-shell-cwd://workstation/home/alice/项目",
            &unix_context(),
        );
        let CwdPresentationUpdate::Set(presentation) = report.presentation else {
            panic!("raw UTF-8 presentation");
        };
        assert_eq!(presentation.copy_value, "/home/alice/项目");
        assert_eq!(presentation.title, "~/项目");
    }

    #[test]
    fn managed_safe_paths_feed_canonical_operational_consumers() {
        let report = parse_local_cwd_report(
            "file:///C:/Users/Alice/my%20project/%252F",
            &windows_context(),
        );
        assert_eq!(
            operational_cwd_update(&report, true),
            OperationalCwdUpdate::Set(r"C:\Users\Alice\my project\%2F".to_string())
        );
        assert_eq!(
            operational_cwd_update(&report, false),
            OperationalCwdUpdate::Set(
                "/C:/Users/Alice/my%20project/%252F".to_string()
            )
        );

        let cmd = parse_local_cwd_report(
            r"nyaterm-cmd://session-1/C:\Users\Alice\project",
            &windows_context(),
        );
        assert_eq!(
            operational_cwd_update(&cmd, true),
            OperationalCwdUpdate::Set(r"C:\Users\Alice\project".to_string())
        );
        assert_eq!(
            operational_cwd_update(&cmd, false),
            OperationalCwdUpdate::Ignore
        );

        let trusted_cmd_unc = parse_local_cwd_report(
            r"nyaterm-cmd://session-1/\\server\share\project",
            &windows_context(),
        );
        assert_eq!(
            operational_cwd_update(&trusted_cmd_unc, true),
            OperationalCwdUpdate::Set(r"\\server\share\project".to_string())
        );
    }

    #[test]
    fn unsafe_and_clear_locations_never_feed_operational_cwd() {
        for uri in [
            "file://localhost/home/alice/a%1Bb",
            "file://localhost/home/alice/a%E2%80%AEb",
            "file://localhost/home/alice/a%FFb",
            "file://localhost/home/alice/a\u{FFFD}b",
        ] {
            let unsafe_path = parse_local_cwd_report(uri, &unix_context());
            assert_eq!(
                operational_cwd_update(&unsafe_path, true),
                OperationalCwdUpdate::Clear,
                "managed {uri}"
            );
            assert_eq!(
                operational_cwd_update(&unsafe_path, false),
                OperationalCwdUpdate::Clear,
                "passive {uri}"
            );
        }

        let oversized = format!(
            "file:///{}",
            "a".repeat(MAX_LOCAL_CWD_PAYLOAD_BYTES - "file:///".len() + 1)
        );
        assert_eq!(oversized.len(), MAX_LOCAL_CWD_PAYLOAD_BYTES + 1);
        let oversized = parse_local_cwd_report(&oversized, &unix_context());
        for managed in [false, true] {
            assert_eq!(
                operational_cwd_update(&oversized, managed),
                OperationalCwdUpdate::Clear,
                "oversized, managed={managed}"
            );
        }

        let clear = parse_local_cwd_report(
            "nyaterm-clear://session-1",
            &windows_context(),
        );
        assert_eq!(
            operational_cwd_update(&clear, true),
            OperationalCwdUpdate::Clear
        );
    }

    #[test]
    fn strict_invalid_replacement_clears_stale_presentation_atomically() {
        let malformed = parse_local_cwd_report(
            "file://localhost/home/alice/a%2",
            &unix_context(),
        );
        let replacement = cwd_state_replacement(&malformed, true);
        assert_eq!(replacement.legacy_path.as_deref(), Some("/home/alice/a%2"));
        assert!(replacement.operational_path.is_none());
        assert!(replacement.presentation.is_none());
    }

    #[test]
    fn windows_invalid_native_components_are_presentation_only_uris() {
        for uri in [
            "file:///C:/Temp/bad%3Fname",
            "file:///C:/Temp/file%3Astream",
            "file:///C:/Temp/CON.txt",
            "file:///C:/Temp/trailing.%20",
        ] {
            let report = parse_local_cwd_report(uri, &windows_context());
            let CwdPresentationUpdate::Set(ref presentation) = report.presentation else {
                panic!("presentation for {uri}");
            };
            assert!(presentation.copy_as_uri, "{uri}");
            assert!(presentation.operational_path.is_none(), "{uri}");
            assert_eq!(
                operational_cwd_update(&report, true),
                OperationalCwdUpdate::Clear
            );
        }
    }

    #[test]
    fn windows_msys_paths_have_separate_shell_copy_and_host_operational_paths() {
        for (uri, copy, operational) in [
            (
                "file://localhost/c/Users/Alice/project",
                "/c/Users/Alice/project",
                r"C:\Users\Alice\project",
            ),
            (
                "file://localhost/cygdrive/d/work/tree",
                "/cygdrive/d/work/tree",
                r"D:\work\tree",
            ),
        ] {
            let report = parse_local_cwd_report(uri, &windows_context());
            let CwdPresentationUpdate::Set(ref presentation) = report.presentation else {
                panic!("presentation for {uri}");
            };
            assert_eq!(presentation.copy_value, copy);
            assert_eq!(presentation.operational_path.as_deref(), Some(operational));
            assert_eq!(
                operational_cwd_update(&report, true),
                OperationalCwdUpdate::Set(operational.to_string())
            );
        }

        let unmappable = parse_local_cwd_report(
            "file://localhost/usr/local/bin",
            &windows_context(),
        );
        let CwdPresentationUpdate::Set(ref presentation) = unmappable.presentation else {
            panic!("unmappable presentation");
        };
        assert_eq!(presentation.copy_value, "/usr/local/bin");
        assert!(presentation.operational_path.is_none());
        assert_eq!(
            operational_cwd_update(&unmappable, true),
            OperationalCwdUpdate::Clear
        );
    }

    #[test]
    fn managed_invalid_strict_signal_clears_operational_state_but_passive_keeps_legacy() {
        let malformed = parse_local_cwd_report(
            "file://localhost/home/alice/a%2",
            &unix_context(),
        );
        assert_eq!(malformed.presentation, CwdPresentationUpdate::Ignore);
        assert_eq!(
            operational_cwd_update(&malformed, true),
            OperationalCwdUpdate::Clear
        );
        assert_eq!(
            operational_cwd_update(&malformed, false),
            OperationalCwdUpdate::Set("/home/alice/a%2".to_string())
        );
    }

    #[test]
    fn standard_uri_round_trips_percent_and_unicode() {
        let report = parse_local_cwd_report(
            "file://localhost/home/alice/a%252F/%E9%A1%B9%E7%9B%AE",
            &unix_context(),
        );
        assert_eq!(report.legacy_path.as_deref(), Some("/home/alice/a%252F/%E9%A1%B9%E7%9B%AE"));
        let CwdPresentationUpdate::Set(presentation) = report.presentation else {
            panic!("presentation");
        };
        assert_eq!(presentation.display_path, "/home/alice/a%2F/项目");
        assert_eq!(presentation.copy_value, "/home/alice/a%2F/项目");
        assert!(!presentation.copy_as_uri);
        assert_eq!(presentation.title, "~/a%2F/项目");
    }

    #[test]
    fn dangerous_and_non_utf8_paths_are_uri_only() {
        for uri in [
            "file://localhost/home/alice/a%07b",
            "file://localhost/home/alice/a%1Bb",
            "file://localhost/home/alice/a%E2%80%AEb",
            "file://localhost/home/alice/%FF",
        ] {
            let report = parse_local_cwd_report(uri, &unix_context());
            let CwdPresentationUpdate::Set(presentation) = report.presentation else {
                panic!("presentation for {uri}");
            };
            assert!(presentation.copy_as_uri);
            assert_eq!(presentation.copy_value, uri);
            assert!(!presentation.display_path.contains('\u{1b}'));
            assert!(!presentation.display_path.contains('\u{202e}'));
        }
    }

    #[test]
    fn malformed_uri_is_ignored_without_changing_legacy_projection() {
        for uri in [
            "file://localhost/home/%2",
            "file://localhost/home/a?query",
            "file://localhost/home/a#fragment",
            "https://localhost/home/alice",
        ] {
            assert!(matches!(
                parse_local_cwd_report(uri, &unix_context()).presentation,
                CwdPresentationUpdate::Ignore
            ));
        }
    }

    #[test]
    fn unix_foreign_host_is_not_promoted_but_legacy_path_remains() {
        let report = parse_local_cwd_report(
            "file://remote.example/home/remote",
            &unix_context(),
        );
        assert_eq!(report.legacy_path.as_deref(), Some("/home/remote"));
        assert_eq!(report.presentation, CwdPresentationUpdate::Ignore);
    }

    #[test]
    fn unix_double_slash_remains_a_posix_path() {
        let report = parse_local_cwd_report(
            "file:////srv/share/project",
            &unix_context(),
        );
        let CwdPresentationUpdate::Set(presentation) = report.presentation else {
            panic!("posix presentation");
        };
        assert_eq!(presentation.display_path, "//srv/share/project");
        assert!(!presentation.copy_as_uri);
    }

    #[test]
    fn posix_drive_shaped_path_remains_posix() {
        let report = parse_local_cwd_report(
            "file://localhost/C:/project",
            &unix_context(),
        );
        let CwdPresentationUpdate::Set(presentation) = report.presentation else {
            panic!("posix presentation");
        };
        assert_eq!(presentation.display_path, "/C:/project");
        assert_eq!(presentation.copy_value, "/C:/project");
    }

    #[test]
    fn malformed_ipv6_and_windows_device_namespaces_are_rejected() {
        for uri in [
            "file://[]/tmp",
            "file://[not-ipv6]/tmp",
            "file://./pipe/name",
            "file:////./pipe/name",
            "file:////../share/name",
        ] {
            assert_eq!(
                parse_local_cwd_report(uri, &windows_context()).presentation,
                CwdPresentationUpdate::Ignore,
                "{uri}"
            );
        }
    }

    #[test]
    fn ipv6_loopback_is_local_but_foreign_ipv6_is_not_unc() {
        assert!(matches!(
            parse_local_cwd_report("file://[::1]/home/alice", &unix_context()).presentation,
            CwdPresentationUpdate::Set(_)
        ));
        assert_eq!(
            parse_local_cwd_report("file://[2001:db8::1]/share", &windows_context())
                .presentation,
            CwdPresentationUpdate::Ignore
        );
    }

    #[test]
    fn invalid_windows_unc_components_are_not_promoted() {
        for uri in [
            "file://server/bad%3Ashare/path",
            "file://server/bad%2Ashare/path",
            "file://server/bad%7Cshare/path",
        ] {
            assert_eq!(
                parse_local_cwd_report(uri, &windows_context()).presentation,
                CwdPresentationUpdate::Ignore
            );
        }
    }

    #[test]
    fn unauthenticated_standard_osc7_unc_is_presentation_only() {
        for uri in [
            "file://server/share/project/src",
            "file:////server/share/project/src",
            "file://localhost//server/share/project/src",
            "kitty-shell-cwd:////server/share/project/src",
            "kitty-shell-cwd://localhost//server/share/project/src",
        ] {
            let report = parse_local_cwd_report(uri, &windows_context());
            let CwdPresentationUpdate::Set(ref presentation) = report.presentation else {
                panic!("unc presentation for {uri}");
            };
            assert_eq!(presentation.display_path, r"\\server\share\project\src");
            assert_eq!(presentation.copy_value, r"\\server\share\project\src");
            assert_eq!(presentation.title, "//server/share/project/src");
            assert!(presentation.operational_path.is_none());
            for managed in [false, true] {
                assert_eq!(
                    operational_cwd_update(&report, managed),
                    OperationalCwdUpdate::Clear,
                    "{uri}, managed={managed}"
                );
            }
        }
    }

    #[test]
    fn deep_windows_unc_title_keeps_separator_after_share_root() {
        let report = parse_local_cwd_report(
            "file://server/share/project/src/module",
            &windows_context(),
        );
        let CwdPresentationUpdate::Set(presentation) = report.presentation else {
            panic!("deep unc presentation");
        };

        assert_eq!(presentation.title, "//server/share/…/src/module");
        assert_eq!(
            presentation.display_path,
            r"\\server\share\project\src\module"
        );
        assert!(presentation.operational_path.is_none());
    }

    #[test]
    fn windows_drive_and_home_are_normalized_only_for_presentation() {
        let report = parse_local_cwd_report(
            "file:///C:/Users/Alice/projects/nyaterm/src",
            &windows_context(),
        );
        assert_eq!(
            report.legacy_path.as_deref(),
            Some("/C:/Users/Alice/projects/nyaterm/src")
        );
        let CwdPresentationUpdate::Set(presentation) = report.presentation else {
            panic!("drive presentation");
        };
        assert_eq!(
            presentation.display_path,
            r"C:\Users\Alice\projects\nyaterm\src"
        );
        assert_eq!(presentation.title, "~/…/nyaterm/src");
    }

    #[test]
    fn dangerous_cmd_unc_path_uses_a_non_duplicated_encoded_authority() {
        let context = windows_context();
        let report = parse_local_cwd_report(
            "nyaterm-cmd://session-1/\\\\server\\share\\a\u{202e}b",
            &context,
        );
        let CwdPresentationUpdate::Set(presentation) = report.presentation else {
            panic!("cmd unc presentation");
        };
        assert!(presentation.copy_as_uri);
        assert_eq!(
            presentation.copy_value,
            "file://server/share/a%E2%80%AEb"
        );
        assert_eq!(presentation.title, "//server/share/a%E2%80%AEb");
    }

    #[test]
    fn cmd_and_clear_signals_are_session_bound() {
        let context = windows_context();
        let report = parse_local_cwd_report(
            "nyaterm-cmd://session-1/C:\\Users\\Alice\\project",
            &context,
        );
        let CwdPresentationUpdate::Set(presentation) = report.presentation else {
            panic!("cmd presentation");
        };
        assert_eq!(presentation.title, "~/project");
        assert!(matches!(
            parse_local_cwd_report("nyaterm-clear://session-1", &context).presentation,
            CwdPresentationUpdate::Clear
        ));
        assert!(matches!(
            parse_local_cwd_report(
                "nyaterm-cmd://other/C:\\Users\\Alice",
                &context
            )
            .presentation,
            CwdPresentationUpdate::Ignore
        ));
    }
}
