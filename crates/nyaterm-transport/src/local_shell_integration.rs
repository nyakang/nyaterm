//! Local shell integration: local PTYs receive the same prompt script SSH
//! uploads, so POSIX shells under Windows (WSL, Git Bash) report OSC 7 working
//! directories and OSC 133 command boundaries without rc-file setup.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::local_shell::ResolvedLocalShell;
use super::ssh_shell_integration::{
    PreparedSshShellIntegration, ShellIntegrationMode, ShellKind, SshIntegrationOutput,
    SshShellIntegrationState, build_legacy_ssh_ready_marker, build_ssh_ready_marker,
    prepare_shell_integration,
};
use crate::{TransportWriterCommand, WriterMailbox};

/// Mirrors the SSH initial delay so the shell prompt is reading stdin before
/// the script arrives.
const LOCAL_INJECT_DELAY: Duration = Duration::from_millis(500);

/// WSL cold boot (VM start, distro init) can take well over the SSH window;
/// without the wider window the suppression expires first and the injected
/// script echo becomes visible in the terminal.
const LOCAL_INTEGRATION_TIMEOUT: Duration = Duration::from_secs(30);

/// ConPTY keeps a virtual screen and repaints with absolute cursor positions.
/// The suppressed echo leaves rows behind in that screen that never reach the
/// emulator, so reset the screen while suppression still hides it; otherwise
/// later prompts are painted far below the last visible line.
const LOCAL_SCREEN_RESET: &str = "printf '\\033[2J\\033[H'; ";

const READY_MARKER_PRINTF: &str = "printf '\\033]7777;NyaTermReady:";

#[derive(Clone)]
pub(super) struct LocalShellIntegration {
    state: Arc<Mutex<SshShellIntegrationState>>,
    integration: PreparedSshShellIntegration,
}

impl LocalShellIntegration {
    pub(super) fn prepare(shell: &ResolvedLocalShell, session_id: &str) -> Option<Self> {
        let kind = local_shell_kind(shell)?;
        let ready_marker = build_ssh_ready_marker(session_id);
        let legacy_ready_marker = build_legacy_ssh_ready_marker(&ready_marker);
        let integration =
            prepare_shell_integration(kind, &ready_marker, ShellIntegrationMode::Full)?;
        let mut state = SshShellIntegrationState::new(
            Some(integration.clone()),
            ready_marker,
            legacy_ready_marker,
        );
        state.set_integration_timeout(LOCAL_INTEGRATION_TIMEOUT);
        state.enable_control_passthrough();
        // A warm shell prints its first prompt before the injection lands; start
        // suppressing at session creation so that prompt cannot leak.
        state.begin_suppression();
        Some(Self {
            state: Arc::new(Mutex::new(state)),
            integration,
        })
    }

    pub(super) fn filter(&self, bytes: &[u8]) -> SshIntegrationOutput {
        self.lock().filter_output(bytes)
    }

    /// Detached worker: session teardown never waits for the injection delay.
    pub(super) fn spawn_injector(&self, mailbox: WriterMailbox, session_id: &str) {
        let integration = self.clone();
        let worker_name = format!("transport-local-injection-{session_id}");
        let _ = nyaterm_core::runtime::worker::spawn(worker_name, move || {
            std::thread::sleep(LOCAL_INJECT_DELAY);
            integration.inject_now(mailbox);
        });
    }

    pub(super) fn inject_now(&self, mailbox: WriterMailbox) {
        let script = String::from_utf8(self.integration.inline_script())
            .expect("prepared shell integration script is UTF-8");
        // PTY canonical input ends a line on CR (ICRNL), the same encoding Enter
        // keystrokes use; bare LF bytes depend on console input translation.
        let script = script.replace("\r\n", "\r").replace('\n', "\r");
        let script = insert_screen_reset(script);
        {
            let mut state = self.lock();
            state.take_pending_integration();
            state.begin_suppression();
        }
        if let Err(error) = mailbox.send(TransportWriterCommand::Write(script.into_bytes())) {
            tracing::warn!(%error, "local shell integration injection write failed");
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, SshShellIntegrationState> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }
}

fn insert_screen_reset(mut script: String) -> String {
    if let Some(index) = script.rfind(READY_MARKER_PRINTF) {
        script.insert_str(index, LOCAL_SCREEN_RESET);
    }
    script
}

/// Shells whose prompt scripts NyaTerm ships; `None` keeps today's behavior
/// (raw output, no markers).
fn local_shell_kind(shell: &ResolvedLocalShell) -> Option<ShellKind> {
    let program = shell
        .program
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(&shell.program)
        .to_ascii_lowercase();
    let file_name = program.strip_suffix(".exe").unwrap_or(&program);
    // The WSL launcher starts the distro default shell, normally bash.
    if file_name == "wsl" {
        return Some(ShellKind::Bash);
    }
    match ShellKind::from_name(file_name) {
        kind @ (ShellKind::Bash | ShellKind::Zsh | ShellKind::Fish) => Some(kind),
        ShellKind::PosixSh | ShellKind::Unknown => None,
    }
}

#[cfg(test)]
mod tests {
    use base64::Engine as _;

    use super::{
        LOCAL_INTEGRATION_TIMEOUT, LocalShellIntegration, ResolvedLocalShell, ShellKind,
        TransportWriterCommand, WriterMailbox, local_shell_kind,
    };
    use crate::ssh_shell_integration::SSH_INTEGRATION_TIMEOUT;

    fn shell(program: &str) -> ResolvedLocalShell {
        ResolvedLocalShell {
            program: program.to_string(),
            args: Vec::new(),
        }
    }

    #[test]
    fn injectable_shell_detection_covers_launchers_and_posix_paths() {
        assert_eq!(local_shell_kind(&shell("wsl.exe")), Some(ShellKind::Bash));
        assert_eq!(
            local_shell_kind(&shell(r"C:\Program Files\WSL\wsl.exe")),
            Some(ShellKind::Bash)
        );
        assert_eq!(local_shell_kind(&shell("bash.exe")), Some(ShellKind::Bash));
        assert_eq!(
            local_shell_kind(&shell("/usr/bin/zsh")),
            Some(ShellKind::Zsh)
        );
        assert_eq!(local_shell_kind(&shell("fish.exe")), Some(ShellKind::Fish));
        assert_eq!(local_shell_kind(&shell("powershell.exe")), None);
        assert_eq!(local_shell_kind(&shell("pwsh")), None);
        assert_eq!(local_shell_kind(&shell("cmd.exe")), None);
        assert_eq!(local_shell_kind(&shell("/bin/sh")), None);
    }

    #[test]
    fn injection_suppresses_startup_noise_until_the_ready_marker_reports_cwd() {
        let integration = LocalShellIntegration::prepare(&shell("wsl.exe"), "session-1").unwrap();

        let output = integration.filter(b"before\r\n");
        assert!(output.visible.is_empty());

        let mailbox = WriterMailbox::default();
        integration.inject_now(mailbox.clone());
        let (command, _permit) = mailbox.receive(None).unwrap().unwrap();
        let TransportWriterCommand::Write(script) = command else {
            panic!("injection writes a shell script");
        };
        let script = String::from_utf8(script).unwrap();
        assert!(script.contains("NyaTermReady:session-1"));
        assert!(script.contains("file://"));
        assert!(!script.contains('\n'));

        let output = integration.filter(b"shell startup noise");
        assert!(output.visible.is_empty());

        let output = integration.filter(b"prompt\x1b]7777;NyaTermReady:session-1\x07> ");
        assert_eq!(output.visible, b"> ");

        let output = integration.filter(b"\x1b]7;file://host/mnt/d/Codes\x07dir>");
        assert_eq!(output.cwd_paths, ["/mnt/d/Codes"]);
        assert_eq!(output.visible, b"dir>");

        let encoded = base64::engine::general_purpose::STANDARD.encode("ls -la");
        let output =
            integration.filter(format!("\x1b]7777;NyaTermCommand:{encoded}\x07").as_bytes());
        assert_eq!(output.accepted_commands, ["ls -la"]);
    }

    #[test]
    fn uninjectable_shells_keep_raw_local_output() {
        assert!(LocalShellIntegration::prepare(&shell("cmd.exe"), "session-2").is_none());
        assert!(LocalShellIntegration::prepare(&shell("powershell.exe"), "session-2").is_none());
    }

    #[test]
    fn suppression_forwards_terminal_queries_to_the_emulator() {
        let integration = LocalShellIntegration::prepare(&shell("wsl.exe"), "session-4").unwrap();

        let output = integration.filter(b"\x1b[6nstartup noise");
        assert_eq!(output.visible, b"\x1b[6n");
        assert!(output.cwd_paths.is_empty());
    }

    #[test]
    fn injection_resets_the_conpty_screen_before_the_ready_marker() {
        let integration = LocalShellIntegration::prepare(&shell("wsl.exe"), "session-6").unwrap();

        let mailbox = WriterMailbox::default();
        integration.inject_now(mailbox.clone());
        let (command, _permit) = mailbox.receive(None).unwrap().unwrap();
        let TransportWriterCommand::Write(script) = command else {
            panic!("injection writes a shell script");
        };
        let script = String::from_utf8(script).unwrap();

        let reset = script.find("\\033[2J").expect("screen reset");
        let ready = script
            .rfind("NyaTermReady:session-6")
            .expect("ready marker");
        assert!(reset < ready);
    }

    #[test]
    fn local_suppression_outlives_wsl_cold_boot() {
        let integration = LocalShellIntegration::prepare(&shell("wsl.exe"), "session-3").unwrap();
        let timeout = integration.lock().integration_timeout();
        assert_eq!(timeout, LOCAL_INTEGRATION_TIMEOUT);
        assert!(timeout > SSH_INTEGRATION_TIMEOUT);
    }
}
