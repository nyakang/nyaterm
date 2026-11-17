//! Translate host browser paths at the boundary back to the local shell namespace.

use nyaterm_transport::local_shell::LocalShellPathStyle;

/// Call only for paths from a Windows local browser, never for bare filenames or
/// remote paths. Guest paths retain their case and whitespace.
pub fn local_browser_path_to_shell_path(
    style: LocalShellPathStyle,
    path: &str,
    wsl_distro: Option<&str>,
) -> Option<String> {
    if path.is_empty() || path.chars().any(char::is_control) {
        return None;
    }
    if style == LocalShellPathStyle::Native {
        return Some(path.to_string());
    }
    let normalized = path.replace('\\', "/");
    let normalized = normalized.strip_prefix("//?/").unwrap_or(&normalized);
    let bytes = normalized.as_bytes();
    if bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'/' {
        let prefix = if style == LocalShellPathStyle::Wsl {
            "/mnt/"
        } else {
            "/"
        };
        return Some(format!(
            "{prefix}{}{}",
            (bytes[0] as char).to_ascii_lowercase(),
            &normalized[2..]
        ));
    }
    if style == LocalShellPathStyle::Msys {
        return Some(normalized.to_string());
    }
    let unc = normalized.strip_prefix("//").or_else(|| {
        normalized
            .get(..4)
            .filter(|prefix| prefix.eq_ignore_ascii_case("UNC/"))
            .map(|_| &normalized[4..])
    });
    if let Some(unc) = unc {
        let (host, rest) = unc.split_once('/')?;
        if !host.eq_ignore_ascii_case("wsl.localhost") && !host.eq_ignore_ascii_case("wsl$") {
            return None;
        }
        let (distro, guest) = rest.split_once('/').unwrap_or((rest, ""));
        // Dropping another distro's share prefix would silently address this
        // session's filesystem instead. An unknown target must fail closed too.
        if distro.is_empty() || !distro.eq_ignore_ascii_case(wsl_distro?) {
            return None;
        }
        return Some(format!("/{guest}"));
    }
    // Already a guest path. A Windows root-relative backslash path is ambiguous.
    (path.starts_with('/') && !path.starts_with("//")).then(|| path.to_string())
}

#[cfg(test)]
mod tests {
    use nyaterm_transport::local_shell::LocalShellPathStyle;

    use super::local_browser_path_to_shell_path;

    #[test]
    fn drive_paths_use_the_target_shell_namespace() {
        for (style, expected) in [
            (LocalShellPathStyle::Wsl, "/mnt/e/Projects/Rust/nya term"),
            (LocalShellPathStyle::Msys, "/e/Projects/Rust/nya term"),
            (LocalShellPathStyle::Native, r"E:\Projects\Rust\nya term"),
        ] {
            assert_eq!(
                local_browser_path_to_shell_path(style, r"E:\Projects\Rust\nya term", None),
                Some(expected.into())
            );
        }
        for path in ["E:/", r"\\?\E:\"] {
            assert_eq!(
                local_browser_path_to_shell_path(LocalShellPathStyle::Wsl, path, None),
                Some("/mnt/e/".into())
            );
        }
    }

    #[test]
    fn wsl_shares_require_the_session_distribution_and_preserve_guest_case() {
        for prefix in [
            r"\\wsl.localhost\Ubuntu",
            r"\\wsl$\ubuntu",
            r"\\?\UNC\WSL.LOCALHOST\UBUNTU",
        ] {
            for (suffix, expected) in [
                ("", "/"),
                (r"\root", "/root"),
                (r"\home\User\O'Brien ", "/home/User/O'Brien "),
            ] {
                let path = format!("{prefix}{suffix}");
                assert_eq!(
                    local_browser_path_to_shell_path(
                        LocalShellPathStyle::Wsl,
                        &path,
                        Some("Ubuntu")
                    ),
                    Some(expected.into())
                );
                for distro in [None, Some("Debian")] {
                    assert!(
                        local_browser_path_to_shell_path(LocalShellPathStyle::Wsl, &path, distro)
                            .is_none()
                    );
                }
            }
        }
    }

    #[test]
    fn wsl_rejects_unmappable_host_paths_without_reinterpreting_them() {
        for path in [
            r"\\server\share\file",
            r"C:relative",
            r"\root",
            "relative",
            "",
            "/tmp/x\nexit",
        ] {
            assert!(
                local_browser_path_to_shell_path(LocalShellPathStyle::Wsl, path, Some("Ubuntu"))
                    .is_none()
            );
        }
        assert_eq!(
            local_browser_path_to_shell_path(LocalShellPathStyle::Wsl, "/home/User/ file ", None),
            Some("/home/User/ file ".into())
        );
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "requires an installed WSL distribution with Bash"]
    fn wsl_bash_receives_browser_paths_as_single_literal_arguments() {
        use nyaterm_transport::directory_command::{DirectoryShell, quote_path_for_shell};
        use std::process::Command;

        let probe = Command::new("wsl.exe")
            .args(["--exec", "printenv", "WSL_DISTRO_NAME"])
            .output()
            .unwrap();
        assert!(probe.status.success(), "WSL distribution probe failed");
        let distro = String::from_utf8(probe.stdout).unwrap();
        let distro = distro.trim();
        assert!(!distro.is_empty());
        for (browser, expected) in [
            (
                r"E:\Projects\Rust\nyaterm".to_string(),
                "/mnt/e/Projects/Rust/nyaterm",
            ),
            (format!(r"\\wsl.localhost\{distro}\root"), "/root"),
            (
                format!(r"\\wsl$\{distro}\home\O'Brien $(exit 7); & notes "),
                "/home/O'Brien $(exit 7); & notes ",
            ),
        ] {
            let path =
                local_browser_path_to_shell_path(LocalShellPathStyle::Wsl, &browser, Some(distro))
                    .unwrap();
            let quoted = quote_path_for_shell(&path, DirectoryShell::Posix).unwrap();
            let script = format!(
                "expected=$1; set -- {quoted}; test \"$#\" -eq 1 && test \"$1\" = \"$expected\""
            );
            let result = Command::new("wsl.exe")
                .args([
                    "--distribution",
                    distro,
                    "--exec",
                    "bash",
                    "--noprofile",
                    "--norc",
                    "-c",
                    &script,
                    "nyaterm-path-test",
                    expected,
                ])
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "WSL path argument did not round trip"
            );
        }
    }
}
