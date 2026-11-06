use crate::local_shell::ResolvedLocalShell;

use super::{
    LocalShellIntegration, interactive_args, startup_script, wsl_distro, wsl_distro_from_args,
    wsl_target,
};

fn shell(program: &str, args: &[&str]) -> ResolvedLocalShell {
    ResolvedLocalShell {
        program: program.into(),
        args: args.iter().map(|arg| (*arg).into()).collect(),
    }
}

#[test]
fn commands_scripts_and_unknown_options_are_never_rewritten() {
    for (program, args) in [
        ("bash", vec!["-c", "cat"]),
        ("bash", vec!["-ic", "cat"]),
        ("bash", vec!["script.sh"]),
        ("bash", vec!["--", "script.sh"]),
        ("zsh", vec!["-f"]),
        ("fish", vec!["--command", "cat"]),
        ("fish", vec!["-C", "read"]),
        ("wsl.exe", vec!["--exec", "sh", "-c", "cat"]),
        ("wsl.exe", vec!["--exec", "bash", "-lc", "cat"]),
        ("wsl.exe", vec!["--list"]),
        ("wsl.exe", vec!["--distribution"]),
        ("wsl.exe", vec!["bash", "-c", "cat"]),
        ("mybash", vec![]),
        ("cmd.exe", vec![]),
    ] {
        assert!(
            LocalShellIntegration::prepare(&shell(program, &args), "test").is_none(),
            "{program}"
        );
    }
}

#[test]
fn startup_files_are_owned_before_spawn_and_removed_when_preparation_is_dropped() {
    let (launch, integration) =
        LocalShellIntegration::prepare(&shell("wsl.exe", &[]), "test").unwrap();
    let directory = integration
        .startup_files
        .as_ref()
        .unwrap()
        .path()
        .to_path_buf();
    assert!(directory.is_dir());
    assert!(
        launch
            .args
            .iter()
            .any(|arg| std::path::Path::new(arg) == directory)
    );
    drop(integration);
    assert!(!directory.exists());
}

#[test]
fn wsl_preserves_selectors_and_the_explicit_shell() {
    let args = shell(
        "wsl",
        &[
            "-d",
            "Test distro",
            "-u",
            "user",
            "--cd",
            "/tmp",
            "--exec",
            "/usr/bin/fish",
            "--no-config",
            "-i",
        ],
    )
    .args;
    let (prefix, target, flags) = wsl_target(&args).unwrap();
    assert_eq!(prefix, ["-d", "Test distro", "-u", "user", "--cd", "/tmp"]);
    assert_eq!(target, "/usr/bin/fish");
    assert_eq!(flags, ["--no-config", "-i"]);
    let (launch, _) = LocalShellIntegration::prepare(&shell("wsl.exe", &[]), "test").unwrap();
    assert_eq!(launch.program, "wsl.exe");
    assert_eq!(&launch.args[..3], ["--exec", "/bin/sh", "-c"]);
    assert_eq!(launch.args.last().unwrap(), "");
}

#[test]
fn known_interactive_flags_remain_supported() {
    for (kind, flags) in [
        ("bash", vec!["--noprofile", "--norc", "-i"]),
        ("bash", vec!["--login", "-i"]),
        ("zsh", vec!["-il"]),
        ("fish", vec!["--no-config", "--interactive"]),
    ] {
        assert!(interactive_args(kind, &shell(kind, &flags).args));
    }
}

#[test]
fn startup_prompts_errors_and_terminal_queries_are_visible_without_ready_marker() {
    let (_, mut integration) =
        LocalShellIntegration::prepare(&shell("wsl.exe", &[]), "test").unwrap();
    for data in [
        b"startup error\r\n".as_slice(),
        b"Continue? ",
        b"\x1b[6n",
        b"last output before EOF",
    ] {
        assert_eq!(integration.filter(data).visible, data);
    }
    let output = integration.filter(b"\x1b]7;file://localhost/tmp/My%20Files\x07prompt> ");
    assert_eq!(output.cwd_paths, ["/tmp/My Files"]);
    assert_eq!(output.visible, b"prompt> ");
}

#[test]
fn startup_script_fits_windows_command_line_and_sources_owned_command_hooks() {
    let script = startup_script();
    assert!(script.len() < 12_000);
    assert!(script.contains("full.bash"));
    assert!(script.contains("full.zsh"));
    assert!(script.contains("full.fish"));
    assert!(script.contains("NyaTermShell"));
    assert!(!script.contains('\r'));
}

#[test]
fn wsl_distro_uses_explicit_selectors_then_the_host_default() {
    assert_eq!(
        wsl_distro_from_args(&["-d".into(), "Ubuntu".into()]),
        Some("Ubuntu".into())
    );
    assert_eq!(
        wsl_distro_from_args(&["--distribution".into(), "Debian".into()]),
        Some("Debian".into())
    );
    assert_eq!(
        wsl_distro_from_args(&["--distribution=Arch".into()]),
        Some("Arch".into())
    );
    assert_eq!(
        wsl_distro_from_args(&["--cd".into(), "/mnt/c".into(), "-d".into(), "NixOS".into()]),
        Some("NixOS".into())
    );
    // Selector flags end at `--exec`; a missing or flag-like value is not a
    // distribution name.
    assert_eq!(
        wsl_distro_from_args(&["--exec".into(), "bash".into()]),
        None
    );
    assert_eq!(wsl_distro_from_args(&["-d".into()]), None);
    assert_eq!(
        wsl_distro_from_args(&["-d".into(), "-u".into(), "root".into()]),
        None
    );
    assert_eq!(wsl_distro_from_args(&[]), None);

    // Only WSL launchers carry a distribution.
    assert_eq!(wsl_distro(&shell("bash.exe", &[])), None);
    assert_eq!(wsl_distro(&shell("cmd.exe", &[])), None);
    assert_eq!(
        wsl_distro(&shell("wsl.exe", &["-d", "Ubuntu"])),
        Some("Ubuntu".into())
    );
}

#[test]
fn guest_identity_is_bounded_redacted_and_isolates_users_shells_and_distributions() {
    use base64::Engine;
    let (_, mut integration) =
        LocalShellIntegration::prepare(&shell("wsl.exe", &[]), "identity").unwrap();
    let encode = |text: &str| base64::engine::general_purpose::STANDARD.encode(text);
    let marker = format!(
        "\x1b]7777;NyaTermShell:{}:{}\x07",
        encode("/bin/zsh"),
        encode("guest-user")
    );
    let output = integration.filter(marker.as_bytes());
    assert!(output.visible.is_empty());
    let identity = output.shell_identities[0].clone();
    assert!(!format!("{identity:?}").contains("guest-user"));
    integration.completion_distro = Some("Distro A".into());
    let first = integration.completion_context(identity.clone());
    assert_eq!(
        first.dialect,
        nyaterm_core::completion::types::ShellDialect::Zsh
    );
    integration.completion_distro = Some("Distro B".into());
    assert_ne!(
        first.scope,
        integration.completion_context(identity.clone()).scope
    );
    let mut other = identity.clone();
    other.user = "other-user".into();
    assert_ne!(
        integration.completion_context(identity.clone()).scope,
        integration.completion_context(other).scope
    );
    let mut other = identity.clone();
    other.program = "/bin/fish".into();
    assert_ne!(
        integration.completion_context(identity).scope,
        integration.completion_context(other).scope
    );
    let bad = format!(
        "\x1b]7777;NyaTermShell:{}:{}\x07",
        encode("/bin/zsh\n"),
        encode("user")
    );
    let output = integration.filter(bad.as_bytes());
    assert!(output.visible.is_empty());
    assert!(output.shell_identities.is_empty());
}

#[cfg(unix)]
#[test]
fn real_local_bash_reports_actual_context_and_one_accepted_command() {
    assert_real_local_shell_context(
        "/bin/bash",
        &["--noprofile", "-i"],
        nyaterm_core::completion::types::ShellDialect::Bash,
    );
}

#[cfg(unix)]
#[test]
fn real_local_zsh_reports_actual_context_and_one_accepted_command() {
    assert_real_local_shell_context(
        "/bin/zsh",
        &["-i"],
        nyaterm_core::completion::types::ShellDialect::Zsh,
    );
}

#[cfg(unix)]
#[test]
fn real_local_fish_reports_actual_context_and_one_accepted_command() {
    assert_real_local_shell_context(
        "/usr/bin/fish",
        &["-i"],
        nyaterm_core::completion::types::ShellDialect::Fish,
    );
}

#[cfg(unix)]
fn assert_real_local_shell_context(
    program: &str,
    args: &[&str],
    dialect: nyaterm_core::completion::types::ShellDialect,
) {
    use crate::environment::{EnvironmentValue, ShellEnvironmentCache};
    use crate::{LocalSessionConfig, SessionEvent, SessionManager};
    use std::collections::HashMap;
    use std::time::{Duration, Instant};
    if !std::path::Path::new(program).is_file() {
        eprintln!("skipping live {dialect:?} fixture: shell not installed");
        return;
    }
    let home = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join(".bashrc"), "PS1='fixture> '\n").unwrap();
    std::fs::write(home.path().join(".zshrc"), "PROMPT='fixture> '\n").unwrap();
    // Ubuntu's global zshrc runs compinit before our prompt hooks. Its cold
    // completion scan (or security prompt) is unrelated to this isolated fixture.
    std::fs::write(home.path().join(".zshenv"), "skip_global_compinit=1\n").unwrap();
    let fish_config = home.path().join(".config/fish");
    std::fs::create_dir_all(&fish_config).unwrap();
    std::fs::write(
        fish_config.join("config.fish"),
        "function fish_prompt; printf 'fixture> '; end\n",
    )
    .unwrap();
    let mut manager = SessionManager::new();
    manager.shell_environment = ShellEnvironmentCache::with_values_for_test(
        program.into(),
        HashMap::from([
            (
                "HOME".into(),
                EnvironmentValue::new(home.path().to_string_lossy().into_owned()),
            ),
            ("BASH_ENV".into(), EnvironmentValue::new(String::new())),
            (
                "XDG_CONFIG_HOME".into(),
                EnvironmentValue::new(home.path().join(".config").to_string_lossy().into_owned()),
            ),
            (
                "ZDOTDIR".into(),
                EnvironmentValue::new(home.path().to_string_lossy().into_owned()),
            ),
            (
                "PROMPT_COMMAND".into(),
                EnvironmentValue::new(String::new()),
            ),
        ]),
    );
    let session = manager
        .create_local_session(LocalSessionConfig {
            shell_path: Some(program.into()),
            shell_args: args.iter().map(|arg| (*arg).into()).collect(),
            working_dir: Some(home.path().into()),
            ..Default::default()
        })
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut ready = false;
    let mut contexts = 0;
    let mut anchors = 0;
    let mut any_context = 0;
    let mut cwds = 0;
    let mut fixture_prompt = false;
    let mut errors = false;
    let mut terminal = nyaterm_terminal::TerminalCore::new(80, 24);
    while Instant::now() < deadline && !ready {
        for event in manager.drain_events(64).unwrap().events {
            match &event {
                SessionEvent::CommandContext { .. } => any_context += 1,
                SessionEvent::CwdChanged { .. } => cwds += 1,
                SessionEvent::Output { data, .. } => {
                    terminal.advance(data);
                    for reply in terminal.take_effects().pty_write {
                        manager.write_raw(&session.id, &reply).unwrap();
                    }
                    anchors += usize::from(data.windows(5).any(|w| w == b"133;A"));
                    fixture_prompt |= data.windows(8).any(|w| w == b"fixture>");
                    errors |= data.windows(12).any(|w| w == b"syntax error")
                        || data.windows(12).any(|w| w == b"No such file");
                }
                _ => {}
            }
            if let SessionEvent::CommandContext { context, .. } = event
                && context.command_events
                && context.dialect == dialect
            {
                contexts += 1;
            }
        }
        // Initial launch metadata is followed by the installed guest identity.
        ready = contexts >= 2;
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        ready,
        "installed {dialect:?} identity missing: contexts={contexts}, any={any_context}, cwd={cwds}, anchor={anchors}, fixture_prompt={fixture_prompt}, errors={errors}"
    );
    manager
        .write_raw(&session.id, b"printf fixture-command\r")
        .unwrap();
    let mut accepted = 0;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        for event in manager.drain_events(64).unwrap().events {
            if let SessionEvent::Output { data, .. } = &event {
                terminal.advance(data);
                for reply in terminal.take_effects().pty_write {
                    manager.write_raw(&session.id, &reply).unwrap();
                }
            }
            if let SessionEvent::CommandAccepted { command, .. } = event {
                assert_eq!(command.trim(), "printf fixture-command");
                accepted += 1;
            }
        }
        if accepted > 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    manager.close(&session.id).unwrap();
    assert_eq!(accepted, 1);
}
