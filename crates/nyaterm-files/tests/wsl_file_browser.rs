//! Real WSL filesystem regression. Run explicitly on Windows with WSL installed.
#![cfg(windows)]

use nyaterm_files::file_browser::{
    FileBrowserBackendKind, FileBrowserService, file_browser_path_eq,
    local_shell_cwd_to_browser_path_with_distro,
};
use nyaterm_files::remote_file::RemoteTextWriteResult;
use nyaterm_files::sftp::RemoteFilePath;
use nyaterm_transport::local_shell::LocalShellPathStyle;
use std::process::Command;

#[test]
#[ignore = "requires WSL; defaults to Ubuntu, override with NYATERM_TEST_WSL_DISTRO"]
fn wsl_unc_browser_operates_on_case_distinct_guest_directories() {
    let distro = std::env::var("NYATERM_TEST_WSL_DISTRO").unwrap_or_else(|_| "Ubuntu".into());
    let mapped = |path: &str| {
        local_shell_cwd_to_browser_path_with_distro(LocalShellPathStyle::Wsl, path, Some(&distro))
            .expect("representable WSL path")
    };
    // /var/tmp survives a distribution start (which may remount/clear /tmp).
    // The guard owns only this randomly named test directory, including on failure.
    let root = tempfile::Builder::new()
        .prefix("nyaterm-wsl-browser-")
        .tempdir_in(mapped("/var/tmp"))
        .expect("WSL temporary directory");
    let guest_root = format!(
        "/var/tmp/{}",
        root.path().file_name().unwrap().to_str().unwrap()
    );
    let output = Command::new("wsl.exe")
        .args(["--distribution", &distro, "--exec", "/bin/mkdir", "--"])
        .args([
            format!("{guest_root}/Project"),
            format!("{guest_root}/project"),
        ])
        .output()
        .expect("create Linux case-distinct fixtures");
    assert!(
        output.status.success(),
        "WSL fixture setup failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let service = FileBrowserService::local();
    let backend = FileBrowserBackendKind::Local;
    let root_path = RemoteFilePath::new(mapped(&guest_root));
    let upper = service.child_path(&root_path, "Project").unwrap();
    let lower = service.child_path(&root_path, "project").unwrap();
    assert!(!file_browser_path_eq(
        backend,
        &upper.display_path,
        &lower.display_path
    ));
    let entries = service.list_dir_path(&root_path).unwrap();
    assert_eq!(entries.len(), 2);
    for name in ["Project", "project"] {
        assert!(entries.iter().any(|entry| entry.name == name));
    }
    assert_ne!(entries[0].identity_key(), entries[1].identity_key());
    let upper_file = service.child_path(&upper, "marker.txt").unwrap();
    let lower_file = service.child_path(&lower, "marker.txt").unwrap();
    for (path, content) in [(&upper_file, "UPPER"), (&lower_file, "lower")] {
        service.create_file_remote_path(path, None).unwrap();
        assert!(matches!(
            service
                .write_text_document_path(path, content, None, true)
                .unwrap(),
            RemoteTextWriteResult::Saved { .. }
        ));
        assert_eq!(
            service
                .read_file_bytes_path(path, 64)
                .unwrap()
                .content_bytes,
            content.as_bytes()
        );
    }
    assert_eq!(
        service
            .read_text_document_path(&upper_file, 64)
            .unwrap()
            .content,
        "UPPER"
    );
    service
        .validate_copy_destination(&upper, &service, &lower, true)
        .unwrap();
    assert!(
        service
            .validate_copy_destination(&upper, &service, &upper, true)
            .is_err()
    );

    let new_dir = service.child_path(&lower, "新建 notes").unwrap();
    service.create_dir_remote_path(&new_dir, None).unwrap();
    assert!(service.list_dir_path(&new_dir).unwrap().is_empty());
    assert!(file_browser_path_eq(
        backend,
        &service.parent_path(&new_dir).unwrap().display_path,
        &lower.display_path
    ));
    let renamed = service.child_path(&lower, "renamed.txt").unwrap();
    service
        .rename_remote_paths_no_replace(&lower_file, &renamed)
        .unwrap();
    assert_eq!(
        service
            .read_file_bytes_path(&renamed, 64)
            .unwrap()
            .content_bytes,
        b"lower"
    );
    assert_eq!(
        service
            .read_file_bytes_path(&upper_file, 64)
            .unwrap()
            .content_bytes,
        b"UPPER"
    );
    service.delete_remote_path(&renamed).unwrap();
    service.delete_remote_path(&new_dir).unwrap();
    assert!(service.list_dir_path(&lower).unwrap().is_empty());
    assert_eq!(service.list_dir_path(&upper).unwrap().len(), 1);
    service.delete_remote_path(&upper_file).unwrap();
    service.delete_remote_path(&upper).unwrap();
    service.delete_remote_path(&lower).unwrap();
    assert!(service.list_dir_path(&root_path).unwrap().is_empty());
    root.close().expect("clean up the owned test directory");
}
