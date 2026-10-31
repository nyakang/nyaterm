use crate::local_shell::ResolvedLocalShell;

use super::{
    LocalShellIntegration, interactive_args, startup_script, wsl_distro, wsl_distro_from_args,
    wsl_target,
};

#[cfg(unix)]
use std::io::Write;
#[cfg(unix)]
use std::path::Path;
#[cfg(unix)]
use std::process::{Command, Stdio};

fn shell(program: &str, args: &[&str]) -> ResolvedLocalShell {
    ResolvedLocalShell {
        program: program.into(),
        args: args.iter().map(|arg| (*arg).into()).collect(),
    }
}

#[cfg(unix)]
fn run_integrated_zsh(home: &Path, args: &[&str]) -> (String, String, String) {
    let (launch, integration) = LocalShellIntegration::prepare(&shell("zsh", args), "history")
        .expect("interactive zsh receives the startup script");
    let startup_dir = integration
        .startup_files
        .as_ref()
        .unwrap()
        .path()
        .to_owned();
    let mut child = Command::new(&launch.program)
        .args(&launch.args)
        .env_clear()
        .env("HOME", home)
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("TERM", "dumb")
        .current_dir(home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"print -r -- \"NYATERM_HISTFILE=${HISTFILE-}\"\nprint -r -- \"NYATERM_ZDOTDIR=${ZDOTDIR-}\"\nexit\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "zsh startup failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    drop(integration);
    assert!(!startup_dir.exists());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let value = |key: &str| {
        stdout
            .lines()
            .find_map(|line| line.split_once(key).map(|(_, value)| value.to_owned()))
            .unwrap_or_else(|| panic!("missing {key} in zsh output: {stdout:?}"))
    };
    (
        value("NYATERM_HISTFILE="),
        value("NYATERM_ZDOTDIR="),
        stdout,
    )
}

#[cfg(unix)]
#[test]
fn zsh_default_history_follows_user_zdotdir_for_login_and_non_login_shells() {
    if Command::new("zsh").arg("--version").output().is_err() {
        eprintln!("zsh is unavailable; skipping shell startup regression");
        return;
    }

    for custom_zdotdir in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let home = root.path();
        let config_dir = if custom_zdotdir {
            home.join("zsh-config")
        } else {
            home.to_owned()
        };
        std::fs::create_dir_all(&config_dir).unwrap();
        let zshenv = if custom_zdotdir {
            "print -r -- zshenv >> \"$HOME/rc-order\"\nHISTFILE=\"$NYATERM_LOCAL_DIR/.zsh_history\"\nZDOTDIR=\"$HOME/zsh-config\"\n"
        } else {
            "print -r -- zshenv >> \"$HOME/rc-order\"\nHISTFILE=\"$NYATERM_LOCAL_DIR/.zsh_history\"\n"
        };
        std::fs::write(home.join(".zshenv"), zshenv).unwrap();
        for rc in ["zprofile", "zshrc", "zlogin"] {
            let history_options = if rc == "zshrc" {
                "HISTSIZE=50\nSAVEHIST=50\n"
            } else {
                ""
            };
            std::fs::write(
                config_dir.join(format!(".{rc}")),
                format!("print -r -- {rc} >> \"$HOME/rc-order\"\n{history_options}"),
            )
            .unwrap();
        }

        for (args, expected_order) in [
            (&["-i"][..], "zshenv\nzshrc\n"),
            (&["-il"][..], "zshenv\nzprofile\nzshrc\nzlogin\n"),
        ] {
            let (history, zdotdir, output) = run_integrated_zsh(home, args);
            let history_file = config_dir.join(".zsh_history");
            assert_eq!(history, history_file.to_string_lossy());
            assert!(
                std::fs::read_to_string(history_file)
                    .unwrap()
                    .contains("NYATERM_HISTFILE="),
                "command history must be saved outside the temporary startup directory"
            );
            assert_eq!(
                zdotdir,
                if custom_zdotdir {
                    config_dir.to_str().unwrap()
                } else {
                    ""
                }
            );
            assert_eq!(
                std::fs::read_to_string(home.join("rc-order")).unwrap(),
                expected_order
            );
            assert!(output.contains("\x1b]7;file://localhost"));
            std::fs::remove_file(home.join("rc-order")).unwrap();
        }
    }
}

#[cfg(unix)]
#[test]
fn zsh_preserves_explicit_history_paths_and_user_rc_overrides() {
    if Command::new("zsh").arg("--version").output().is_err() {
        eprintln!("zsh is unavailable; skipping shell startup regression");
        return;
    }

    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    std::fs::write(
        home.join(".zshenv"),
        "HISTFILE=\"$HOME/explicit-history\"\n",
    )
    .unwrap();
    // macOS /etc/zshrc overrides .zshenv's HISTFILE before our .zshrc wrapper runs.
    let expected_history = if cfg!(target_os = "macos") {
        home.join(".zsh_history")
    } else {
        home.join("explicit-history")
    };
    assert_eq!(
        run_integrated_zsh(home, &["-i"]).0,
        expected_history.to_string_lossy()
    );

    std::fs::write(
        home.join(".zshenv"),
        "HISTFILE=\"$NYATERM_LOCAL_DIR/.zsh_history\"\n",
    )
    .unwrap();
    std::fs::write(home.join(".zshrc"), "HISTFILE=\"$HOME/rc-history\"\n").unwrap();
    assert_eq!(
        run_integrated_zsh(home, &["-i"]).0,
        home.join("rc-history").to_string_lossy()
    );

    std::fs::write(home.join(".zlogin"), "HISTFILE=\"$HOME/login-history\"\n").unwrap();
    assert_eq!(
        run_integrated_zsh(home, &["-il"]).0,
        home.join("login-history").to_string_lossy()
    );
}

#[cfg(target_os = "linux")]
#[test]
fn zsh_leaves_unset_or_empty_history_unmodified() {
    if Command::new("zsh").arg("--version").output().is_err() {
        eprintln!("zsh is unavailable; skipping shell startup regression");
        return;
    }

    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    for zshenv in ["unset HISTFILE\n", "HISTFILE=\"\"\n"] {
        std::fs::write(home.join(".zshenv"), zshenv).unwrap();
        assert_eq!(run_integrated_zsh(home, &["-i"]).0, "");
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
fn startup_script_fits_windows_command_line_and_enables_only_cwd() {
    let script = startup_script();
    assert!(script.len() < 12_000);
    assert!(!script.contains("133;"));
    assert!(!script.contains("NyaTermCommand"));
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
