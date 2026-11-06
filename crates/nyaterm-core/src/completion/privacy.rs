//! Conservative learning policy; this cannot identify arbitrary secrets in text.
use crate::terminal::input_tracker::MAX_TRACKED_INPUT_BYTES;

pub fn command_may_be_learned(command: &str, sensitive: bool) -> bool {
    if sensitive
        || command.is_empty()
        || command.len() > MAX_TRACKED_INPUT_BYTES
        || command.chars().any(char::is_control)
    {
        return false;
    }
    let lower = command.to_ascii_lowercase();
    if lower
        .split_whitespace()
        .any(|word| word.contains('=') && !word.starts_with("--"))
        || lower.contains("$env:")
        || lower.contains("-----begin")
    {
        return false;
    }
    if [
        "password",
        "passwd",
        "passphrase",
        "token",
        "secret",
        "api-key",
        "api_key",
        "apikey",
        "private-key",
        "private_key",
        "authorization",
        "bearer ",
        "cookie",
        "credential",
    ]
    .iter()
    .any(|word| lower.contains(word))
    {
        return false;
    }
    let tokens: Vec<_> = lower.split_whitespace().collect();
    if tokens.iter().any(|token| {
        token.rsplit('/').next() == Some("sshpass")
            || [
                "--key",
                "--access-key",
                "--auth",
                "--identity",
                "--identity-file",
                "--identity_file",
                "--privatekey",
                "--client-key",
                "-inkey",
                "-pass",
                "-pw",
            ]
            .iter()
            .any(|flag| token == flag || token.starts_with(&format!("{flag}=")))
    }) {
        return false;
    }
    let has_tool = |name| {
        tokens.iter().any(|token| {
            token
                .trim_matches(['\'', '"'])
                .rsplit(['/', '\\'])
                .next()
                .is_some_and(|base| base.strip_suffix(".exe").unwrap_or(base) == name)
        })
    };
    if ((has_tool("ssh") || has_tool("scp") || has_tool("sftp"))
        && tokens.iter().any(|token| token.starts_with("-i")))
        || (has_tool("curl")
            && tokens.iter().any(|token| {
                token.starts_with("-u")
                    || token.starts_with("--user")
                    || token.starts_with("--proxy-user")
            }))
        || (has_tool("docker")
            && tokens.contains(&"login")
            && tokens.iter().any(|token| token.starts_with("-p")))
        || ((has_tool("mysql") || has_tool("mariadb"))
            && tokens.iter().any(|token| token.starts_with("-p")))
        || lower.contains("passin")
        || lower.contains("passout")
    {
        return false;
    }
    for part in command.split_whitespace() {
        if let Some((_, rest)) = part.split_once("://")
            && rest
                .split('/')
                .next()
                .is_some_and(|authority| authority.contains('@'))
        {
            return false;
        }
    }
    true
}

pub(crate) fn bare_word(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 1024
        && value
            .chars()
            .all(|ch| ch.is_alphanumeric() || "-_.:/+@,%=\\".contains(ch))
}
pub(crate) fn safe_word(value: &str, dialect: crate::completion::types::ShellDialect) -> bool {
    bare_word(value)
        && match dialect {
            crate::completion::types::ShellDialect::Cmd => !value.contains('%'),
            crate::completion::types::ShellDialect::PowerShell => !value.starts_with('@'),
            _ => !value.contains('\\'),
        }
}

#[cfg(test)]
mod tests {
    use crate::completion::privacy::command_may_be_learned;
    #[test]
    fn rejects_secret_assignments_and_authentication_records() {
        for line in [
            "TOKEN=x tool",
            "curl -H 'Authorization: Bearer x' url",
            "tool --password x",
            "curl https://user:pass@example.test/a",
            "set X=y",
            "ssh -i /home/example/.ssh/id_rsa host",
            "scp -i/home/example/key file host:/path",
            "tool --identity-file /home/example/key",
            r#""/usr/bin/curl" -u user:pass https://example.test"#,
            "docker login -pattached",
        ] {
            assert!(!command_may_be_learned(line, false));
        }
        assert!(!command_may_be_learned("echo ordinary", true));
        assert!(command_may_be_learned("docker run -d redis", false));
        assert!(command_may_be_learned("cat /home/example/config", false));
    }
}
