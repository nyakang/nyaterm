use std::net::IpAddr;
use std::str::FromStr as _;

use percent_encoding::percent_decode_str;
use thiserror::Error;
use url::{Host, Url};

use super::{ActivationRequest, RawActivationArg};
use crate::models::ssh_runtime::SshRuntimeMode;
use crate::secret::SecretString;
use zeroize::Zeroizing;

pub const MAX_DEEP_LINK_BYTES: usize = 4 * 1024;
pub const MAX_ACTIVATION_ACTIONS: usize = 16;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActivationAction {
    Activate,
    Connect(ExternalConnectionRequest),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExternalConnectionRequest {
    Local {
        cwd: Option<String>,
    },
    Ssh {
        host: String,
        port: u16,
        username: Option<String>,
        password: Option<SecretString>,
        runtime_mode: Option<SshRuntimeMode>,
    },
    Telnet {
        host: String,
        port: u16,
    },
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum ActivationParseError {
    #[error("too many activation arguments")]
    TooManyActions,
    #[error("activation argument is not valid text")]
    InvalidTextEncoding,
    #[error("activation argument is empty or contains unsafe text")]
    InvalidText,
    #[error("activation link exceeds the size limit")]
    LinkTooLong,
    #[error("activation link is not a valid URL")]
    InvalidUrl,
    #[error("activation link uses an unsupported scheme")]
    UnsupportedScheme,
    #[error("activation link contains credentials")]
    CredentialsNotAllowed,
    #[error("activation link contains an unsupported URL component")]
    UnsupportedComponent,
    #[error("activation link targets an unsupported action")]
    UnsupportedAction,
    #[error("activation link is missing a valid host")]
    InvalidHost,
    #[error("activation link contains an invalid username")]
    InvalidUsername,
    #[error("activation link contains an invalid port")]
    InvalidPort,
    #[error("activation link contains an invalid query parameter")]
    InvalidQuery,
    #[error("activation link repeats a query parameter")]
    DuplicateParameter,
    #[error("activation link contains an unknown query parameter")]
    UnknownParameter,
    #[error("invalid external invocation option")]
    InvalidOption,
    #[error("invalid SSH runtime mode")]
    InvalidMode,
}

pub fn parse_activation_request(
    request: &ActivationRequest,
) -> Result<Vec<ActivationAction>, ActivationParseError> {
    parse_activation_batch(request).into_iter().collect()
}

/// Parse independently so one rejected link cannot suppress other valid requests.
pub fn parse_activation_batch(
    request: &ActivationRequest,
) -> Vec<Result<ActivationAction, ActivationParseError>> {
    if request.args.len() > super::MAX_ACTIVATION_ARGS {
        return vec![Err(ActivationParseError::TooManyActions)];
    }
    let mut actions = Vec::new();
    let mut local = false;
    let mut reuse = false;
    let mut cwd = None;
    let mut local_error = None;
    let mut args = request.args.iter().peekable();
    while let Some(arg) = args.next() {
        let text = match activation_text(arg) {
            Ok(text) => text,
            Err(error) => {
                actions.push(Err(error));
                continue;
            }
        };
        match text.as_str() {
            "--reuse-window" => {
                if reuse {
                    actions.push(Err(ActivationParseError::DuplicateParameter));
                }
                reuse = true;
            }
            "--local" => {
                if local {
                    local_error = Some(ActivationParseError::DuplicateParameter);
                }
                local = true;
            }
            "--cwd" => {
                let value = if args.peek().is_some_and(|arg| {
                    activation_text(arg).is_ok_and(|text| text.starts_with("--"))
                }) {
                    Err(ActivationParseError::InvalidOption)
                } else {
                    args.next()
                        .ok_or(ActivationParseError::InvalidOption)
                        .and_then(activation_text)
                };
                match value {
                    Ok(value) if !value.starts_with("--") => {
                        if let Err(error) = set_once(&mut cwd, value.to_string()) {
                            local_error = Some(error);
                        }
                    }
                    _ => local_error = Some(ActivationParseError::InvalidOption),
                }
            }
            value if value.starts_with("--cwd=") => {
                if let Err(error) = set_once(&mut cwd, value[6..].to_string()) {
                    local_error = Some(error);
                }
            }
            _ => actions.push(parse_deep_link(&text)),
        }
    }
    if cwd.is_some() && !local {
        local_error = Some(ActivationParseError::InvalidOption);
    }
    if let Some(error) = local_error {
        actions.push(Err(error));
    } else if local {
        actions.push(
            validate_cwd(cwd)
                .map(|cwd| ActivationAction::Connect(ExternalConnectionRequest::Local { cwd })),
        );
    }
    if actions.len() > MAX_ACTIVATION_ACTIONS {
        actions.truncate(MAX_ACTIVATION_ACTIONS);
        actions.insert(0, Err(ActivationParseError::TooManyActions));
    }
    if actions.is_empty() {
        actions.push(Ok(ActivationAction::Activate));
    }
    actions
}

fn activation_text(arg: &RawActivationArg) -> Result<Zeroizing<String>, ActivationParseError> {
    let text = match arg {
        RawActivationArg::Bytes(bytes) => String::from_utf8(bytes.clone())
            .map_err(|_| ActivationParseError::InvalidTextEncoding)?,
        RawActivationArg::Wide(units) => {
            String::from_utf16(units).map_err(|_| ActivationParseError::InvalidTextEncoding)?
        }
    };
    if text.len() > MAX_DEEP_LINK_BYTES {
        return Err(ActivationParseError::LinkTooLong);
    }
    if text.is_empty() || text.chars().any(char::is_control) {
        return Err(ActivationParseError::InvalidText);
    }
    Ok(Zeroizing::new(text))
}

fn validate_cwd(cwd: Option<String>) -> Result<Option<String>, ActivationParseError> {
    if cwd
        .as_ref()
        .is_some_and(|value| value.trim().is_empty() || value.chars().any(char::is_control))
    {
        return Err(ActivationParseError::InvalidText);
    }
    Ok(cwd)
}

pub fn parse_deep_link(text: &str) -> Result<ActivationAction, ActivationParseError> {
    if text.len() > MAX_DEEP_LINK_BYTES {
        return Err(ActivationParseError::LinkTooLong);
    }
    if text.is_empty() || text.trim() != text || text.chars().any(char::is_control) {
        return Err(ActivationParseError::InvalidText);
    }
    let url = Url::parse(text).map_err(|_| ActivationParseError::InvalidUrl)?;
    if !text
        .split_once(':')
        .is_some_and(|(_, rest)| rest.starts_with("//"))
    {
        return Err(ActivationParseError::UnsupportedScheme);
    }
    match url.scheme() {
        "ssh" => parse_ssh_url(text, &url),
        "telnet" => parse_telnet_url(text, &url),
        "nyaterm" => parse_nyaterm_url(&url),
        "nyaterm-preview" => parse_nyaterm_url(&url),
        _ => Err(ActivationParseError::UnsupportedScheme),
    }
}

fn parse_ssh_url(text: &str, url: &Url) -> Result<ActivationAction, ActivationParseError> {
    reject_common_suffix(url)?;
    if !url.path().is_empty() {
        return Err(ActivationParseError::UnsupportedComponent);
    }
    let authority = authority(text, "ssh://")?;
    if authority.ends_with(':') {
        return Err(ActivationParseError::InvalidPort);
    }
    let password = authority
        .rsplit_once('@')
        .and_then(|(userinfo, _)| userinfo.split_once(':'))
        .map(|(_, password)| strict_percent_decode(password))
        .transpose()?
        .map(SecretString::new);
    let mut runtime_mode = None;
    if url.query().is_some() {
        for (key, value) in strict_query_pairs(url.query())? {
            if key != "mode" {
                return Err(ActivationParseError::UnknownParameter);
            }
            set_once(
                &mut runtime_mode,
                SshRuntimeMode::parse(&value).ok_or(ActivationParseError::InvalidMode)?,
            )?;
        }
    }
    let username = if authority.contains('@') {
        Some(validate_url_username(url.username())?)
    } else {
        None
    };
    let host = normalize_url_host(url)?;
    let port = url_port(url, 22)?;
    Ok(ActivationAction::Connect(ExternalConnectionRequest::Ssh {
        host,
        port,
        username,
        password,
        runtime_mode,
    }))
}

fn parse_telnet_url(text: &str, url: &Url) -> Result<ActivationAction, ActivationParseError> {
    reject_common_suffix(url)?;
    if !url.path().is_empty() || url.query().is_some() {
        return Err(ActivationParseError::UnsupportedComponent);
    }
    let authority = authority(text, "telnet://")?;
    if authority.ends_with(':') {
        return Err(ActivationParseError::InvalidPort);
    }
    if authority.contains('@') || !url.username().is_empty() || url.password().is_some() {
        return Err(ActivationParseError::CredentialsNotAllowed);
    }
    let host = normalize_url_host(url)?;
    let port = url_port(url, 23)?;
    Ok(ActivationAction::Connect(
        ExternalConnectionRequest::Telnet { host, port },
    ))
}

fn parse_nyaterm_url(url: &Url) -> Result<ActivationAction, ActivationParseError> {
    reject_common_suffix(url)?;
    if !url.username().is_empty() || url.password().is_some() || url.port().is_some() {
        return Err(ActivationParseError::CredentialsNotAllowed);
    }
    if !url
        .host_str()
        .is_some_and(|host| host.eq_ignore_ascii_case("connect"))
    {
        return Err(ActivationParseError::UnsupportedAction);
    }
    match url.path().to_ascii_lowercase().as_str() {
        "/ssh" => parse_nyaterm_ssh_query(url.query()),
        "/telnet" => parse_nyaterm_telnet_query(url.query()),
        "/local" => {
            let mut cwd = None;
            if url.query().is_some() {
                for (key, value) in strict_query_pairs(url.query())? {
                    if key != "cwd" {
                        return Err(ActivationParseError::UnknownParameter);
                    }
                    set_once(&mut cwd, value)?;
                }
            }
            Ok(ActivationAction::Connect(
                ExternalConnectionRequest::Local {
                    cwd: validate_cwd(cwd)?,
                },
            ))
        }
        _ => Err(ActivationParseError::UnsupportedAction),
    }
}

fn parse_nyaterm_ssh_query(query: Option<&str>) -> Result<ActivationAction, ActivationParseError> {
    let mut host = None;
    let mut port = None;
    let mut username = None;
    let mut runtime_mode = None;
    for (key, value) in strict_query_pairs(query)? {
        match key.as_str() {
            "host" => set_once(&mut host, normalize_external_host(&value)?)?,
            "port" => set_once(&mut port, parse_port(&value)?)?,
            "username" => set_once(&mut username, validate_query_username(value)?)?,
            "mode" => set_once(
                &mut runtime_mode,
                SshRuntimeMode::parse(&value).ok_or(ActivationParseError::InvalidMode)?,
            )?,
            _ => return Err(ActivationParseError::UnknownParameter),
        }
    }
    Ok(ActivationAction::Connect(ExternalConnectionRequest::Ssh {
        host: host.ok_or(ActivationParseError::InvalidHost)?,
        port: port.unwrap_or(22),
        username,
        password: None,
        runtime_mode,
    }))
}

fn parse_nyaterm_telnet_query(
    query: Option<&str>,
) -> Result<ActivationAction, ActivationParseError> {
    let mut host = None;
    let mut port = None;
    for (key, value) in strict_query_pairs(query)? {
        match key.as_str() {
            "host" => set_once(&mut host, normalize_external_host(&value)?)?,
            "port" => set_once(&mut port, parse_port(&value)?)?,
            _ => return Err(ActivationParseError::UnknownParameter),
        }
    }
    Ok(ActivationAction::Connect(
        ExternalConnectionRequest::Telnet {
            host: host.ok_or(ActivationParseError::InvalidHost)?,
            port: port.unwrap_or(23),
        },
    ))
}

fn reject_common_suffix(url: &Url) -> Result<(), ActivationParseError> {
    if url.fragment().is_some() {
        return Err(ActivationParseError::UnsupportedComponent);
    }
    Ok(())
}

fn authority<'a>(text: &'a str, prefix: &str) -> Result<&'a str, ActivationParseError> {
    let (scheme, rest) = text
        .split_once("://")
        .ok_or(ActivationParseError::UnsupportedScheme)?;
    if !scheme.eq_ignore_ascii_case(prefix.trim_end_matches("://")) {
        return Err(ActivationParseError::UnsupportedScheme);
    }
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    if authority.is_empty() {
        return Err(ActivationParseError::InvalidHost);
    }
    Ok(authority)
}

fn normalize_url_host(url: &Url) -> Result<String, ActivationParseError> {
    let host = url.host().ok_or(ActivationParseError::InvalidHost)?;
    normalize_host(host)
}

pub fn normalize_external_host(value: &str) -> Result<String, ActivationParseError> {
    validate_plain_text(value, 253).map_err(|_| ActivationParseError::InvalidHost)?;
    if value.contains(['/', '\\', '@', '?', '#']) {
        return Err(ActivationParseError::InvalidHost);
    }
    let value = value
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or(value);
    if let Ok(address) = IpAddr::from_str(value) {
        return Ok(address.to_string());
    }
    normalize_host(Host::parse(value).map_err(|_| ActivationParseError::InvalidHost)?)
}

fn normalize_host<S: AsRef<str>>(host: Host<S>) -> Result<String, ActivationParseError> {
    let value = match host {
        Host::Domain(domain) => domain.as_ref().to_lowercase(),
        Host::Ipv4(address) => address.to_string(),
        Host::Ipv6(address) => address.to_string(),
    };
    validate_plain_text(&value, 253).map_err(|_| ActivationParseError::InvalidHost)?;
    Ok(value)
}

fn validate_url_username(value: &str) -> Result<String, ActivationParseError> {
    validate_query_username(strict_percent_decode(value)?)
}

fn validate_query_username(value: String) -> Result<String, ActivationParseError> {
    validate_plain_text(&value, 128).map_err(|_| ActivationParseError::InvalidUsername)?;
    Ok(value)
}

fn validate_plain_text(value: &str, max_bytes: usize) -> Result<(), ()> {
    if value.is_empty()
        || value.len() > max_bytes
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        return Err(());
    }
    Ok(())
}

fn url_port(url: &Url, default: u16) -> Result<u16, ActivationParseError> {
    match url.port() {
        Some(0) => Err(ActivationParseError::InvalidPort),
        Some(port) => Ok(port),
        None => Ok(default),
    }
}

fn parse_port(value: &str) -> Result<u16, ActivationParseError> {
    value
        .parse::<u16>()
        .ok()
        .filter(|port| *port > 0)
        .ok_or(ActivationParseError::InvalidPort)
}

fn strict_query_pairs(query: Option<&str>) -> Result<Vec<(String, String)>, ActivationParseError> {
    let query = query.ok_or(ActivationParseError::InvalidQuery)?;
    if query.is_empty() {
        return Err(ActivationParseError::InvalidQuery);
    }
    query
        .split('&')
        .map(|pair| {
            let (key, value) = pair
                .split_once('=')
                .ok_or(ActivationParseError::InvalidQuery)?;
            if key.is_empty() || value.is_empty() || value.contains('+') {
                return Err(ActivationParseError::InvalidQuery);
            }
            Ok((strict_percent_decode(key)?, strict_percent_decode(value)?))
        })
        .collect()
}

fn strict_percent_decode(value: &str) -> Result<String, ActivationParseError> {
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len()
                || !bytes[index + 1].is_ascii_hexdigit()
                || !bytes[index + 2].is_ascii_hexdigit()
            {
                return Err(ActivationParseError::InvalidQuery);
            }
            index += 3;
        } else {
            index += 1;
        }
    }
    let decoded = percent_decode_str(value)
        .decode_utf8()
        .map_err(|_| ActivationParseError::InvalidQuery)?
        .into_owned();
    if decoded.chars().any(char::is_control) {
        return Err(ActivationParseError::InvalidQuery);
    }
    Ok(decoded)
}

fn set_once<T>(slot: &mut Option<T>, value: T) -> Result<(), ActivationParseError> {
    if slot.replace(value).is_some() {
        return Err(ActivationParseError::DuplicateParameter);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        ActivationAction, ActivationParseError, ExternalConnectionRequest, MAX_ACTIVATION_ACTIONS,
        parse_activation_request, parse_deep_link,
    };
    use crate::ActivationOpenBehavior;
    use crate::models::ssh_runtime::SshRuntimeMode;
    use crate::{ActivationRequest, RawActivationArg};

    fn invocation(args: &[&str]) -> ActivationRequest {
        ActivationRequest::from_os_args([9; 16], args.iter().map(std::ffi::OsString::from))
    }

    #[test]
    fn encoded_userinfo_is_decoded_once_and_never_debugged() {
        let action =
            parse_deep_link("SSH://domain%5Cuser%40realm:p%40ss%2521@[2001:db8::1]:2222?mode=sftp")
                .unwrap();
        assert!(!format!("{action:?}").contains("p@ss%21"));
        let ActivationAction::Connect(ExternalConnectionRequest::Ssh {
            host,
            port,
            username,
            password,
            runtime_mode,
        }) = action
        else {
            panic!("expected SSH");
        };
        assert_eq!(host, "2001:db8::1");
        assert_eq!(port, 2222);
        assert_eq!(username.as_deref(), Some("domain\\user@realm"));
        assert_eq!(password.unwrap().expose_secret(), "p@ss%21");
        assert_eq!(runtime_mode, Some(SshRuntimeMode::Sftp));
        let raw = invocation(&["ssh://user:do-not-log@example.com"]);
        assert!(!format!("{raw:?}").contains("do-not-log"));
        assert!(!format!("{:?}", raw.parse()).contains("do-not-log"));
        assert!(matches!(
            parse_deep_link("ssh://user:@host"),
            Ok(ActivationAction::Connect(ExternalConnectionRequest::Ssh {
                password: Some(_),
                ..
            }))
        ));
    }

    #[test]
    fn routing_and_execution_share_options_and_keep_valid_batch_items() {
        let parsed = invocation(&[
            "--reuse-window",
            "ssh://host",
            "ssh://host?command=id",
            "telnet://host",
        ])
        .parse();
        assert_eq!(
            parsed.open_behavior,
            ActivationOpenBehavior::ReuseMostRecent
        );
        assert_eq!(parsed.actions.len(), 3);
        assert!(parsed.actions[0].is_ok());
        assert!(parsed.actions[1].is_err());
        assert!(parsed.actions[2].is_ok());
        assert_eq!(
            invocation(&[]).parse().open_behavior,
            ActivationOpenBehavior::NewWindow
        );
        assert_eq!(
            invocation(&["ssh://host"]).parse().open_behavior,
            ActivationOpenBehavior::ReuseMostRecent
        );
        assert_eq!(
            invocation(&["--reuse-window"]).parse().actions,
            vec![Ok(ActivationAction::Activate)]
        );
    }

    #[test]
    fn local_invocation_preserves_spaces_unicode_and_literal_directory() {
        let expected = ActivationAction::Connect(ExternalConnectionRequest::Local {
            cwd: Some("C:\\中文 目录".into()),
        });
        assert_eq!(
            parse_activation_request(&invocation(&["--local", "--cwd", "C:\\中文 目录"])),
            Ok(vec![expected.clone()])
        );
        assert_eq!(
            parse_activation_request(&invocation(&[
                "--cwd=C:\\中文 目录",
                "--local",
                "--reuse-window"
            ])),
            Ok(vec![expected.clone()])
        );
        assert_eq!(
            parse_deep_link(
                "NYATERM://CONNECT/LOCAL?cwd=C%3A%5C%E4%B8%AD%E6%96%87%20%E7%9B%AE%E5%BD%95"
            ),
            Ok(expected)
        );
        for args in [
            vec!["--cwd", "x"],
            vec!["--local", "--cwd="],
            vec!["--local", "--cwd"],
            vec!["--cwd", "--local"],
            vec!["--local", "--local"],
            vec!["--local", "--cwd=x", "--cwd=y"],
            vec!["--reuse-window", "--reuse-window"],
        ] {
            assert!(parse_activation_request(&invocation(&args)).is_err());
        }
    }

    #[test]
    fn all_modes_and_strict_rejections_apply_to_both_ssh_url_forms() {
        for mode in ["standard", "terminal", "sftp"] {
            for link in [
                format!("ssh://host?mode={mode}"),
                format!("nyaterm-preview://connect/ssh?host=host&mode={mode}"),
            ] {
                assert!(matches!(
                    parse_deep_link(&link),
                    Ok(ActivationAction::Connect(ExternalConnectionRequest::Ssh {
                        runtime_mode: Some(_),
                        ..
                    }))
                ));
            }
        }
        for link in [
            "ssh://a:%FF@host",
            "ssh://a:%GG@host",
            "ssh://a%00@host",
            "ssh://host?mode=bad",
            "ssh://host?mode=sftp&mode=terminal",
            "ssh://host?proxy=x",
            "ssh://host?private_key=x",
            "ssh://host?forward=x",
            "nyaterm://connect/local?cwd=",
            "nyaterm://connect/local?command=id",
        ] {
            assert!(parse_deep_link(link).is_err());
        }
        assert!(matches!(
            parse_deep_link(&format!("ssh://{}", "x".repeat(super::MAX_DEEP_LINK_BYTES))),
            Err(ActivationParseError::LinkTooLong)
        ));
    }

    #[test]
    fn empty_request_activates_without_connecting() {
        let request = ActivationRequest {
            request_id: [1; 16],
            args: Vec::new(),
        };
        assert_eq!(
            parse_activation_request(&request),
            Ok(vec![ActivationAction::Activate])
        );
    }

    #[test]
    fn parses_strict_ssh_telnet_and_nyaterm_links() {
        assert_eq!(
            parse_deep_link("ssh://alice@example.com:2200"),
            Ok(ActivationAction::Connect(ExternalConnectionRequest::Ssh {
                host: "example.com".to_string(),
                port: 2200,
                username: Some("alice".to_string()),
                password: None,
                runtime_mode: None,
            }))
        );
        assert_eq!(
            parse_deep_link("telnet://[2001:db8::1]"),
            Ok(ActivationAction::Connect(
                ExternalConnectionRequest::Telnet {
                    host: "2001:db8::1".to_string(),
                    port: 23,
                }
            ))
        );
        assert_eq!(
            parse_deep_link("nyaterm://connect/ssh?host=example.com&port=22&username=deploy"),
            Ok(ActivationAction::Connect(ExternalConnectionRequest::Ssh {
                host: "example.com".to_string(),
                port: 22,
                username: Some("deploy".to_string()),
                password: None,
                runtime_mode: None,
            }))
        );
        assert_eq!(
            parse_deep_link("nyaterm://connect/telnet?host=example.com"),
            Ok(ActivationAction::Connect(
                ExternalConnectionRequest::Telnet {
                    host: "example.com".to_string(),
                    port: 23,
                }
            ))
        );
    }

    #[test]
    fn preview_scheme_reuses_connection_validation_without_relaxing_security() {
        for suffix in [
            "connect/ssh?host=example.com&port=2200&username=deploy",
            "connect/telnet?host=example.com",
        ] {
            assert_eq!(
                parse_deep_link(&format!("nyaterm-preview://{suffix}")),
                parse_deep_link(&format!("nyaterm://{suffix}"))
            );
            assert!(parse_deep_link(&format!("nyaterm-preview://{suffix}")).is_ok());
        }
        for suffix in [
            "user:password@connect/ssh?host=example.com",
            "connect/ssh?host=a&host=b",
            "connect/ssh?host=a&command=id",
            "connect/ssh?host=a&password=secret",
            "connect/ssh?host=%00example.com",
            "connect/ssh?host=%GG",
            "connect/ssh?host=example.com%2Fother",
            "connect/ssh?host=a&port=0",
            "connect/ssh?host=a&port=65536",
            "connect/rdp?host=a",
            "connect/telnet?host=a&username=user",
            "connect/ssh?host=a#fragment",
        ] {
            let stable = parse_deep_link(&format!("nyaterm://{suffix}"));
            assert!(stable.is_err());
            assert_eq!(
                parse_deep_link(&format!("nyaterm-preview://{suffix}")),
                stable
            );
        }
        assert!(parse_deep_link("NYATERM-PREVIEW://connect/ssh?host=a").is_ok());
    }

    #[test]
    fn raw_byte_and_wide_arguments_share_the_same_parser() {
        let expected = ActivationAction::Connect(ExternalConnectionRequest::Ssh {
            host: "example.com".to_string(),
            port: 22,
            username: None,
            password: None,
            runtime_mode: None,
        });
        for arg in [
            RawActivationArg::Bytes(b"ssh://example.com".to_vec()),
            RawActivationArg::Wide("ssh://example.com".encode_utf16().collect()),
        ] {
            let request = ActivationRequest {
                request_id: [2; 16],
                args: vec![arg],
            };
            assert_eq!(
                parse_activation_request(&request),
                Ok(vec![expected.clone()])
            );
        }
    }

    #[test]
    fn rejects_credentials_suffixes_and_noncanonical_schemes() {
        for link in [
            "telnet://alice@example.com",
            "ssh://example.com/command",
            "ssh://example.com?command=id",
            "ssh://example.com:0",
            "ssh://example.com:",
            "telnet://example.com:0",
            "telnet://example.com#fragment",
            "https://example.com",
            "nyaterm://user@connect/ssh?host=example.com",
        ] {
            assert!(parse_deep_link(link).is_err(), "accepted {link}");
        }
    }

    #[test]
    fn rejects_unknown_duplicate_and_unsafe_query_values() {
        assert_eq!(
            parse_deep_link("nyaterm://connect/ssh?host=a&host=b"),
            Err(ActivationParseError::DuplicateParameter)
        );
        for link in [
            "nyaterm://connect/ssh?host=a&password=secret",
            "nyaterm://connect/ssh?host=a&command=id",
            "nyaterm://connect/ssh?host=%00example.com",
            "nyaterm://connect/ssh?host=%GG",
            "nyaterm://connect/ssh?host=example.com%2Fother",
            "nyaterm://connect/rdp?host=example.com",
            "nyaterm://connect/telnet?host=a&username=user",
        ] {
            assert!(parse_deep_link(link).is_err(), "accepted {link}");
        }
    }

    #[test]
    fn rejects_invalid_os_text_and_action_floods() {
        let invalid_bytes = ActivationRequest {
            request_id: [3; 16],
            args: vec![RawActivationArg::Bytes(vec![0xff])],
        };
        assert_eq!(
            parse_activation_request(&invalid_bytes),
            Err(ActivationParseError::InvalidTextEncoding)
        );
        let invalid_wide = ActivationRequest {
            request_id: [4; 16],
            args: vec![RawActivationArg::Wide(vec![0xd800])],
        };
        assert_eq!(
            parse_activation_request(&invalid_wide),
            Err(ActivationParseError::InvalidTextEncoding)
        );
        let flooded = ActivationRequest {
            request_id: [5; 16],
            args: vec![
                RawActivationArg::Bytes(b"ssh://example.com".to_vec());
                MAX_ACTIVATION_ACTIONS + 1
            ],
        };
        assert_eq!(
            parse_activation_request(&flooded),
            Err(ActivationParseError::TooManyActions)
        );
    }
}
