//! Presentation-only prompt matching. These matches grant no editing capability.

use crate::terminal::input_tracker::prompt_matchers;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptFamily {
    Bracketed,
    UserHost,
    PowerShell,
    Cmd,
    HostPath,
    HostDirUser,
    ShellVersion,
    Path,
    LeadingArrow,
    Arrow,
    Bare,
}

pub struct PromptMatch<'a> {
    pub prefix: &'a str,
    pub family: PromptFamily,
    identity: &'a str,
    terminator: char,
}

impl PromptMatch<'_> {
    /// Compare parsed evidence without running either prompt regex again.
    pub fn same_shell(&self, other: &Self) -> bool {
        self.family == other.family
            && self.identity == other.identity
            && self.terminator == other.terminator
    }
}

/// Shares the input tracker's existing syntax without broadening its accepted prompts.
pub fn match_prompt(text: &str) -> Option<PromptMatch<'_>> {
    let (index, found) = prompt_matchers()
        .iter()
        .enumerate()
        .find_map(|(index, matcher)| matcher.find(text).map(|found| (index, found)))?;
    let prefix = &text[..found.end()];
    let trimmed = prefix.trim_end();
    let family = [
        PromptFamily::Bracketed,
        PromptFamily::UserHost,
        PromptFamily::PowerShell,
        PromptFamily::Cmd,
        PromptFamily::HostPath,
        PromptFamily::HostDirUser,
        PromptFamily::ShellVersion,
        PromptFamily::Path,
        PromptFamily::LeadingArrow,
        PromptFamily::Arrow,
        PromptFamily::Bare,
    ][index];
    let identity = match family {
        PromptFamily::UserHost | PromptFamily::HostPath => trimmed.split(':').next()?,
        // The user (with its terminator) is what a `su`/`sudo -i` changes.
        PromptFamily::HostDirUser => trimmed.rsplit(' ').next()?,
        PromptFamily::Bracketed => trimmed
            .strip_prefix('[')?
            .split(']')
            .next()?
            .split_whitespace()
            .next()?
            .split(':')
            .next()?,
        _ => "",
    };
    let terminator = if family == PromptFamily::LeadingArrow {
        trimmed.chars().next()?
    } else {
        trimmed.chars().last()?
    };
    Some(PromptMatch {
        prefix,
        family,
        identity,
        terminator,
    })
}

/// Directory changes are allowed, but shell family, identity and terminator must agree.
pub fn prompts_match(expected: &str, actual: &str) -> bool {
    let Some(expected) = match_prompt(expected) else {
        return false;
    };
    let Some(actual) = match_prompt(actual) else {
        return false;
    };
    expected.same_shell(&actual)
}

/// Conservative presentation fallback, not a shell parser or execution validator.
/// Open syntax and here-documents may echo additional input on later hard lines.
pub fn single_line_command_is_complete(command: &str) -> bool {
    let mut quote = None;
    let mut escaped = false;
    let mut groups = Vec::new();
    let mut chars = command.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch.is_control() {
            return false;
        }
        if escaped {
            escaped = false;
            continue;
        }
        if let Some(delimiter) = quote {
            if ch == delimiter {
                quote = None;
            } else if ch == '\\' && delimiter != '\'' {
                escaped = true;
            }
            continue;
        }
        match ch {
            '\'' | '"' => quote = Some(ch),
            '\\' => escaped = true,
            '`' => return false,
            '<' if chars.peek() == Some(&'<') => return false,
            '(' | '[' | '{' => groups.push(ch),
            ')' | ']' | '}' => {
                let expected = match ch {
                    ')' => '(',
                    ']' => '[',
                    _ => '{',
                };
                if groups.pop() != Some(expected) {
                    return false;
                }
            }
            _ => {}
        }
    }
    quote.is_none()
        && !escaped
        && groups.is_empty()
        && !command.trim_end().ends_with(['|', '&', '^'])
        && !matches!(
            command.split_whitespace().last(),
            Some("then" | "do" | "else")
        )
}

#[cfg(test)]
mod tests {
    use crate::terminal::input_tracker::known_terminal_prompt_prefix;
    use crate::terminal::prompt::{match_prompt, prompts_match, single_line_command_is_complete};

    #[test]
    fn continuation_and_here_document_syntax_cannot_authorize_an_output_range() {
        for command in [
            "echo 'open",
            "echo \"open",
            "echo \\",
            "cat <<EOF",
            "cat <<-EOF",
            "echo hi |",
            "echo hi &&",
            "if true; then",
            "for x in a b; do",
            "echo $(open",
            "function f {",
            "echo ^",
            "Write-Output `",
        ] {
            assert!(!single_line_command_is_complete(command), "{command}");
        }
        for command in [
            "echo hi",
            "echo 'closed'",
            "echo \"closed\"",
            "echo $(pwd)",
            "echo '<<literal'",
            "echo a; echo b",
        ] {
            assert!(single_line_command_is_complete(command), "{command}");
        }
    }

    #[test]
    fn presentation_matching_preserves_prefix_syntax() {
        for text in [
            "[user@host src]$ ls",
            "user@host:~$ ls",
            "PS C:\\src>dir",
            "C:\\src>dir",
            "device:/sdcard # ls",
            "Jakubs-MacBook-Pro:~ root# ls",
            "bash-3.2$ ls",
            "~/src ❯ ls",
            "➜ ~/src ls",
            "❯ ls",
            "$ ls",
            "Password: ",
            ">>> ",
            "mysql> ",
            "custom> ",
        ] {
            assert_eq!(
                match_prompt(text).map(|found| found.prefix),
                known_terminal_prompt_prefix(text)
            );
        }
    }

    #[test]
    fn prompt_identity_survives_directory_changes_without_accepting_other_shells() {
        for (before, after) in [
            ("user@host:~$ ", "user@host:/tmp$ "),
            ("[user@host src]$ ", "[user@host tmp]$ "),
            ("PS C:\\src> ", "PS D:\\tmp> "),
            ("C:\\src>", "D:\\tmp>"),
            ("device:/sdcard # ", "device:/data # "),
            ("host:~ jakub$ ", "host:/tmp jakub$ "),
            ("bash-3.2$ ", "bash-3.2$ "),
            ("~/src ❯ ", "/tmp ❯ "),
            ("➜ ~/src ", "➜ /tmp "),
            ("$ ", "$"),
        ] {
            assert!(prompts_match(before, after));
        }
        for (before, after) in [
            ("user@host:~$ ", "root@host:~$ "),
            ("user@host:~$ ", "user@other:~$ "),
            ("[user@host src]$ ", "[root@host src]$ "),
            ("$ ", "# "),
            ("host:~ jakub$ ", "host:~ root# "),
            ("bash-3.2$ ", "bash-3.2# "),
            ("❯ ", "➜ "),
            ("PS C:\\src> ", "C:\\src>"),
        ] {
            assert!(!prompts_match(before, after));
        }
    }
}
