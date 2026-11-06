//! Conservative lexical adapters. No expansion or shell execution occurs here.
use crate::completion::types::{CompletionContext, CompletionScope, ShellDialect};
use crate::terminal::input_tracker::MAX_TRACKED_INPUT_BYTES;

#[derive(Clone)]
pub(crate) struct Word {
    pub text: String,
    pub start: usize,
    pub end: usize,
    pub segment: usize,
    pub quoted: bool,
}

pub(crate) fn words(input: &str, dialect: ShellDialect) -> Option<Vec<Word>> {
    if dialect == ShellDialect::Unknown
        || input.len() > MAX_TRACKED_INPUT_BYTES
        || input.chars().any(char::is_control)
    {
        return None;
    }
    let mut out = Vec::new();
    let mut text = String::new();
    let mut start = None;
    let mut quote = None;
    let mut quoted = false;
    let mut escaped = false;
    let mut segment = 0;
    let mut iter = input.char_indices().peekable();
    while let Some((i, ch)) = iter.next() {
        if escaped {
            if (dialect == ShellDialect::PowerShell
                && matches!(
                    ch,
                    '0' | 'a' | 'b' | 'e' | 'f' | 'n' | 'r' | 't' | 'v' | 'u'
                ))
                || (dialect == ShellDialect::Fish
                    && quote.is_none()
                    && (ch.is_ascii_alphabetic() || ch.is_ascii_digit()))
            {
                return None;
            }
            text.push(ch);
            escaped = false;
            continue;
        }
        if let Some(q) = quote {
            if ch == q {
                if dialect == ShellDialect::PowerShell && iter.peek().is_some_and(|(_, c)| *c == q)
                {
                    iter.next();
                    text.push(ch);
                } else {
                    quote = None;
                }
            } else if (q == '"'
                && (ch == '$'
                    || ch == '`' && matches!(dialect, ShellDialect::Bash | ShellDialect::Zsh))
                && dialect != ShellDialect::Cmd)
                || (dialect == ShellDialect::Cmd && matches!(ch, '%' | '!'))
            {
                return None;
            } else if is_escape(ch, dialect) && dialect != ShellDialect::Cmd {
                let next = iter.peek().map(|(_, ch)| *ch);
                let escapes = match dialect {
                    ShellDialect::Bash | ShellDialect::Zsh => {
                        q == '"' && next.is_some_and(|ch| matches!(ch, '$' | '`' | '"' | '\\'))
                    }
                    ShellDialect::Fish => {
                        next.is_some_and(|ch| ch == '\\' || ch == q || q == '"' && ch == '$')
                    }
                    ShellDialect::PowerShell => q == '"',
                    _ => false,
                };
                if escapes {
                    escaped = true;
                } else {
                    text.push(ch);
                }
            } else {
                text.push(ch);
            }
            continue;
        }
        if is_escape(ch, dialect) {
            start.get_or_insert(i);
            escaped = true;
            quoted = true;
            continue;
        }
        if ch == '"' || (ch == '\'' && dialect != ShellDialect::Cmd) {
            start.get_or_insert(i);
            quote = Some(ch);
            quoted = true;
            continue;
        }
        if (matches!(
            dialect,
            ShellDialect::Bash | ShellDialect::Zsh | ShellDialect::Fish
        ) && (matches!(ch, '*' | '?' | '[' | ']') || ch == '~' && start.is_none()))
            || (dialect == ShellDialect::PowerShell && ch == '@' && start.is_none())
        {
            return None;
        }
        if matches!(ch, '$' | '(' | ')' | '{' | '}' | '<' | '>') && dialect != ShellDialect::Cmd
            || (dialect == ShellDialect::Cmd && matches!(ch, '%' | '!' | '(' | ')' | '<' | '>'))
            || (ch == '`' && !matches!(dialect, ShellDialect::PowerShell | ShellDialect::Cmd))
        {
            return None;
        }
        if ch == '#' && dialect != ShellDialect::Cmd && start.is_none() {
            return None;
        }
        if ch == '&'
            && dialect == ShellDialect::PowerShell
            && start.is_none()
            && out
                .iter()
                .all(|word: &Word| word.segment != segment || word.text.is_empty())
            && !iter.peek().is_some_and(|(_, c)| *c == '&')
        {
            continue;
        }
        let separator = matches!(ch, '|' | '&') || (ch == ';' && dialect != ShellDialect::Cmd);
        if ch.is_whitespace() || separator {
            if let Some(s) = start.take() {
                out.push(Word {
                    text: std::mem::take(&mut text),
                    start: s,
                    end: i,
                    segment,
                    quoted,
                });
                quoted = false;
            }
            if separator {
                if ch == '&'
                    && !iter.peek().is_some_and(|(_, c)| *c == '&')
                    && dialect != ShellDialect::Cmd
                {
                    return None;
                }
                if iter.peek().is_some_and(|(_, c)| *c == ch) {
                    iter.next();
                }
                segment += 1;
                // Keep an empty boundary so a trailing pipe starts a new command.
                out.push(Word {
                    text: String::new(),
                    start: i + ch.len_utf8(),
                    end: i + ch.len_utf8(),
                    segment,
                    quoted: false,
                });
            }
        } else {
            start.get_or_insert(i);
            text.push(ch);
        }
    }
    if quote.is_some() || escaped {
        return None;
    }
    if let Some(s) = start {
        out.push(Word {
            text,
            start: s,
            end: input.len(),
            segment,
            quoted,
        });
    }
    let mut previous_segment = None;
    for word in &out {
        if word.text.is_empty() {
            continue;
        }
        if previous_segment != Some(word.segment) {
            if dialect == ShellDialect::PowerShell && word.quoted {
                let before = input[..word.start].trim_end();
                if !before
                    .strip_suffix('&')
                    .is_some_and(|rest| !rest.trim_end().ends_with('&'))
                {
                    return None;
                }
            }
            let name = word.text.to_ascii_lowercase();
            let structural = match dialect {
                ShellDialect::Bash | ShellDialect::Zsh | ShellDialect::Fish => matches!(
                    name.as_str(),
                    "if" | "then"
                        | "else"
                        | "elif"
                        | "fi"
                        | "for"
                        | "while"
                        | "until"
                        | "case"
                        | "esac"
                        | "function"
                        | "begin"
                        | "end"
                        | "do"
                        | "done"
                ),
                ShellDialect::PowerShell => matches!(
                    name.as_str(),
                    "if" | "else"
                        | "foreach"
                        | "for"
                        | "while"
                        | "switch"
                        | "function"
                        | "try"
                        | "catch"
                        | "class"
                        | "do"
                ),
                ShellDialect::Cmd => matches!(name.as_str(), "if" | "for"),
                ShellDialect::Unknown => true,
            };
            if structural && !word.quoted {
                return None;
            }
            previous_segment = Some(word.segment);
        } else if dialect == ShellDialect::PowerShell && word.quoted && word.text.starts_with('-') {
            return None;
        }
    }
    Some(out)
}
fn is_escape(ch: char, dialect: ShellDialect) -> bool {
    match dialect {
        ShellDialect::PowerShell => ch == '`',
        ShellDialect::Cmd => ch == '^',
        _ => ch == '\\',
    }
}

pub(crate) fn unwrap_words(tokens: &mut Vec<String>, dialect: ShellDialect) -> Option<()> {
    if !matches!(
        dialect,
        ShellDialect::Bash | ShellDialect::Zsh | ShellDialect::Fish
    ) {
        return Some(());
    }
    let mut skip = 0;
    while let Some(token) = tokens.get(skip) {
        if token.contains('=') {
            skip += 1;
            continue;
        }
        match token.as_str() {
            "sudo" | "doas" => {
                skip += 1;
                while let Some(flag) = tokens.get(skip).filter(|t| t.starts_with('-')) {
                    if flag == "--" {
                        skip += 1;
                        break;
                    }
                    if matches!(
                        flag.as_str(),
                        "-u" | "-g"
                            | "-h"
                            | "-p"
                            | "-C"
                            | "-T"
                            | "--user"
                            | "--group"
                            | "--host"
                            | "--prompt"
                    ) {
                        skip += 2;
                    } else if matches!(
                        flag.as_str(),
                        "-n" | "-E" | "-H" | "-S" | "--non-interactive"
                    ) {
                        skip += 1;
                    } else {
                        return None;
                    }
                }
            }
            "and" | "or" | "not" if dialect == ShellDialect::Fish => {
                skip += 1;
            }
            "time" => {
                skip += 1;
                if tokens.get(skip).is_some_and(|t| t == "-p") {
                    skip += 1;
                }
                if tokens.get(skip).is_some_and(|t| t.starts_with('-')) {
                    return None;
                }
            }
            "env" | "command" | "nohup" | "builtin" => {
                skip += 1;
                if tokens.get(skip).is_some_and(|t| t.starts_with('-')) {
                    return None;
                }
            }
            _ => break,
        }
    }
    if skip > tokens.len() {
        return None;
    }
    tokens.drain(..skip);
    Some(())
}

pub fn parse_context(
    input: &str,
    cursor: usize,
    dialect: ShellDialect,
    scope: CompletionScope,
) -> Option<CompletionContext> {
    if cursor != input.len() || !input.is_char_boundary(cursor) {
        return None;
    }
    let all = words(input, dialect)?;
    let segment = all.last().map_or(0, |w| w.segment);
    let mut active: Vec<_> = all
        .into_iter()
        .filter(|w| w.segment == segment && (!w.text.is_empty() || w.quoted))
        .collect();
    let current = active
        .last()
        .filter(|w| w.end == cursor && !input.ends_with(char::is_whitespace))
        .cloned();
    if current.as_ref().is_some_and(|w| w.quoted) {
        return None;
    }
    if current.is_some() {
        active.pop();
    }
    let mut preceding: Vec<String> = active.into_iter().map(|w| w.text).collect();
    unwrap_words(&mut preceding, dialect)?;
    let (mut token_range, mut prefix) = current.map_or((cursor..cursor, String::new()), |w| {
        (w.start..cursor, w.text)
    });
    let mut option_value = None;
    let inline_value = prefix
        .split_once('=')
        .filter(|(flag, _)| flag.starts_with("--") && !preceding.iter().any(|word| word == "--"))
        .or_else(|| {
            (dialect == ShellDialect::PowerShell
                && !preceding.iter().any(|word| word == "--")
                && preceding
                    .first()
                    .is_some_and(|root| dialect.native_options(root))
                && prefix.starts_with('-'))
            .then(|| prefix.split_once(':'))
            .flatten()
        });
    if let Some((name, value)) = inline_value {
        let name = name.to_string();
        let value = value.to_string();
        token_range.start += name.len() + 1;
        preceding.push(name.clone());
        option_value = Some(name);
        prefix = value;
    }
    // An assignment being edited cannot be interpreted as an executable name.
    if preceding.is_empty() && prefix.contains('=') {
        return None;
    }
    Some(CompletionContext {
        input: input.into(),
        cursor,
        token_range,
        prefix,
        command_path: Vec::new(),
        option_value,
        preceding,
        dialect,
        scope,
    })
}

#[cfg(test)]
mod tests {
    use crate::completion::parser::parse_context;
    use crate::completion::types::{CompletionScope, ShellDialect};
    #[test]
    fn preserves_byte_ranges_and_command_boundaries() {
        for dialect in [
            ShellDialect::Bash,
            ShellDialect::Zsh,
            ShellDialect::Fish,
            ShellDialect::PowerShell,
            ShellDialect::Cmd,
        ] {
            let input = "echo 中文 | docker r";
            let context =
                parse_context(input, input.len(), dialect, CompletionScope::legacy()).unwrap();
            assert_eq!(context.preceding, ["docker"]);
            assert_eq!(&input[context.token_range], "r");
            let input = "docker ";
            assert_eq!(
                parse_context(input, input.len(), dialect, CompletionScope::legacy())
                    .unwrap()
                    .prefix,
                ""
            );
        }
    }
    #[test]
    fn rejects_uncertain_expansion_and_current_quotes() {
        for (input, dialect) in [
            ("echo $HOME", ShellDialect::Bash),
            ("echo %PATH%", ShellDialect::Cmd),
            ("echo !PATH!", ShellDialect::Cmd),
            ("echo `", ShellDialect::PowerShell),
            ("echo 'ab'", ShellDialect::Bash),
        ] {
            assert!(
                parse_context(input, input.len(), dialect, CompletionScope::legacy()).is_none()
            );
        }
        let input = "sudo -u root docker ru";
        assert_eq!(
            parse_context(
                input,
                input.len(),
                ShellDialect::Bash,
                CompletionScope::legacy()
            )
            .unwrap()
            .preceding,
            ["docker"]
        );
    }
    #[test]
    fn parses_dialect_escapes_quotes_and_chains_without_losing_byte_ranges() {
        for (dialect, input, preceding) in [
            (
                ShellDialect::Bash,
                r"echo 'a b' && sudo -n docker r",
                vec!["docker"],
            ),
            (ShellDialect::Zsh, r"echo a\ b; docker r", vec!["docker"]),
            (ShellDialect::Fish, r"echo 'a b'; docker r", vec!["docker"]),
            (
                ShellDialect::PowerShell,
                "Write-Output 'a''b'; Get-Process -N",
                vec!["Get-Process"],
            ),
            (ShellDialect::Cmd, r"echo a^&b & DIR /", vec!["DIR"]),
        ] {
            let context =
                parse_context(input, input.len(), dialect, CompletionScope::legacy()).unwrap();
            assert_eq!(context.preceding, preceding);
            assert_eq!(context.prefix, input[context.token_range.clone()]);
            assert_eq!(context.cursor, input.len());
        }
        for (dialect, input) in [
            (ShellDialect::Bash, "tool $(cmd) ar"),
            (ShellDialect::Zsh, "tool ${value} ar"),
            (ShellDialect::Fish, "tool (cmd) ar"),
            (ShellDialect::PowerShell, "Get-Process $env:Name"),
            (ShellDialect::Cmd, "echo %Name% ar"),
        ] {
            assert!(
                parse_context(input, input.len(), dialect, CompletionScope::legacy()).is_none()
            );
        }
    }
    #[test]
    fn unwraps_fish_conditions_and_powershell_invocation_but_rejects_structures() {
        for (dialect, input) in [
            (ShellDialect::Fish, "echo ready; and not docker r"),
            (ShellDialect::PowerShell, "& docker r"),
            (ShellDialect::PowerShell, "Write-Output ready; & 'docker' r"),
        ] {
            let context =
                parse_context(input, input.len(), dialect, CompletionScope::legacy()).unwrap();
            assert_eq!(context.preceding, ["docker"]);
            assert_eq!(&input[context.token_range], "r");
        }
        for (dialect, input) in [
            (ShellDialect::Bash, "if docker r"),
            (ShellDialect::Zsh, "for docker r"),
            (ShellDialect::Fish, "begin docker r"),
            (ShellDialect::PowerShell, "foreach docker r"),
            (ShellDialect::Cmd, "if docker r"),
        ] {
            assert!(
                parse_context(input, input.len(), dialect, CompletionScope::legacy()).is_none()
            );
        }
    }
    #[test]
    fn completed_quotes_preserve_literal_escapes_and_reject_substitution_or_controls() {
        for dialect in [ShellDialect::Bash, ShellDialect::Zsh, ShellDialect::Fish] {
            let input = r#"tool "a\q" r"#;
            let context =
                parse_context(input, input.len(), dialect, CompletionScope::legacy()).unwrap();
            assert_eq!(context.preceding, ["tool", r"a\q"]);
        }
        let input = r#"tool "a^b" r"#;
        let context = parse_context(
            input,
            input.len(),
            ShellDialect::Cmd,
            CompletionScope::legacy(),
        )
        .unwrap();
        assert_eq!(context.preceding, ["tool", "a^b"]);
        for (dialect, input) in [
            (ShellDialect::Bash, r#"tool "`command`" r"#),
            (ShellDialect::Zsh, r#"tool "`command`" r"#),
            (ShellDialect::PowerShell, r#"tool "a`nb" r"#),
            (ShellDialect::Fish, r"tool a\nb r"),
            (ShellDialect::Bash, "tool *.rs r"),
            (ShellDialect::Zsh, "tool ~/path r"),
            (ShellDialect::Fish, "tool [ab] r"),
            (ShellDialect::PowerShell, "Get-Process @parameters r"),
            (ShellDialect::PowerShell, "'Get-Process' r"),
            (ShellDialect::PowerShell, "Get-Content '-file' r"),
        ] {
            assert!(
                parse_context(input, input.len(), dialect, CompletionScope::legacy()).is_none()
            );
        }
    }
    #[test]
    fn completed_empty_quoted_arguments_still_occupy_a_value_position() {
        for dialect in [
            ShellDialect::Bash,
            ShellDialect::Zsh,
            ShellDialect::Fish,
            ShellDialect::PowerShell,
            ShellDialect::Cmd,
        ] {
            let input = "tool --mode \"\" --next";
            let context =
                parse_context(input, input.len(), dialect, CompletionScope::legacy()).unwrap();
            assert_eq!(context.preceding, ["tool", "--mode", ""]);
            assert_eq!(&input[context.token_range], "--next");
        }
    }
}
