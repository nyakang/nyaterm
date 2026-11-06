use crate::remote_file::{shell_file_replace_command, shell_file_upload_command};
use crate::sftp::{SftpFileProperties, SftpFileType};

fn properties(uid: u32, gid: u32, mode: u32) -> SftpFileProperties {
    SftpFileProperties {
        symlink_target: None,
        name: String::new(),
        path: String::new(),
        file_type: SftpFileType::File,
        size: None,
        permissions: Some(mode),
        permissions_symbolic: String::new(),
        owner: String::new(),
        group: String::new(),
        uid: Some(uid),
        gid: Some(gid),
        modified_at: None,
        accessed_at: None,
        raw_path_token: None,
        symlink_target_is_directory: false,
    }
}

#[test]
fn shell_editor_save_refuses_missing_original_attributes() {
    for missing in ["uid", "gid", "mode"] {
        let mut attrs = properties(1000, 1001, 0o644);
        match missing {
            "uid" => attrs.uid = None,
            "gid" => attrs.gid = None,
            "mode" => attrs.permissions = None,
            _ => unreachable!(),
        }
        assert!(shell_file_replace_command(&attrs, "/tmp/edit", "/tmp/original").is_err());
        assert!(shell_file_upload_command(&attrs, "/tmp/edit", "/tmp/original", 3).is_err());
    }
}

#[cfg(unix)]
mod posix {
    use crate::remote_file::editor_save_tests::properties;
    use crate::remote_file::{shell_file_replace_command, shell_file_upload_command};
    use std::fs;
    use std::io::Write;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    const UPLOAD_BYTES: &[u8] = b"new\0data";

    fn run_replace(
        uid: u32,
        gid: u32,
        mode: u32,
        prefix: &str,
        as_unprivileged: bool,
        succeeds: bool,
    ) {
        run_save(uid, gid, mode, prefix, as_unprivileged, succeeds, None);
    }

    fn run_save(
        uid: u32,
        gid: u32,
        mode: u32,
        prefix: &str,
        as_unprivileged: bool,
        succeeds: bool,
        upload_size: Option<usize>,
    ) {
        run_save_with_paths(
            &properties(uid, gid, mode),
            prefix,
            as_unprivileged,
            succeeds,
            upload_size,
            false,
        );
    }

    fn run_save_with_paths(
        attrs: &crate::sftp::SftpFileProperties,
        prefix: &str,
        as_unprivileged: bool,
        succeeds: bool,
        upload_size: Option<usize>,
        relative_dash_paths: bool,
    ) {
        let (uid, gid, mode) = (
            attrs.uid.unwrap(),
            attrs.gid.unwrap(),
            attrs.permissions.unwrap(),
        );
        let directory = tempfile::tempdir().unwrap();
        // Quoting and glob expansion must be safe even for shell metacharacters.
        let original = directory.path().join("-original ' $file [*].txt");
        let temporary = directory.path().join("-temporary ' $file [*].txt");
        fs::write(&original, b"old").unwrap();
        fs::write(&temporary, b"new").unwrap();
        fs::set_permissions(&original, fs::Permissions::from_mode(mode)).unwrap();
        // SAFETY: geteuid only reads process identity.
        if unsafe { libc::geteuid() } == 0 {
            let status = Command::new("chown")
                .arg(format!("{uid}:{gid}"))
                .arg(&original)
                .status()
                .unwrap();
            assert!(status.success());
            fs::set_permissions(&original, fs::Permissions::from_mode(mode)).unwrap();
        }
        let before = fs::metadata(&original).unwrap();
        if as_unprivileged {
            fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o777)).unwrap();
            let status = Command::new("chown")
                .arg("65534:65534")
                .arg(&temporary)
                .status()
                .unwrap();
            assert!(status.success());
        }
        let (temporary_path, original_path) = if relative_dash_paths {
            (
                temporary.file_name().unwrap().to_str().unwrap(),
                original.file_name().unwrap().to_str().unwrap(),
            )
        } else {
            (temporary.to_str().unwrap(), original.to_str().unwrap())
        };
        let command = if let Some(size) = upload_size {
            shell_file_upload_command(attrs, temporary_path, original_path, size)
        } else {
            shell_file_replace_command(attrs, temporary_path, original_path)
        }
        .unwrap();
        let mut process = Command::new("sh");
        process.current_dir(directory.path());
        process.arg("-c").arg(format!("{prefix}\n{command}"));
        if as_unprivileged {
            process.uid(65534).gid(65534);
        }
        process
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = process.spawn().unwrap();
        if upload_size.is_some() {
            child.stdin.take().unwrap().write_all(UPLOAD_BYTES).unwrap();
        }
        let output = child.wait_with_output().unwrap();
        assert_eq!(
            output.status.success(),
            succeeds,
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        if succeeds {
            let expected = if upload_size.is_some() {
                UPLOAD_BYTES
            } else {
                b"new"
            };
            assert_eq!(fs::read(&original).unwrap(), expected);
            assert!(!temporary.exists());
            let metadata = fs::metadata(&original).unwrap();
            assert_eq!((metadata.uid(), metadata.gid()), (uid, gid));
            assert_eq!(metadata.mode() & 0o7777, mode & 0o7777);
        } else {
            assert_eq!(fs::read(&original).unwrap(), b"old");
            let after = fs::metadata(&original).unwrap();
            assert_eq!(
                (after.uid(), after.gid(), after.mode()),
                (before.uid(), before.gid(), before.mode())
            );
            assert!(temporary.exists());
        }
    }

    #[test]
    fn shell_editor_save_preserves_own_file_without_chown() {
        // SAFETY: getuid/getgid only read process identity.
        let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
        run_replace(uid, gid, 0o755, "chown() { return 1; }", false, true);
    }

    #[test]
    fn shell_editor_save_and_upload_handle_relative_dash_paths_with_posix_operands() {
        // Reject extra operands so GNU option permutation cannot hide a
        // post-mode/owner `--` on Linux. The native tools still do the work.
        let prefix = concat!(
            "ls() { test \"$#\" -eq 2 && command ls \"$@\"; }\n",
            "chmod() { test \"$#\" -eq 2 && command chmod \"$@\"; }\n",
            "chown() { test \"$#\" -eq 2 && command chown \"$@\"; }\n",
            "mv() { test \"$#\" -eq 3 && command mv \"$@\"; }",
        );
        // SAFETY: getuid/getgid/geteuid only read process identity.
        let (uid, gid, euid) = unsafe { (libc::getuid(), libc::getgid(), libc::geteuid()) };
        for upload_size in [None, Some(UPLOAD_BYTES.len())] {
            run_save_with_paths(
                &properties(uid, gid, 0o755),
                prefix,
                false,
                true,
                upload_size,
                true,
            );
            if euid == 0 {
                // Exercise the real chown branch and special-bit restoration.
                run_save_with_paths(
                    &properties(65534, 65534, 0o6755),
                    prefix,
                    false,
                    true,
                    upload_size,
                    true,
                );
            }
            run_save_with_paths(
                &properties(uid.wrapping_add(1), gid, 0o755),
                "chown() { return 0; }",
                false,
                false,
                upload_size,
                true,
            );
        }
    }

    #[test]
    fn shell_editor_save_verifies_changes_before_replacement() {
        // SAFETY: getuid/getgid only read process identity.
        let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
        run_replace(uid, gid, 0o755, "chmod() { return 0; }", false, false);
        run_replace(
            uid.wrapping_add(1),
            gid,
            0o755,
            "chown() { return 0; }",
            false,
            false,
        );
    }

    #[test]
    fn shell_editor_save_preserves_foreign_owner_and_special_bits_as_root() {
        // SAFETY: geteuid only reads process identity.
        if unsafe { libc::geteuid() } != 0 {
            return;
        }
        run_replace(65534, 65534, 0o6755, "", false, true);
        run_replace(65534, 65534, 0o644, "", true, true);
        run_replace(0, 0, 0o644, "", true, false);
    }

    #[test]
    fn shell_external_edit_upload_preserves_metadata_and_binary_contents() {
        // SAFETY: getuid/getgid only read process identity.
        let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
        run_save(
            uid,
            gid,
            0o755,
            "chown() { return 1; }",
            false,
            true,
            Some(UPLOAD_BYTES.len()),
        );
        // SAFETY: geteuid only reads process identity.
        if unsafe { libc::geteuid() } == 0 {
            run_save(
                65534,
                65534,
                0o6755,
                "",
                false,
                true,
                Some(UPLOAD_BYTES.len()),
            );
            run_save(0, 0, 0o644, "", true, false, Some(UPLOAD_BYTES.len()));
        }
    }

    #[test]
    fn shell_external_edit_upload_keeps_original_on_incomplete_write_or_attribute_failure() {
        // SAFETY: getuid/getgid only read process identity.
        let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
        run_save(
            uid,
            gid,
            0o755,
            "",
            false,
            false,
            Some(UPLOAD_BYTES.len() + 1),
        );
        for prefix in ["chmod() { return 1; }", "chmod() { return 0; }"] {
            run_save(
                uid,
                gid,
                0o755,
                prefix,
                false,
                false,
                Some(UPLOAD_BYTES.len()),
            );
        }
        run_save(
            uid.wrapping_add(1),
            gid,
            0o755,
            "chown() { return 0; }",
            false,
            false,
            Some(UPLOAD_BYTES.len()),
        );
    }
}
