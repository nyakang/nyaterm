use super::super::duplicate::{DuplicateChoice, TransferDuplicateManager};
use super::*;

#[test]
fn skipped_download_event_is_not_completed_and_does_not_register_a_target() {
    for is_directory in [false, true] {
        let id = uuid::Uuid::new_v4().to_string();
        let event = build_skipped_download_event(
            "session-1",
            "/remote/app.log",
            "/local/app.log",
            Some(id.clone()),
            is_directory,
        );
        assert_eq!(event.id, id);
        assert_eq!(event.status, "skipped");
        assert_eq!(event.direction, "download");
        assert_eq!(event.kind, if is_directory { "directory" } else { "file" });
        assert_eq!(event.file_name, "app.log");
        assert_eq!(event.size, 0);
        assert_eq!(event.bytes_transferred, 0);
        assert_eq!(event.total_size, 0);
        assert!(event.error_msg.is_none());
        assert!(find_transfer(&id).is_none());
        assert!(find_transfer_target(&id).is_none());
    }
}

#[tokio::test]
async fn preparing_download_cancellation_ends_hidden_and_displayed_prompts() {
    for shown in [false, true] {
        let id = uuid::Uuid::new_v4().to_string();
        let preparing = create_child_file_transfer_controller(
            Some(id.clone()),
            "session",
            "app.log".to_string(),
            "/remote/app.log",
            "",
            "download",
            None,
        );
        let retained = RetainedTransfer::new(preparing.clone());
        let manager = Arc::new(TransferDuplicateManager::new());
        let (reply, displayed) = manager.register("cancelled".to_string()).await;
        if shown {
            assert!(manager.mark_displayed("cancelled").await);
        }
        let waiting = {
            let manager = manager.clone();
            let controller = preparing.clone();
            tokio::spawn(async move {
                manager
                    .wait_for_response(
                        "cancelled",
                        reply,
                        displayed,
                        Some(&controller),
                        std::time::Duration::from_secs(120),
                        std::time::Duration::from_secs(600),
                    )
                    .await
            })
        };
        tokio::task::yield_now().await;
        preparing.cancel();
        assert!(matches!(
            waiting.await.unwrap(),
            Err(AppError::Cancelled(_))
        ));
        assert!(!manager.mark_displayed("cancelled").await);
        assert!(
            !manager
                .respond("cancelled", DuplicateChoice::Overwrite)
                .await
        );
        assert!(matches!(
            retained.ready().await,
            Err(AppError::Cancelled(_))
        ));
        drop(retained);
        assert!(find_transfer(&id).is_none());
        assert!(find_transfer_target(&id).is_none());
    }
}

#[tokio::test]
async fn preparing_download_cancellation_survives_the_handoff_before_writing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("app.log").to_string_lossy().to_string();
    std::fs::write(&path, b"previous download").unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    let preparing = create_child_file_transfer_controller(
        Some(id.clone()),
        "session",
        "app.log".to_string(),
        "/remote/app.log",
        "",
        "download",
        None,
    );
    let retained = RetainedTransfer::new(preparing.clone());
    let manager = TransferDuplicateManager::new();
    let (reply, displayed) = manager.register("answered".to_string()).await;
    assert!(
        manager
            .respond("answered", DuplicateChoice::Overwrite)
            .await
    );
    assert_eq!(
        manager
            .wait_for_response(
                "answered",
                reply,
                displayed,
                Some(&preparing),
                std::time::Duration::from_secs(120),
                std::time::Duration::from_secs(600)
            )
            .await
            .unwrap(),
        DuplicateChoice::Overwrite
    );
    preparing.cancel();
    let actual = create_child_file_transfer_controller(
        Some(id.clone()),
        "session",
        "app.log".to_string(),
        "/remote/app.log",
        &path,
        "download",
        None,
    );
    register_transfer(actual.clone());
    assert!(matches!(
        wait_for_transfer_chain(&actual, None).await,
        Err(AppError::Cancelled(_))
    ));
    unregister_transfer(&id);
    assert!(matches!(
        retained.ready().await,
        Err(AppError::Cancelled(_))
    ));
    assert_eq!(std::fs::read(&path).unwrap(), b"previous download");
    drop(retained);
    assert!(find_transfer(&id).is_none());
}

#[test]
fn download_rename_preserves_file_extension_and_full_directory_name() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("app.log").to_string_lossy().to_string();
    std::fs::write(&file, b"first server").unwrap();
    std::fs::write(dir.path().join("app(1).log"), b"previous download").unwrap();
    assert_eq!(
        generate_unique_local_path(&file, false).unwrap(),
        dir.path().join("app(2).log").to_string_lossy()
    );
    assert_eq!(std::fs::read(&file).unwrap(), b"first server");

    let folder = dir.path().join("logs.2026").to_string_lossy().to_string();
    std::fs::create_dir(&folder).unwrap();
    assert_eq!(
        generate_unique_local_path(&folder, true).unwrap(),
        dir.path().join("logs.2026(1)").to_string_lossy()
    );
    assert_eq!(local_rename_candidate(".env", false, 1), ".env(1)");
    assert_eq!(local_rename_candidate("README", false, 1), "README(1)");
}

#[test]
fn download_rename_exhaustion_never_returns_original_path() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("app.log").to_string_lossy().to_string();
    std::fs::write(&file, b"original").unwrap();
    for index in 1..=999 {
        std::fs::write(local_rename_candidate(&file, false, index), b"").unwrap();
    }
    assert!(generate_unique_local_path(&file, false).is_err());
    assert!(reserve_renamed_local_download_target(&file, false).is_err());
    assert_eq!(std::fs::read(&file).unwrap(), b"original");
}

#[test]
fn download_reservation_protects_paths_before_files_are_created_and_releases_on_drop() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("app.log").to_string_lossy().to_string();
    let first = reserve_local_download_target(&file, false, false)
        .unwrap()
        .unwrap();
    assert!(!Path::new(&file).exists());
    assert!(
        reserve_local_download_target(&file, false, false)
            .unwrap()
            .is_none()
    );
    assert!(reserve_local_download_target(&file, false, true).is_err());
    let second = reserve_renamed_local_download_target(&file, false).unwrap();
    let third = reserve_renamed_local_download_target(&file, false).unwrap();
    assert_eq!(second.path, dir.path().join("app(1).log").to_string_lossy());
    assert_eq!(third.path, dir.path().join("app(2).log").to_string_lossy());
    drop(first);
    assert!(
        reserve_local_download_target(&file, false, false)
            .unwrap()
            .is_some()
    );
}

#[test]
fn download_reservation_rejects_overlapping_directory_transfers() {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("logs").to_string_lossy().to_string();
    let child = dir
        .path()
        .join("logs/app.log")
        .to_string_lossy()
        .to_string();
    let target = reserve_local_download_target(&folder, true, false)
        .unwrap()
        .unwrap();
    assert!(reserve_local_download_target(&child, false, true).is_err());
    drop(target);
    let _child_target = reserve_local_download_target(&child, false, false)
        .unwrap()
        .unwrap();
    assert!(reserve_local_download_target(&folder, true, true).is_err());
}

#[test]
fn download_reservation_keeps_existing_file_until_overwrite_is_explicit() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("app.log").to_string_lossy().to_string();
    std::fs::write(&file, b"original").unwrap();
    assert!(
        reserve_local_download_target(&file, false, false)
            .unwrap()
            .is_none()
    );
    let _target = reserve_local_download_target(&file, false, true)
        .unwrap()
        .unwrap();
    assert_eq!(std::fs::read(&file).unwrap(), b"original");
}

#[cfg(windows)]
#[test]
fn download_reservation_recognizes_windows_case_aliases() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("App.log").to_string_lossy().to_string();
    let alias = dir.path().join("app.log").to_string_lossy().to_string();
    let _target = reserve_local_download_target(&file, false, false)
        .unwrap()
        .unwrap();
    assert!(
        reserve_local_download_target(&alias, false, false)
            .unwrap()
            .is_none()
    );
}
