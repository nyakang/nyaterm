use std::env;
use std::fs;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use nyaterm_transport::{
    SftpDuplicateDecision, SftpDuplicatePolicy, SftpDuplicateRequest, SftpDuplicateResolver,
    SftpFileType, SftpPathTransferOptions, SftpService, SftpSettings, SftpTransferControl,
    SftpTransferOptions, SftpWriteTextResult, SshCredentialProvider, SshHostKey,
    SshHostKeyDecision, SshHostKeyVerifier, SshOtpProvider, SshSessionConfig,
};

struct AcceptEphemeralHostKey;

struct OverwriteRootDirectory;

impl SftpDuplicateResolver for OverwriteRootDirectory {
    fn resolve_duplicate(
        &self,
        request: &SftpDuplicateRequest,
    ) -> Result<SftpDuplicateDecision, String> {
        assert!(request.is_directory, "only the root directory should ask");
        Ok(SftpDuplicateDecision::Overwrite)
    }
}

impl SshHostKeyVerifier for AcceptEphemeralHostKey {
    fn verify(&self, _host_key: &SshHostKey) -> Result<SshHostKeyDecision, String> {
        Ok(SshHostKeyDecision::Accept)
    }
}

fn required_env(name: &str) -> String {
    env::var(name).unwrap_or_else(|_| panic!("{name} must be set for the SFTP E2E test"))
}

fn test_config() -> SshSessionConfig {
    SshSessionConfig {
        runtime_mode: Default::default(),
        attempt: Default::default(),
        post_login: None,
        name: "SFTP E2E".to_string(),
        host: required_env("NYATERM_TEST_SFTP_HOST"),
        port: required_env("NYATERM_TEST_SFTP_PORT")
            .parse()
            .expect("NYATERM_TEST_SFTP_PORT must be a valid port"),
        host_key_alias: None,
        username: required_env("NYATERM_TEST_SFTP_USERNAME"),
        password: Some(required_env("NYATERM_TEST_SFTP_PASSWORD").into()),
        key_auth: None,
        agent_auth: false,
        agent_endpoint: Default::default(),
        agent_forwarding: false,
        agent_forwarding_config: None,
        agent_stored_key_provider: None,
        otp_id: None,
        auto_fill_otp: false,
        proxy_jump: None,
        proxy: None,
        allow_none_auth: false,
        profile: Default::default(),
        backspace_mode: "del".to_string(),
        term: "xterm-256color".to_string(),
        x11_forwarding: false,
        x11_display: String::new(),
        encoding: "UTF-8".to_string(),
        ssh_algorithms: None,
        sftp: SftpSettings::default(),
        deferred_pty: true,
        terminal_shell_integration: true,
        keep_alive_interval_secs: 0,
        keep_alive_mode: Default::default(),
        cols: 80,
        rows: 24,
        pixel_width: 0,
        pixel_height: 0,
        host_key_verifier: Some(Arc::new(AcceptEphemeralHostKey)),
        credential_provider: None::<Arc<dyn SshCredentialProvider>>,
        agent_prompt_provider: None,
        otp_provider: None::<Arc<dyn SshOtpProvider>>,
    }
}

#[test]
#[ignore = "requires NYATERM_TEST_SFTP_* variables and a disposable SFTP directory"]
fn sftp_service_round_trips_file_manager_operations() -> anyhow::Result<()> {
    let service = SftpService::new(test_config());
    let root = required_env("NYATERM_TEST_SFTP_ROOT")
        .trim_end_matches('/')
        .to_string();
    let unique = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let remote_dir = format!("{root}/nyaterm-e2e-{}-{unique}", std::process::id());
    let remote_file = format!("{remote_dir}/file2.txt");
    let renamed_file = format!("{remote_dir}/file10.txt");
    let uploaded_file = format!("{remote_dir}/uploaded.txt");
    let local_dir =
        nyaterm_core::test_support::TestTempDir::new(&format!("nyaterm-sftp-e2e-{unique}"));
    let local_source = local_dir.join("source.txt");
    let local_download = local_dir.join("download.txt");

    fs::create_dir_all(&local_dir)?;
    fs::write(&local_source, "uploaded through SFTP\n")?;

    let result = (|| -> anyhow::Result<()> {
        service.create_dir_path(&remote_dir, Some(0o750))?;
        service.create_file_path(&remote_file, Some(0o640))?;

        let saved =
            service.write_text_file(&remote_file, "hello from NyaTerm\n", None, None, true)?;
        anyhow::ensure!(matches!(saved, SftpWriteTextResult::Saved { .. }));

        let text = service.read_text_file(&remote_file, 1024)?;
        anyhow::ensure!(text.content == "hello from NyaTerm\n");

        let properties = service.file_properties(&remote_file)?;
        anyhow::ensure!(properties.name == "file2.txt");
        anyhow::ensure!(properties.file_type == SftpFileType::File);

        service.rename_path(&remote_file, &renamed_file)?;
        service.upload_file(&local_source, &uploaded_file)?;
        service.download_file(&uploaded_file, &local_download)?;
        anyhow::ensure!(fs::read_to_string(&local_download)? == "uploaded through SFTP\n");

        let entries = service.list_dir(&remote_dir)?;
        anyhow::ensure!(entries.iter().any(|entry| entry.name == "file10.txt"));
        anyhow::ensure!(entries.iter().any(|entry| entry.name == "uploaded.txt"));
        anyhow::ensure!(!entries.iter().any(|entry| entry.name == "file2.txt"));

        // Exercise actual directory enumeration, deferred downloads, and commit together.
        // Invalid local names on ignored links must not abort the regular-file downloads.
        let collision_dir = format!("{remote_dir}/collisions");
        service.create_dir_path(&collision_dir, None)?;
        for (name, contents) in [("foo", "first"), ("foo(1)", "second")] {
            let path = format!("{collision_dir}/{name}");
            service.create_file_path(&path, None)?;
            service.write_text_file(&path, contents, None, None, true)?;
        }
        service.create_symlink_path(format!("{collision_dir}/bad\\link"), "foo")?;
        service.create_symlink_path(format!("{collision_dir}/CON"), "foo")?;
        let collision_target = local_dir.join("collisions");
        fs::create_dir(&collision_target)?;
        fs::write(collision_target.join("foo"), b"existing")?;
        let summary = service.download_path_with_progress_and_path_options(
            &collision_dir,
            &collision_target,
            SftpTransferControl::default(),
            SftpPathTransferOptions::new(
                SftpDuplicatePolicy::Ask,
                Some(Arc::new(OverwriteRootDirectory)),
                SftpTransferOptions::default().with_download_threads(2),
            ),
            |_| {},
        )?;
        anyhow::ensure!(!summary.skipped);
        anyhow::ensure!(fs::read(collision_target.join("foo"))? == b"first");
        let mut contents = fs::read_dir(&collision_target)?
            .map(|entry| fs::read(entry?.path()))
            .collect::<std::io::Result<Vec<_>>>()?;
        contents.sort();
        anyhow::ensure!(contents == [b"first".to_vec(), b"second".to_vec()]);
        Ok(())
    })();

    let remote_cleanup = service.delete_path(&remote_dir);
    let local_cleanup = fs::remove_dir_all(&local_dir);
    result?;
    remote_cleanup?;
    local_cleanup?;
    Ok(())
}
#[test]
#[ignore = "requires NYATERM_TEST_SFTP_* variables and a disposable SFTP directory"]
fn shared_copy_engine_preserves_raw_targets_across_source_combinations() -> anyhow::Result<()> {
    use nyaterm_transport::{
        FileBrowserService, FileCopyRequest, RemoteFilePath, RemoteFileService,
    };
    let config = test_config();
    let origin = FileBrowserService::remote(RemoteFileService::new(config.clone()));
    let other_session = FileBrowserService::remote(RemoteFileService::new(config));
    let root = required_env("NYATERM_TEST_SFTP_ROOT")
        .trim_end_matches('/')
        .to_string();
    let remote_root = RemoteFilePath::new(format!(
        "{root}/nyaterm-copy-matrix-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    ));
    let local = nyaterm_core::test_support::TestTempDir::new("nyaterm-sftp-copy-matrix");
    fs::create_dir_all(local.join("source/目录"))?;
    fs::write(local.join("source/目录/文件.txt"), b"payload")?;
    let local_service = FileBrowserService::local();
    let source = RemoteFilePath::new(local.join("source").to_string_lossy());
    origin.create_dir_remote_path(&remote_root, None)?;
    let result = (|| -> anyhow::Result<()> {
        let uploaded_display = format!("{}/uploaded", remote_root.display_path);
        let uploaded = RemoteFilePath::from_raw(&uploaded_display, uploaded_display.as_bytes());
        let same = origin.child_path(&remote_root, "same-session")?;
        let different = origin.child_path(&remote_root, "other-session")?;
        let local_copy = RemoteFilePath::new(local.join("local-copy").to_string_lossy());
        let downloaded = RemoteFilePath::new(local.join("downloaded").to_string_lossy());
        for (source_service, source, target_service, target) in [
            (&local_service, &source, &local_service, &local_copy),
            (&local_service, &source, &origin, &uploaded),
            (&origin, &uploaded, &origin, &same),
            (&origin, &same, &other_session, &different),
            (&other_session, &different, &local_service, &downloaded),
        ] {
            let summary = FileCopyRequest {
                source: source_service.transfer_endpoint(source),
                destination: target_service.transfer_endpoint(target),
                control: SftpTransferControl::new(),
                options: SftpPathTransferOptions::new(
                    SftpDuplicatePolicy::Skip,
                    None,
                    Default::default(),
                ),
            }
            .execute()?;
            anyhow::ensure!(summary.copied && summary.bytes == 7);
        }
        anyhow::ensure!(fs::read(local.join("downloaded/目录/文件.txt"))? == b"payload");
        let skipped = FileCopyRequest {
            source: local_service.transfer_endpoint(&source),
            destination: origin.transfer_endpoint(&uploaded),
            control: SftpTransferControl::new(),
            options: SftpPathTransferOptions::new(
                SftpDuplicatePolicy::Skip,
                None,
                Default::default(),
            ),
        }
        .execute()?;
        anyhow::ensure!(!skipped.copied);
        anyhow::ensure!(
            origin
                .rename_remote_paths_no_replace(&same, &different)
                .is_err()
        );
        anyhow::ensure!(origin.remote_file_properties(&same)?.is_directory());
        let descendant = origin.child_path(&same, "recursive")?;
        anyhow::ensure!(
            FileCopyRequest {
                source: origin.transfer_endpoint(&same),
                destination: other_session.transfer_endpoint(&descendant),
                control: SftpTransferControl::new(),
                options: Default::default()
            }
            .execute()
            .is_err()
        );
        Ok(())
    })();
    let cleanup = origin.delete_remote_path(&remote_root);
    result?;
    cleanup?;
    Ok(())
}

/// Read-only remote fixtures; every sample downloads into a fresh local directory.
#[test]
#[ignore = "requires SFTP credentials, NYATERM_BENCH_LOCAL_ROOT and three prepared remote fixtures"]
fn sftp_download_release_benchmark_five_samples() -> anyhow::Result<()> {
    use std::time::Instant;
    let service = SftpService::new(test_config());
    let local_root = required_env("NYATERM_BENCH_LOCAL_ROOT");
    let fixtures = [
        required_env("NYATERM_BENCH_FILE"),
        required_env("NYATERM_BENCH_SMALL_DIR"),
        required_env("NYATERM_BENCH_MIXED_DIR"),
    ];
    for (index, fixture) in fixtures.iter().enumerate() {
        let mut samples = Vec::new();
        for _ in 0..5 {
            let root = tempfile::tempdir_in(&local_root)?;
            let target = root.path().join("download");
            let started = Instant::now();
            let summary = service.download_path_with_progress_and_path_options(
                fixture,
                &target,
                SftpTransferControl::new(),
                Default::default(),
                |_| {},
            )?;
            let elapsed_us = started.elapsed().as_micros();
            if index == 0 {
                anyhow::ensure!(
                    summary.bytes == 1024 * 1024 * 1024,
                    "single-file fixture must be 1 GiB"
                );
            }
            let mut pending = vec![target];
            let mut files = 0;
            while let Some(path) = pending.pop() {
                if path.is_dir() {
                    for entry in fs::read_dir(path)? {
                        pending.push(entry?.path());
                    }
                } else {
                    files += 1;
                }
            }
            if index == 1 {
                anyhow::ensure!(files == 5000, "small-file fixture must contain 5000 files");
            }
            if index == 2 {
                anyhow::ensure!(files > 1, "mixed fixture must contain multiple files");
            }
            eprintln!(
                "{}",
                serde_json::json!({ "bytes": summary.bytes, "files": files, "total_us": elapsed_us })
            );
            samples.push(elapsed_us);
            // Cleanup and verification are deliberately outside the measured interval.
        }
        samples.sort_unstable();
        eprintln!(
            "{}",
            serde_json::json!({ "samples": 5, "median_total_us": samples[2] })
        );
    }
    Ok(())
}
