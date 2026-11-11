//! Current prompt evidence, independent of the program used to launch a session.

use crate::action_links::ActionLinkPlatform;
use crate::completion::types::ShellDialect;
use crate::terminal::editing::ShellLineEditing;
use crate::terminal::input_tracker::known_terminal_prompt_prefix;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShellPrompt {
    pub dialect: Option<ShellDialect>,
    pub line_editing: ShellLineEditing,
    pub platform: Option<ActionLinkPlatform>,
}

/// Classify an exact prompt prefix, never arbitrary output or command text.
/// This identifies an encoding; callers still need confirmed input and their
/// foreground-program policy before allowing destructive edits.
pub fn classify_shell_prompt(prompt: &str) -> Option<ShellPrompt> {
    let prompt = prompt.trim_start();
    let prefix = known_terminal_prompt_prefix(prompt)?;
    if prefix.trim_end() != prompt.trim_end() {
        return None;
    }
    if let Some(path) = prefix.strip_prefix("PS ") {
        let path = path.trim_start().trim_end_matches(['>', ' ']);
        let platform = if windows_path(path) {
            Some(ActionLinkPlatform::Windows)
        } else if path.starts_with('/') || path.starts_with('~') {
            Some(ActionLinkPlatform::Posix)
        } else {
            // PowerShell providers and custom locations do not identify an OS.
            None
        };
        Some(ShellPrompt {
            dialect: Some(ShellDialect::PowerShell),
            line_editing: ShellLineEditing::AnsiLineEditor,
            platform,
        })
    } else if windows_path(prefix) {
        Some(ShellPrompt {
            dialect: Some(ShellDialect::Cmd),
            line_editing: ShellLineEditing::LegacyConsole,
            platform: Some(ActionLinkPlatform::Windows),
        })
    } else {
        Some(ShellPrompt {
            dialect: None,
            line_editing: ShellLineEditing::AnsiLineEditor,
            platform: Some(ActionLinkPlatform::Posix),
        })
    }
}

fn windows_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
        || path.starts_with("\\\\")
}

#[cfg(test)]
mod tests {
    use crate::action_links::ActionLinkPlatform;
    use crate::terminal::editing::ShellLineEditing;
    use crate::terminal::shell_prompt::classify_shell_prompt;

    #[test]
    fn prompt_evidence_distinguishes_nested_windows_editors_and_posix_powershell() {
        for (prompt, line_editing, platform) in [
            (
                "C:\\Users>",
                ShellLineEditing::LegacyConsole,
                Some(ActionLinkPlatform::Windows),
            ),
            (
                "PS C:\\Users> ",
                ShellLineEditing::AnsiLineEditor,
                Some(ActionLinkPlatform::Windows),
            ),
            (
                "PS /home/user> ",
                ShellLineEditing::AnsiLineEditor,
                Some(ActionLinkPlatform::Posix),
            ),
            (
                "PS HKLM:\\Software> ",
                ShellLineEditing::AnsiLineEditor,
                None,
            ),
            (
                "user@host:~$ ",
                ShellLineEditing::AnsiLineEditor,
                Some(ActionLinkPlatform::Posix),
            ),
        ] {
            let evidence = classify_shell_prompt(prompt).unwrap();
            assert_eq!(evidence.line_editing, line_editing);
            assert_eq!(evidence.platform, platform);
        }
    }

    #[test]
    fn commands_credentials_and_repl_prompts_are_not_editor_evidence() {
        for text in [
            "C:\\Users>dir",
            "PS /tmp> echo",
            "Password: ",
            ">>> ",
            "mysql> ",
            "custom> ",
            "",
        ] {
            assert_eq!(classify_shell_prompt(text), None);
        }
    }
}
