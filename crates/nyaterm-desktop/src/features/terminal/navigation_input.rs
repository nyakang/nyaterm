//! Session-owned navigation evidence; it grants no shell editing capability.

use nyaterm_terminal::command_navigation::{
    CommandSubmissionEvidence, command_submission_evidence, visible_shell_prompt,
};
use nyaterm_terminal::{TerminalLineId, TerminalSnapshot};
use zeroize::Zeroize;

use crate::features::NyaTermApp;
use crate::models::SessionLaunchConfig;

#[derive(Default)]
pub(super) struct CommandNavigationInputState {
    mode: NavigationInputMode,
}

#[derive(Default)]
enum NavigationInputMode {
    #[default]
    Shell,
    Interactive(PromptExpectation),
    AwaitingPrompt(PromptExpectation),
}

struct PromptExpectation {
    prompt: Option<String>,
    submitted_line: TerminalLineId,
}

impl Drop for PromptExpectation {
    fn drop(&mut self) {
        if let Some(prompt) = self.prompt.as_mut() {
            prompt.zeroize();
        }
    }
}

impl CommandNavigationInputState {
    pub(super) fn observe(&mut self, snapshot: &TerminalSnapshot) {
        let expected = match &self.mode {
            NavigationInputMode::Shell => return,
            NavigationInputMode::Interactive(expected)
            | NavigationInputMode::AwaitingPrompt(expected) => expected,
        };
        if let Some((line, prompt)) = visible_shell_prompt(snapshot)
            && line != expected.submitted_line
            && expected.prompt.as_ref() == Some(&prompt)
        {
            self.mode = NavigationInputMode::Shell;
        }
    }

    pub(super) fn command_finished(&mut self) {
        self.mode = NavigationInputMode::Shell;
    }

    pub(super) fn note_input(&mut self, text: &str) {
        if matches!(text, "\u{3}" | "\u{4}" | "q" | "exit\r" | "quit\r") {
            let mode = std::mem::take(&mut self.mode);
            self.mode = match mode {
                NavigationInputMode::Interactive(expected) => {
                    NavigationInputMode::AwaitingPrompt(expected)
                }
                other => other,
            };
        }
    }

    pub(super) fn accept_submission(
        &mut self,
        evidence: &CommandSubmissionEvidence,
    ) -> Option<TerminalLineId> {
        if !matches!(self.mode, NavigationInputMode::Shell) {
            return None;
        }
        if nyaterm_core::command_starts_interactive_input(&evidence.command) {
            self.mode = NavigationInputMode::Interactive(PromptExpectation {
                prompt: evidence.prompt.clone(),
                submitted_line: evidence.anchor,
            });
        }
        Some(evidence.anchor)
    }
}

impl NyaTermApp {
    pub(in crate::features) fn prepare_command_navigation_input(
        &mut self,
        bytes: &[u8],
    ) -> Option<CommandSubmissionEvidence> {
        let text = std::str::from_utf8(bytes).ok()?;
        if !text.ends_with(['\r', '\n']) {
            return None;
        }
        let id = self.session.active_id_owned()?;
        if self.active_terminal_uses_alternate_screen()
            || self.is_credential_prompt_input_mode()
            || self.terminal.assist.credential_suggestions.is_some()
            || self.security.screen_locked()
            || self.mcp.capabilities.terminal_executions.capturing(&id)
            || !self.session.metadata(&id).is_some_and(|metadata| {
                matches!(
                    metadata.launch_config,
                    SessionLaunchConfig::Local(_) | SessionLaunchConfig::Ssh(_)
                )
            })
        {
            return None;
        }
        // Take position evidence before the transport can deliver the Enter echo.
        self.terminal.editing.activate(&id);
        let snapshot = self.terminal_snapshot_for_session(Some(&id), 0);
        self.terminal
            .editing
            .state_mut()
            .command_navigation
            .observe(&snapshot);
        command_submission_evidence(&snapshot, self.terminal.editing.predicted_input(), bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::CommandNavigationInputState;
    use nyaterm_core::terminal::input_tracker::TerminalInputState;
    use nyaterm_terminal::TerminalCore;
    use nyaterm_terminal::command_navigation::command_submission_evidence;

    #[test]
    fn cancellation_waits_for_outer_prompt_and_failed_launch_also_recovers() {
        for cancel in [None, Some("\u{3}"), Some("q")] {
            let mut terminal = TerminalCore::new(40, 5);
            terminal.advance(b"PS C:\\work> redis-cli");
            let evidence = command_submission_evidence(
                &terminal.snapshot(),
                &TerminalInputState::new(),
                b"\r",
            )
            .unwrap();
            let mut state = CommandNavigationInputState::default();
            assert!(state.accept_submission(&evidence).is_some());
            if let Some(cancel) = cancel {
                state.note_input(cancel);
            }
            state.observe(&terminal.snapshot());
            assert!(state.accept_submission(&evidence).is_none());
            terminal.advance(b"\r\n127.0.0.1:6379> echo inside");
            state.observe(&terminal.snapshot());
            assert!(state.accept_submission(&evidence).is_none());
            terminal.advance(b"\r\nPS C:\\work> echo outside");
            state.observe(&terminal.snapshot());
            let outside = command_submission_evidence(
                &terminal.snapshot(),
                &TerminalInputState::new(),
                b"\r",
            )
            .unwrap();
            assert!(state.accept_submission(&outside).is_some());
        }
    }

    #[test]
    fn shell_completion_edge_releases_interactive_suppression() {
        let mut terminal = TerminalCore::new(40, 5);
        terminal.advance(b"$ redis-cli");
        let evidence =
            command_submission_evidence(&terminal.snapshot(), &TerminalInputState::new(), b"\r")
                .unwrap();
        let mut state = CommandNavigationInputState::default();
        state.accept_submission(&evidence);
        state.command_finished();
        assert!(state.accept_submission(&evidence).is_some());
    }

    #[test]
    fn navigation_interactive_suppression_survives_an_editing_authorization_reset() {
        use crate::features::terminal::editing_state::TerminalEditingState;
        let mut terminal = TerminalCore::new(40, 5);
        terminal.advance(b"$ redis-cli");
        let evidence =
            command_submission_evidence(&terminal.snapshot(), &TerminalInputState::new(), b"\r")
                .unwrap();
        let mut editing = TerminalEditingState::default();
        editing.activate("s1");
        assert!(
            editing
                .state_mut()
                .command_navigation
                .accept_submission(&evidence)
                .is_some()
        );
        editing.clear_active();
        assert!(
            editing
                .state_mut()
                .command_navigation
                .accept_submission(&evidence)
                .is_none()
        );
        editing.state_mut().command_navigation.command_finished();
        assert!(
            editing
                .state_mut()
                .command_navigation
                .accept_submission(&evidence)
                .is_some()
        );
    }
    #[test]
    fn ssh_configuration_does_not_disable_unmarked_fallback_and_credentials_still_block_it() {
        use crate::features::test_support::app_with_visible_local_session;
        use crate::models::SessionLaunchConfig;
        use crate::test_support::{TestAppCleanup, TestConfigDir};
        use gpui::{AppContext as _, TestAppContext};
        use nyaterm_transport::SshSessionConfig;
        let dir = TestConfigDir::new("nyaterm-navigation-fallback-eligibility");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "s1");
        cx.update_entity(&app, |app, _| {
            for enabled in [false, true] {
                app.session.metadata_mut("s1").unwrap().launch_config =
                    SessionLaunchConfig::Ssh(Box::new(SshSessionConfig {
                        terminal_shell_integration: enabled,
                        ..Default::default()
                    }));
                app.terminal
                    .seed_session_view("s1".into(), "$ echo unmarked".into(), "UTF-8");
                assert!(app.prepare_command_navigation_input(b"\r").is_some());
            }
            app.terminal.assist.credential_prompt_input_until_ms = u64::MAX;
            assert!(app.prepare_command_navigation_input(b"\r").is_none());
            app.terminal.assist.credential_prompt_input_until_ms = 0;
            app.terminal.seed_session_view(
                "s1".into(),
                "$ echo unmarked\x1b[?1049h".into(),
                "UTF-8",
            );
            assert!(app.prepare_command_navigation_input(b"\r").is_none());
        });
    }
}
