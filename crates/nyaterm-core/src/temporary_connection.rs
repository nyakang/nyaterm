#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemporaryLinkProtocol {
    Ssh,
    Telnet,
    Serial,
}

impl TemporaryLinkProtocol {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ssh => "ssh",
            Self::Telnet => "telnet",
            Self::Serial => "serial",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemporarySshLinkConfig {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemporaryTelnetLinkConfig {
    pub name: String,
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemporarySerialLinkConfig {
    pub name: String,
    pub port_name: String,
    pub baud_rate: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemporaryLinkError {
    Empty,
    UnsupportedOption,
    MissingHost,
    MissingSerialPort,
    InlinePassword,
    InvalidPort,
    InvalidBaudRate,
    InvalidInput,
}

impl TemporaryLinkError {
    pub fn locale_key(self) -> &'static str {
        match self {
            Self::Empty => "temporarySsh.empty",
            Self::UnsupportedOption => "temporarySsh.unsupportedOption",
            Self::MissingHost => "temporarySsh.missingHost",
            Self::MissingSerialPort => "temporarySsh.serialPortRequired",
            Self::InlinePassword => "temporarySsh.inlinePassword",
            Self::InvalidPort => "temporarySsh.invalidPort",
            Self::InvalidBaudRate => "temporarySsh.invalidBaudRate",
            Self::InvalidInput => "temporarySsh.invalidInput",
        }
    }
}

const DEFAULT_USERNAME: &str = "root";
const DEFAULT_SSH_PORT: u16 = 22;
const DEFAULT_TELNET_PORT: u16 = 23;
const DEFAULT_SERIAL_BAUD_RATE: u32 = 115_200;
const UNSUPPORTED_OPTIONS: [&str; 14] = [
    "-J", "-L", "-R", "-D", "-W", "-b", "-c", "-F", "-I", "-i", "-m", "-o", "-S", "-w",
];
const UNSUPPORTED_OPTION_CHARS: [char; 14] = [
    'J', 'L', 'R', 'D', 'W', 'b', 'c', 'F', 'I', 'i', 'm', 'O', 'S', 'w',
];
const UNSUPPORTED_LONG_OPTIONS: [&str; 7] = [
    "proxyjump",
    "proxycommand",
    "localforward",
    "remoteforward",
    "dynamicforward",
    "identityfile",
    "controlpath",
];

pub fn parse_temporary_ssh_link(input: &str) -> Result<TemporarySshLinkConfig, TemporaryLinkError> {
    let text = input.trim();
    if text.is_empty() {
        return Err(TemporaryLinkError::Empty);
    }

    if text.to_ascii_lowercase().starts_with("ssh://") {
        return parse_ssh_url(text);
    }

    let tokens = tokenize_shell_like(text);
    if tokens.is_empty() {
        return Err(TemporaryLinkError::Empty);
    }

    let command_tokens = if tokens
        .first()
        .is_some_and(|token| token.eq_ignore_ascii_case("ssh"))
    {
        &tokens[1..]
    } else {
        &tokens[..]
    };
    let mut username = None;
    let mut host_spec = None;
    let mut port = None;
    let mut index = 0;

    while index < command_tokens.len() {
        let token = command_tokens[index].as_str();
        if token.is_empty() {
            index += 1;
            continue;
        }

        if token == "--" {
            host_spec = find_host_spec(&command_tokens[index + 1..]).or(host_spec);
            break;
        }

        if token == "-p" {
            if let Some(next) = command_tokens.get(index + 1)
                && let Ok(parsed) = parse_port_token(next)
            {
                port = Some(parsed);
                index += 2;
                continue;
            }
            index += 1;
            continue;
        }

        if let Some(inline_port) = token.strip_prefix("-p").filter(|value| !value.is_empty()) {
            if let Ok(parsed) = parse_port_token(inline_port) {
                port = Some(parsed);
            }
            index += 1;
            continue;
        }

        if token == "-l" {
            if let Some(next) = command_tokens.get(index + 1)
                && !next.starts_with('-')
                && !next.contains('@')
            {
                username = Some(next.clone());
                index += 2;
                continue;
            }
            index += 1;
            continue;
        }

        if let Some(inline_user) = token.strip_prefix("-l").filter(|value| !value.is_empty()) {
            username = Some(inline_user.to_string());
            index += 1;
            continue;
        }

        if is_unsupported_option(token) {
            return Err(TemporaryLinkError::UnsupportedOption);
        }

        if token.starts_with('-') {
            if option_consumes_value(token) {
                index += 2;
            } else {
                index += 1;
            }
            continue;
        }

        if host_spec.is_none() {
            host_spec = Some(token.to_string());
        }
        index += 1;
    }

    let Some(host_spec) = host_spec else {
        return Err(TemporaryLinkError::MissingHost);
    };
    build_ssh_config(&host_spec, username, port)
}

pub fn parse_temporary_telnet_link(
    input: &str,
) -> Result<TemporaryTelnetLinkConfig, TemporaryLinkError> {
    let text = input.trim();
    if text.is_empty() {
        return Err(TemporaryLinkError::Empty);
    }

    if text.to_ascii_lowercase().starts_with("telnet://") {
        return parse_telnet_url(text);
    }

    let tokens = tokenize_shell_like(text);
    if tokens.is_empty() {
        return Err(TemporaryLinkError::Empty);
    }

    let command_tokens = if tokens
        .first()
        .is_some_and(|token| token.eq_ignore_ascii_case("telnet"))
    {
        &tokens[1..]
    } else {
        &tokens[..]
    };
    let mut host_spec = None;
    let mut port = None;
    let mut index = 0;

    while index < command_tokens.len() {
        let token = command_tokens[index].as_str();
        if token.is_empty() {
            index += 1;
            continue;
        }

        if token == "--" {
            host_spec = find_host_spec(&command_tokens[index + 1..]).or(host_spec);
            break;
        }

        if token.starts_with('-') {
            index += 1;
            continue;
        }

        if host_spec.is_none() {
            host_spec = Some(token.to_string());
        } else if port.is_none() {
            port = Some(parse_port_token(token)?);
        }
        index += 1;
    }

    let Some(host_spec) = host_spec else {
        return Err(TemporaryLinkError::MissingHost);
    };
    build_telnet_config(&host_spec, port)
}

pub fn build_temporary_serial_link(
    port_name: &str,
    baud_rate: &str,
) -> Result<TemporarySerialLinkConfig, TemporaryLinkError> {
    let port_name = port_name.trim();
    if port_name.is_empty() {
        return Err(TemporaryLinkError::MissingSerialPort);
    }
    let baud_rate = if baud_rate.trim().is_empty() {
        DEFAULT_SERIAL_BAUD_RATE
    } else {
        baud_rate
            .trim()
            .parse::<u32>()
            .ok()
            .filter(|value| *value > 0)
            .ok_or(TemporaryLinkError::InvalidBaudRate)?
    };
    Ok(TemporarySerialLinkConfig {
        name: format!("{port_name} @ {baud_rate}"),
        port_name: port_name.to_string(),
        baud_rate,
    })
}

fn parse_ssh_url(text: &str) -> Result<TemporarySshLinkConfig, TemporaryLinkError> {
    let rest = text
        .get(6..)
        .ok_or(TemporaryLinkError::InvalidInput)?
        .trim();
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    if authority.is_empty() {
        return Err(TemporaryLinkError::MissingHost);
    }

    let (username, host_port) = match authority.rsplit_once('@') {
        Some((user_info, host_port)) => {
            if user_info.contains(':') {
                return Err(TemporaryLinkError::InlinePassword);
            }
            let username = percent_decode_basic(user_info).unwrap_or_else(|| user_info.to_string());
            (
                if username.is_empty() {
                    DEFAULT_USERNAME.to_string()
                } else {
                    username
                },
                host_port,
            )
        }
        None => (DEFAULT_USERNAME.to_string(), authority),
    };
    let parsed = parse_host_port(host_port)?;
    if parsed.host.is_empty() {
        return Err(TemporaryLinkError::MissingHost);
    }
    create_ssh_config(
        parsed.host,
        username,
        parsed.port.unwrap_or(DEFAULT_SSH_PORT),
    )
}

fn parse_telnet_url(text: &str) -> Result<TemporaryTelnetLinkConfig, TemporaryLinkError> {
    let rest = text
        .get(9..)
        .ok_or(TemporaryLinkError::InvalidInput)?
        .trim();
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    if authority.is_empty() {
        return Err(TemporaryLinkError::MissingHost);
    }
    if authority.contains('@') {
        return Err(TemporaryLinkError::InlinePassword);
    }

    let parsed = parse_host_port(authority)?;
    if parsed.host.is_empty() {
        return Err(TemporaryLinkError::MissingHost);
    }
    create_telnet_config(parsed.host, parsed.port.unwrap_or(DEFAULT_TELNET_PORT))
}

fn build_ssh_config(
    host_spec: &str,
    explicit_username: Option<String>,
    explicit_port: Option<u16>,
) -> Result<TemporarySshLinkConfig, TemporaryLinkError> {
    if host_spec.contains("://") && !host_spec.to_ascii_lowercase().starts_with("ssh://") {
        return Err(TemporaryLinkError::InvalidInput);
    }
    let (username, target) = match host_spec.rsplit_once('@') {
        Some((user_part, target)) => {
            if user_part.contains(':') {
                return Err(TemporaryLinkError::InlinePassword);
            }
            (
                if user_part.is_empty() {
                    explicit_username
                } else {
                    Some(user_part.to_string())
                },
                target,
            )
        }
        None => (explicit_username, host_spec),
    };
    let parsed = parse_host_port(target)?;
    if parsed.host.is_empty() {
        return Err(TemporaryLinkError::MissingHost);
    }
    create_ssh_config(
        parsed.host,
        username.unwrap_or_else(|| DEFAULT_USERNAME.to_string()),
        explicit_port.or(parsed.port).unwrap_or(DEFAULT_SSH_PORT),
    )
}

fn build_telnet_config(
    host_spec: &str,
    explicit_port: Option<u16>,
) -> Result<TemporaryTelnetLinkConfig, TemporaryLinkError> {
    if host_spec.contains("://") && !host_spec.to_ascii_lowercase().starts_with("telnet://") {
        return Err(TemporaryLinkError::InvalidInput);
    }
    if host_spec.contains('@') {
        return Err(TemporaryLinkError::InlinePassword);
    }
    let parsed = parse_host_port(host_spec)?;
    if parsed.host.is_empty() {
        return Err(TemporaryLinkError::MissingHost);
    }
    create_telnet_config(
        parsed.host,
        explicit_port.or(parsed.port).unwrap_or(DEFAULT_TELNET_PORT),
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedHostPort {
    host: String,
    port: Option<u16>,
}

fn parse_host_port(target: &str) -> Result<ParsedHostPort, TemporaryLinkError> {
    if let Some(rest) = target.strip_prefix('[') {
        let Some(end) = rest.find(']') else {
            return Ok(ParsedHostPort {
                host: target.to_string(),
                port: None,
            });
        };
        let host = rest[..end].to_string();
        let tail = &rest[end + 1..];
        let port = tail.strip_prefix(':').map(parse_port_token).transpose()?;
        return Ok(ParsedHostPort { host, port });
    }

    if target.matches(':').count() == 1 {
        let (host, port_text) = target
            .split_once(':')
            .ok_or(TemporaryLinkError::InvalidInput)?;
        let port = if port_text.is_empty() {
            None
        } else {
            Some(parse_port_token(port_text)?)
        };
        return Ok(ParsedHostPort {
            host: host.to_string(),
            port,
        });
    }

    Ok(ParsedHostPort {
        host: target.to_string(),
        port: None,
    })
}

fn create_ssh_config(
    host: String,
    username: String,
    port: u16,
) -> Result<TemporarySshLinkConfig, TemporaryLinkError> {
    if host.trim().is_empty() || username.trim().is_empty() {
        return Err(TemporaryLinkError::MissingHost);
    }
    let host = normalized_host(&host);
    let username = username.trim().to_string();
    let name = format!("{username}@{host}:{port}");
    Ok(TemporarySshLinkConfig {
        name,
        host,
        port,
        username,
    })
}

fn create_telnet_config(
    host: String,
    port: u16,
) -> Result<TemporaryTelnetLinkConfig, TemporaryLinkError> {
    if host.trim().is_empty() {
        return Err(TemporaryLinkError::MissingHost);
    }
    let host = normalized_host(&host);
    let name = format!("telnet://{host}:{port}");
    Ok(TemporaryTelnetLinkConfig { name, host, port })
}

fn normalized_host(host: &str) -> String {
    host.trim()
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or(host.trim())
        .to_string()
}

fn tokenize_shell_like(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut escaping = false;

    for character in text.chars() {
        if escaping {
            current.push(character);
            escaping = false;
            continue;
        }
        if character == '\\' && quote != Some('\'') {
            escaping = true;
            continue;
        }
        if matches!(character, '\'' | '"') && quote.is_none() {
            quote = Some(character);
            continue;
        }
        if quote == Some(character) {
            quote = None;
            continue;
        }
        if character.is_whitespace() && quote.is_none() {
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
            continue;
        }
        current.push(character);
    }

    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

fn find_host_spec(tokens: &[String]) -> Option<String> {
    tokens
        .iter()
        .find(|token| !token.is_empty() && !token.starts_with('-'))
        .cloned()
}

fn is_unsupported_option(token: &str) -> bool {
    if UNSUPPORTED_OPTIONS.contains(&token) {
        return true;
    }
    if token.starts_with("-o") {
        return is_unsupported_open_ssh_option(
            token.trim_start_matches("-o").trim_start_matches('='),
        );
    }
    token
        .strip_prefix('-')
        .and_then(|value| value.chars().next())
        .is_some_and(|flag| UNSUPPORTED_OPTION_CHARS.contains(&flag))
}

fn is_unsupported_open_ssh_option(option_text: &str) -> bool {
    let option = option_text
        .split('=')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    UNSUPPORTED_LONG_OPTIONS.contains(&option.as_str())
}

fn option_consumes_value(token: &str) -> bool {
    matches!(token, "-A" | "-a" | "-E" | "-e" | "-Q")
}

fn parse_port_token(value: &str) -> Result<u16, TemporaryLinkError> {
    value
        .parse::<u16>()
        .ok()
        .filter(|port| *port >= 1)
        .ok_or(TemporaryLinkError::InvalidPort)
}

fn percent_decode_basic(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hi = bytes.get(index + 1).copied()?;
            let lo = bytes.get(index + 2).copied()?;
            let hex = [hi, lo];
            let decoded = std::str::from_utf8(&hex)
                .ok()
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())?;
            output.push(decoded);
            index += 3;
        } else {
            output.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(output).ok()
}

#[cfg(test)]
mod tests {
    use super::{
        TemporaryLinkError, build_temporary_serial_link, parse_temporary_ssh_link,
        parse_temporary_telnet_link,
    };

    #[test]
    fn parses_ssh_url() {
        let parsed = parse_temporary_ssh_link("ssh://deploy@example.com:2200").unwrap();
        assert_eq!(parsed.name, "deploy@example.com:2200");
        assert_eq!(parsed.host, "example.com");
        assert_eq!(parsed.port, 2200);
        assert_eq!(parsed.username, "deploy");
    }

    #[test]
    fn parses_openssh_command() {
        let parsed = parse_temporary_ssh_link("ssh -p 2222 -l admin example.test").unwrap();
        assert_eq!(parsed.name, "admin@example.test:2222");
        assert_eq!(parsed.host, "example.test");
        assert_eq!(parsed.port, 2222);
        assert_eq!(parsed.username, "admin");
    }

    #[test]
    fn parses_ssh_user_host_port() {
        let parsed = parse_temporary_ssh_link("kang@[2001:db8::1]:2022").unwrap();
        assert_eq!(parsed.name, "kang@2001:db8::1:2022");
        assert_eq!(parsed.host, "2001:db8::1");
        assert_eq!(parsed.port, 2022);
        assert_eq!(parsed.username, "kang");
    }

    #[test]
    fn rejects_ssh_inline_password() {
        let error = parse_temporary_ssh_link("root:secret@example.com").unwrap_err();
        assert_eq!(error, TemporaryLinkError::InlinePassword);
    }

    #[test]
    fn rejects_ssh_unsupported_options() {
        let error =
            parse_temporary_ssh_link("ssh -J jump.example.com root@example.com").unwrap_err();
        assert_eq!(error, TemporaryLinkError::UnsupportedOption);
    }

    #[test]
    fn parses_telnet_url() {
        let parsed = parse_temporary_telnet_link("telnet://example.com:2323").unwrap();
        assert_eq!(parsed.name, "telnet://example.com:2323");
        assert_eq!(parsed.host, "example.com");
        assert_eq!(parsed.port, 2323);
    }

    #[test]
    fn parses_telnet_command() {
        let parsed = parse_temporary_telnet_link("telnet example.test 2323").unwrap();
        assert_eq!(parsed.name, "telnet://example.test:2323");
        assert_eq!(parsed.host, "example.test");
        assert_eq!(parsed.port, 2323);
    }

    #[test]
    fn parses_telnet_host_port() {
        let parsed = parse_temporary_telnet_link("example.test:2323").unwrap();
        assert_eq!(parsed.name, "telnet://example.test:2323");
        assert_eq!(parsed.host, "example.test");
        assert_eq!(parsed.port, 2323);
    }

    #[test]
    fn rejects_telnet_missing_host() {
        let error = parse_temporary_telnet_link("telnet").unwrap_err();
        assert_eq!(error, TemporaryLinkError::MissingHost);
    }

    #[test]
    fn rejects_telnet_invalid_port() {
        let error = parse_temporary_telnet_link("telnet example.test nope").unwrap_err();
        assert_eq!(error, TemporaryLinkError::InvalidPort);
    }

    #[test]
    fn rejects_telnet_inline_password() {
        let error = parse_temporary_telnet_link("telnet://root@example.test").unwrap_err();
        assert_eq!(error, TemporaryLinkError::InlinePassword);
    }

    #[test]
    fn rejects_serial_missing_port() {
        let error = build_temporary_serial_link("", "115200").unwrap_err();
        assert_eq!(error, TemporaryLinkError::MissingSerialPort);
    }

    #[test]
    fn rejects_serial_invalid_baud_rate() {
        let error = build_temporary_serial_link("COM1", "fast").unwrap_err();
        assert_eq!(error, TemporaryLinkError::InvalidBaudRate);
    }
}
