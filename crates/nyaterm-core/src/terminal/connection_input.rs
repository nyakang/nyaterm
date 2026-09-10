//! Compatibility policies for saved terminal input and local shell arguments.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackspaceMode {
    Delete,
    Backspace,
}

impl BackspaceMode {
    pub fn parse(value: &str) -> Self {
        match value {
            "ctrl-h" | "ctrl_h" | "bs" => Self::Backspace,
            _ => Self::Delete,
        }
    }

    pub fn persistence_id(self) -> &'static str {
        match self {
            Self::Delete => "del",
            Self::Backspace => "ctrl_h",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("Unclosed quote in shell arguments")]
pub struct UnclosedShellQuote;

/// Parse the saved NyaTerm argument syntax without invoking a shell.
/// Backslashes in Windows paths are literal unless escaping a quote, slash or space.
pub fn parse_shell_args(input: &str) -> Result<Vec<String>, UnclosedShellQuote> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut chars = input.chars().peekable();
    let mut quote = None;
    let mut started = false;
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            let escapes = chars.peek().is_some_and(|next| match quote {
                Some(active) => *next == active || *next == '\\',
                None => next.is_whitespace() || matches!(next, '\'' | '"' | '\\'),
            });
            current.push(if escapes {
                chars.next().expect("peeked character")
            } else {
                ch
            });
            started = true;
        } else if let Some(active) = quote {
            if ch == active {
                quote = None;
            } else {
                current.push(ch);
            }
        } else if matches!(ch, '\'' | '"') {
            quote = Some(ch);
            started = true;
        } else if ch.is_whitespace() {
            if started {
                args.push(std::mem::take(&mut current));
                started = false;
            }
        } else {
            current.push(ch);
            started = true;
        }
    }
    if quote.is_some() {
        return Err(UnclosedShellQuote);
    }
    if started {
        args.push(current);
    }
    Ok(args)
}

#[cfg(test)]
mod tests {
    use super::{BackspaceMode, parse_shell_args};

    #[test]
    fn saved_backspace_aliases_have_one_wire_policy() {
        for alias in ["ctrl-h", "ctrl_h", "bs"] {
            assert_eq!(BackspaceMode::parse(alias), BackspaceMode::Backspace);
            assert_eq!(BackspaceMode::parse(alias).persistence_id(), "ctrl_h");
        }
        assert_eq!(BackspaceMode::parse("del"), BackspaceMode::Delete);
    }

    #[test]
    fn quoted_arguments_preserve_empty_values_and_windows_paths() {
        assert_eq!(
            parse_shell_args(r#"-Command "Write-Output 'hello world'" "" C:\Tools\pwsh.exe"#)
                .unwrap(),
            [
                "-Command",
                "Write-Output 'hello world'",
                "",
                r"C:\Tools\pwsh.exe"
            ]
        );
        assert_eq!(
            parse_shell_args("a\\ b '中文 文件' end\\").unwrap(),
            ["a b", "中文 文件", "end\\"]
        );
        assert!(parse_shell_args("-Command \"unfinished").is_err());
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum KeepaliveMode {
    Disabled,
    Strict,
    #[default]
    Compatible,
}

impl KeepaliveMode {
    pub fn parse(value: &str) -> Self {
        match value {
            "disabled" => Self::Disabled,
            "strict" => Self::Strict,
            _ => Self::Compatible,
        }
    }
}
