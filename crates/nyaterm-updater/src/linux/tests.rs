use super::{
    Authorization, CommandOutput, InstallOutcome, OUTPUT_LIMIT, PreparedPackage, drain,
    run_command, validate_metadata,
};
use crate::download::InstallationTarget;
use nyaterm_core::secret::SecretString;
use nyaterm_core::updater::UpdatePackageKind;
use std::os::unix::process::ExitStatusExt as _;
use std::path::PathBuf;
use std::process::{Command, ExitStatus};

fn package(kind: UpdatePackageKind) -> PreparedPackage {
    PreparedPackage {
        artifact: PathBuf::from("/tmp/update with spaces/package.deb"),
        installation: InstallationTarget {
            package: kind,
            executable: PathBuf::from("/opt/nyaterm/nyaterm"),
        },
        version: "2.0.9".into(),
        signature: "invalid".into(),
    }
}

fn output(code: i32) -> CommandOutput {
    CommandOutput {
        status: ExitStatus::from_raw(code << 8),
        stdout: String::new(),
        stderr: String::new(),
    }
}

#[test]
fn metadata_checks_package_identity_version_architecture_and_compatibility() {
    assert!(
        validate_metadata(
            UpdatePackageKind::LinuxDeb,
            "2.0.9",
            "x86_64",
            &format!(
                "{}\n2.0.9\namd64\n",
                super::package_name(UpdatePackageKind::LinuxDeb)
            )
        )
        .is_ok()
    );
    assert!(
        validate_metadata(
            UpdatePackageKind::LinuxDebGlibc231,
            "2.0.9",
            "x86_64",
            "nyaterm-compat\n2.0.9-1+glibc231\namd64\n"
        )
        .is_ok()
    );
    assert!(
        validate_metadata(
            UpdatePackageKind::LinuxRpm,
            "2.0.9",
            "aarch64",
            &format!(
                "{}\n2.0.9-1\naarch64\n",
                super::package_name(UpdatePackageKind::LinuxRpm)
            )
        )
        .is_ok()
    );
    for metadata in [
        "other\n2.0.9\namd64\n",
        "nyaterm\n2.0.8\namd64\n",
        "nyaterm\n2.0.9\narm64\n",
        "nyaterm\n2.0.9",
    ] {
        assert!(
            validate_metadata(UpdatePackageKind::LinuxDeb, "2.0.9", "x86_64", metadata).is_err()
        );
    }
}

#[test]
fn invalid_signature_prevents_authorization_and_installation() {
    assert!(matches!(
        package(UpdatePackageKind::LinuxDeb).install(Authorization::Polkit),
        InstallOutcome::PreflightFailed(_)
    ));
}

#[test]
fn cancelling_polkit_stops_without_trying_sudo() {
    let mut commands = Vec::new();
    let result = package(UpdatePackageKind::LinuxDeb).install_with(
        Authorization::Polkit,
        &mut |command, secret| {
            assert!(secret.is_none());
            commands.push(command.get_program().to_owned());
            Ok(output(if commands.len() == 1 { 0 } else { 126 }))
        },
    );
    assert_eq!(result, InstallOutcome::Cancelled);
    assert_eq!(commands.len(), 2);
    assert!(commands[0].to_string_lossy().ends_with("apt-get"));
    assert!(commands[1].to_string_lossy().ends_with("pkexec"));
}

#[test]
fn dependency_preflight_failure_never_prompts_for_authorization() {
    let mut count = 0;
    let result =
        package(UpdatePackageKind::LinuxDeb).install_with(Authorization::Polkit, &mut |_, _| {
            count += 1;
            let mut result = output(1);
            result.stderr = "missing dependency".into();
            Ok(result)
        });
    assert_eq!(count, 1);
    assert_eq!(
        result,
        InstallOutcome::PreflightFailed("missing dependency".into())
    );
}

#[test]
fn sudo_password_is_passed_to_one_install_process_and_preserves_whitespace() {
    let mut count = 0;
    let result = package(UpdatePackageKind::LinuxDeb).install_with(
        Authorization::Sudo(SecretString::from("  password  ")),
        &mut |command, secret| {
            count += 1;
            let args: Vec<_> = command
                .get_args()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            assert!(args.iter().all(|arg| !arg.contains("password")));
            if count == 2 {
                assert_eq!(secret.unwrap().expose_secret(), "  password  ");
                assert_eq!(&args[..5], ["-S", "-k", "-p", "", "--"]);
                assert_eq!(args.last().unwrap(), "/tmp/update with spaces/package.deb");
            } else {
                assert!(secret.is_none());
            }
            Ok(output(0))
        },
    );
    assert_eq!(count, 2);
    assert_eq!(result, InstallOutcome::Installed);
}

#[test]
fn incorrect_sudo_password_reports_authorization_failure_without_exposing_diagnostics() {
    let mut count = 0;
    let result = package(UpdatePackageKind::LinuxDeb).install_with(
        Authorization::Sudo(SecretString::from("bad")),
        &mut |_, _| {
            count += 1;
            let mut result = output(if count == 1 { 0 } else { 1 });
            result.stderr = "sudo: private authentication diagnostic".into();
            Ok(result)
        },
    );
    assert_eq!(count, 2);
    assert_eq!(result, InstallOutcome::AuthorizationFailed);
}

#[test]
fn rpm_preflight_and_install_do_not_force_dependencies_or_downgrades() {
    let mut count = 0;
    let result = package(UpdatePackageKind::LinuxRpm).install_with(
        Authorization::Polkit,
        &mut |command, _| {
            count += 1;
            let args: Vec<_> = command
                .get_args()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect();
            assert!(args.contains(&"-U".into()));
            assert_eq!(args.contains(&"--test".into()), count == 1);
            assert!(!args.iter().any(|arg| arg.contains("force")
                || arg.contains("nodeps")
                || arg.contains("oldpackage")));
            Ok(output(if count == 1 { 0 } else { 1 }))
        },
    );
    assert!(matches!(result, InstallOutcome::Failed(_)));
}

#[test]
fn output_is_bounded_but_reader_consumes_the_entire_stream() {
    let data = vec![b'a'; OUTPUT_LIMIT * 8];
    let mut reader = std::io::Cursor::new(&data);
    assert_eq!(drain(&mut reader).unwrap().len(), OUTPUT_LIMIT);
    assert_eq!(reader.position(), data.len() as u64);
}

#[test]
fn both_process_pipes_are_drained_even_after_diagnostic_limit() {
    let mut command = Command::new("sh");
    command.args([
        "-c",
        "head -c 262144 /dev/zero; head -c 262144 /dev/zero >&2",
    ]);
    let result = run_command(&mut command, None).unwrap();
    assert!(result.status.success());
    assert_eq!(result.stdout.len(), OUTPUT_LIMIT);
    assert_eq!(result.stderr.len(), OUTPUT_LIMIT);
}

#[test]
fn dependency_simulation_rejects_extra_packages_removals_and_truncated_output() {
    assert!(super::deb_transaction_is_self_contained(
        "Inst nyaterm (2.0.9 local)\nConf nyaterm (2.0.9 local)\n",
        "nyaterm"
    ));
    for output in [
        "Inst lib-new (1.0 repo)",
        "Remv old-library",
        "Conf broken-package (1.0 local)",
    ] {
        assert!(!super::deb_transaction_is_self_contained(output, "nyaterm"));
    }
    assert!(!super::deb_transaction_is_self_contained(
        &"x".repeat(OUTPUT_LIMIT),
        "nyaterm"
    ));
}
