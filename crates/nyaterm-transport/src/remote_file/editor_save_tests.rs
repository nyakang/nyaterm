use crate::remote_file::{shell_editor_replace_command, shell_preserving_upload_command};
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
        assert!(shell_editor_replace_command(&attrs, "/tmp/edit", "/tmp/original").is_err());
    }
}

#[test]
fn shell_preserving_upload_refuses_missing_original_attributes() {
    for missing in ["uid", "gid", "mode"] {
        let mut attrs = properties(1000, 1001, 0o644);
        match missing {
            "uid" => attrs.uid = None,
            "gid" => attrs.gid = None,
            "mode" => attrs.permissions = None,
            _ => unreachable!(),
        }
        assert!(shell_preserving_upload_command(&attrs, "/tmp/new", "/tmp/original", 3).is_err());
    }
}

#[cfg(unix)]
mod posix {
    use std::fs;
    use std::io::Write as _;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    use crate::remote_file::editor_save_tests::properties;
    use crate::remote_file::{shell_editor_replace_command, shell_preserving_upload_command};

    fn run_replace(
        uid: u32,
        gid: u32,
        mode: u32,
        prefix: &str,
        as_unprivileged: bool,
        succeeds: bool,
    ) {
        let directory = tempfile::tempdir().unwrap();
        // Quoting and glob expansion must be safe even for shell metacharacters.
        let original = directory.path().join("original ' $file [*].txt");
        let temporary = directory.path().join("temporary ' $file [*].txt");
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
        let command = shell_editor_replace_command(
            &properties(uid, gid, mode),
            temporary.to_str().unwrap(),
            original.to_str().unwrap(),
        )
        .unwrap();
        let mut process = Command::new("sh");
        process.arg("-c").arg(format!("{prefix}\n{command}"));
        if as_unprivileged {
            process.uid(65534).gid(65534);
        }
        let output = process.output().unwrap();
        assert_eq!(
            output.status.success(),
            succeeds,
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        if succeeds {
            assert_eq!(fs::read(&original).unwrap(), b"new");
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

    /// The external-editor sync-back streams bytes into a staged file and must keep the
    /// target's owner, group and mode instead of resetting them to the login user.
    #[test]
    fn shell_preserving_upload_keeps_original_ownership_and_mode() {
        // SAFETY: getuid/getgid only read process identity.
        let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
        let directory = tempfile::tempdir().unwrap();
        let original = directory.path().join("original ' $file [*].txt");
        let temporary = directory.path().join("staged ' $file [*].txt");
        fs::write(&original, b"old").unwrap();
        fs::set_permissions(&original, fs::Permissions::from_mode(0o754)).unwrap();
        let command = shell_preserving_upload_command(
            &properties(uid, gid, 0o754),
            temporary.to_str().unwrap(),
            original.to_str().unwrap(),
            b"replacement".len(),
        )
        .unwrap();
        let mut child = Command::new("sh")
            .arg("-c")
            .arg(&command)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"replacement")
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(fs::read(&original).unwrap(), b"replacement");
        assert!(!temporary.exists());
        let metadata = fs::metadata(&original).unwrap();
        assert_eq!((metadata.uid(), metadata.gid()), (uid, gid));
        assert_eq!(metadata.mode() & 0o7777, 0o754);
    }
}
