//! Local and remote target selection for SFTP transfers.

use super::upload_runtime::UploadRetryUnsafe;
use super::{
    SftpDuplicateDecision, SftpDuplicatePolicy, SftpDuplicateRequest, SftpDuplicateResolver,
    SftpPathCodec, SftpPathTransferOptions, SftpTransferControl, SftpTransferDirection,
    await_sftp_request,
};
use crate::download_path::{DownloadTargetAccess, root_for_target};
use crate::sftp_transfer_types::SftpDuplicateCacheKey;
use russh_sftp::client::SftpSession;
use std::path::{Path, PathBuf};

pub(super) fn resolve_remote_upload_target(
    local_path: &Path,
    remote_path: &str,
) -> anyhow::Result<String> {
    if remote_path == "." || remote_path.ends_with('/') {
        Ok(remote_join(remote_path, &local_file_name(local_path)?))
    } else {
        Ok(remote_path.to_string())
    }
}

fn local_file_name(path: &Path) -> anyhow::Result<String> {
    path.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| anyhow::anyhow!("local path has no file name: {}", path.display()))
}

/// 本地下载目标解析所需的显示路径、真实身份和任务选项。
pub(super) struct SftpLocalDownloadTargetContext<'a> {
    pub(super) remote_path: &'a str,
    pub(super) remote_path_raw: &'a [u8],
    pub(super) local_path: &'a Path,
    pub(super) is_directory: bool,
    pub(super) path_options: &'a SftpPathTransferOptions,
}

pub(super) fn resolve_local_download_target(
    context: SftpLocalDownloadTargetContext<'_>,
) -> anyhow::Result<Option<PathBuf>> {
    let key = SftpDuplicateCacheKey::Download {
        remote_path: context.remote_path_raw.to_vec(),
        local_path: context.local_path.to_path_buf(),
        is_directory: context.is_directory,
    };
    let selected = context.path_options.download_runtime()?.selected(&key);
    if let Some(selected) = selected
        && (context.path_options.is_promised_download()
            || context
                .path_options
                .download_runtime()?
                .owns(&selected, context.is_directory)?)
    {
        return Ok(Some(selected));
    }
    let remember =
        |path: PathBuf, access: Option<DownloadTargetAccess>| -> anyhow::Result<Option<PathBuf>> {
            context
                .path_options
                .download_runtime()?
                .select(key.clone(), &path, access);
            Ok(Some(path))
        };
    if !crate::download_path::target_exists(context.local_path)?
        && context
            .path_options
            .reserve_download_target(context.local_path, &key)?
    {
        return remember(
            context.local_path.to_path_buf(),
            (!context.is_directory).then_some(DownloadTargetAccess::CreateNew),
        );
    }

    if context
        .path_options
        .download_runtime()?
        .owns(context.local_path, context.is_directory)?
    {
        return remember(context.local_path.to_path_buf(), None);
    }
    anyhow::ensure!(
        !context.path_options.is_promised_download(),
        "promised destination already exists"
    );

    let decision = resolve_duplicate_decision_for_path(
        context.path_options,
        key.clone(),
        SftpTransferDirection::Download,
        context.remote_path,
        &context.local_path.display().to_string(),
        context.is_directory,
    )?;
    match decision {
        SftpDuplicateDecision::Overwrite => {
            anyhow::ensure!(
                context
                    .path_options
                    .reserve_download_target(context.local_path, &key)?,
                "download target is reserved by another item in this batch"
            );
            let access = if context.is_directory {
                None
            } else {
                Some(DownloadTargetAccess::capture(
                    root_for_target(context.local_path),
                    context.local_path,
                )?)
            };
            remember(context.local_path.to_path_buf(), access)
        }
        SftpDuplicateDecision::Skip => Ok(None),
        SftpDuplicateDecision::Rename => {
            let target =
                resolve_renamed_local_target(context.local_path, context.path_options, &key)?;
            remember(
                target,
                (!context.is_directory).then_some(DownloadTargetAccess::CreateNew),
            )
        }
    }
}

/// 远端写入目标解析所需的会话、路径和任务选项。
pub(super) struct SftpRemoteWriteTargetContext<'a> {
    pub(super) sftp: &'a SftpSession,
    pub(super) codec: &'a SftpPathCodec,
    pub(super) local_path: &'a Path,
    pub(super) remote_path: &'a str,
    pub(super) is_directory: bool,
    pub(super) path_options: &'a SftpPathTransferOptions,
    pub(super) control: &'a SftpTransferControl,
}

pub(super) async fn resolve_remote_write_target(
    context: SftpRemoteWriteTargetContext<'_>,
) -> anyhow::Result<Option<String>> {
    let key = SftpDuplicateCacheKey::Upload {
        local_path: context.local_path.to_path_buf(),
        remote_path: context.remote_path.to_string(),
        is_directory: context.is_directory,
    };
    let runtime = context.path_options.upload_runtime();
    if let Some(selected) = runtime.selected(&key) {
        return Ok(selected);
    }
    let exists = await_sftp_request(
        context.control,
        context
            .sftp
            .try_exists_bytes(context.codec.encode_path(context.remote_path)?),
    )
    .await?;
    let decision = if exists {
        resolve_duplicate_decision_for_path(
            context.path_options,
            key.clone(),
            SftpTransferDirection::Upload,
            &context.local_path.display().to_string(),
            context.remote_path,
            context.is_directory,
        )?
    } else {
        SftpDuplicateDecision::Overwrite
    };
    let selected = match decision {
        SftpDuplicateDecision::Overwrite => Some(context.remote_path.to_string()),
        SftpDuplicateDecision::Skip => None,
        SftpDuplicateDecision::Rename => Some(
            resolve_renamed_remote_target(context.sftp, context.codec, context.remote_path).await?,
        ),
    };
    if let Some(target) = &selected {
        let remembered = runtime
            .remember_target(
                context.sftp,
                context.codec.encode_path(target)?,
                context.is_directory,
                !exists || decision == SftpDuplicateDecision::Rename,
                context.control,
            )
            .await;
        if let Err(error) = remembered {
            // A cached decision is not an ownership grant if its associated
            // target could not be captured after the user answered.
            runtime.block();
            return Err(error.context(UploadRetryUnsafe));
        }
    }
    runtime.select(key, selected.clone());
    Ok(selected)
}

fn resolve_duplicate_decision_for_path(
    path_options: &SftpPathTransferOptions,
    cache_key: SftpDuplicateCacheKey,
    direction: SftpTransferDirection,
    source_path: &str,
    target_path: &str,
    is_directory: bool,
) -> anyhow::Result<SftpDuplicateDecision> {
    if path_options.duplicate_policy() == SftpDuplicatePolicy::Ask
        && let Some(decision) = path_options
            .cached_duplicate_decision(&cache_key)
            .map_err(anyhow::Error::msg)?
    {
        return Ok(decision);
    }
    let decision = resolve_duplicate_decision(
        direction,
        source_path,
        target_path,
        is_directory,
        path_options.duplicate_policy(),
        path_options.duplicate_resolver(),
    )?;
    if path_options.duplicate_policy() == SftpDuplicatePolicy::Ask {
        path_options
            .remember_duplicate_decision(cache_key, decision)
            .map_err(anyhow::Error::msg)?;
    }
    Ok(decision)
}

pub(super) fn resolve_duplicate_decision(
    direction: SftpTransferDirection,
    source_path: &str,
    target_path: &str,
    is_directory: bool,
    duplicate_policy: SftpDuplicatePolicy,
    duplicate_resolver: Option<&dyn SftpDuplicateResolver>,
) -> anyhow::Result<SftpDuplicateDecision> {
    match duplicate_policy {
        SftpDuplicatePolicy::Overwrite => Ok(SftpDuplicateDecision::Overwrite),
        SftpDuplicatePolicy::Skip => Ok(SftpDuplicateDecision::Skip),
        SftpDuplicatePolicy::Rename => Ok(SftpDuplicateDecision::Rename),
        SftpDuplicatePolicy::Ask => {
            let resolver = duplicate_resolver.ok_or_else(|| {
                anyhow::anyhow!("SFTP duplicate policy is ask but no resolver is available")
            })?;
            resolver
                .resolve_duplicate(&SftpDuplicateRequest {
                    direction,
                    source_path: source_path.to_string(),
                    target_path: target_path.to_string(),
                    is_directory,
                })
                .map_err(anyhow::Error::msg)
        }
    }
}

fn resolve_renamed_local_target(
    local_path: &Path,
    options: &SftpPathTransferOptions,
    key: &SftpDuplicateCacheKey,
) -> anyhow::Result<PathBuf> {
    let stem = local_path
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .filter(|stem| !stem.is_empty())
        .unwrap_or_else(|| local_file_name(local_path).unwrap_or_else(|_| "download".to_string()));
    let extension = local_path
        .extension()
        .map(|extension| format!(".{}", extension.to_string_lossy()))
        .unwrap_or_default();
    let parent = local_path.parent().unwrap_or_else(|| Path::new("."));
    for index in 1..=999 {
        let candidate = parent.join(format!("{stem}({index}){extension}"));
        if !crate::download_path::target_exists(&candidate)?
            && options.reserve_download_target(&candidate, key)?
        {
            return Ok(candidate);
        }
    }
    anyhow::bail!(
        "unable to find a non-conflicting local path for {}",
        local_path.display()
    )
}

async fn resolve_renamed_remote_target(
    sftp: &SftpSession,
    codec: &SftpPathCodec,
    remote_path: &str,
) -> anyhow::Result<String> {
    for index in 1..=999 {
        let candidate = remote_conflict_candidate(remote_path, index);
        if !sftp
            .try_exists_bytes(codec.encode_path(&candidate)?)
            .await?
        {
            return Ok(candidate);
        }
    }
    anyhow::bail!("unable to find a non-conflicting remote path for {remote_path}")
}

pub(super) fn remote_conflict_candidate(remote_path: &str, index: usize) -> String {
    let (parent, name) = remote_split_parent_name(remote_path);
    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem.to_string(), format!(".{extension}")),
        _ => (name, String::new()),
    };
    remote_join(&parent, &format!("{stem}({index}){extension}"))
}

fn remote_split_parent_name(remote_path: &str) -> (String, String) {
    let trimmed = remote_path.trim_end_matches('/');
    match trimmed.rsplit_once('/') {
        Some(("", name)) => ("/".to_string(), name.to_string()),
        Some((parent, name)) => (parent.to_string(), name.to_string()),
        None => (".".to_string(), trimmed.to_string()),
    }
}

pub(super) fn remote_join(base: &str, child: &str) -> String {
    if base.is_empty() || base == "." {
        child.to_string()
    } else if base == "/" {
        format!("/{child}")
    } else if base.ends_with('/') {
        format!("{base}{child}")
    } else {
        format!("{base}/{child}")
    }
}

#[cfg(test)]
mod tests {
    use super::{SftpLocalDownloadTargetContext, resolve_local_download_target};
    use crate::sftp_transfer_types::{
        SftpDuplicatePolicy, SftpPathTransferOptions, SftpTransferOptions,
    };

    #[test]
    fn renamed_downloads_reserve_final_targets_before_writing() -> anyhow::Result<()> {
        for names in [["foo", "foo(1)"], ["foo(1)", "foo"]] {
            let root =
                std::env::temp_dir().join(format!("nyaterm-reserved-{}", nyaterm_core::uuid()));
            std::fs::create_dir(&root)?;
            std::fs::write(root.join("foo"), b"existing")?;
            let options = SftpPathTransferOptions::new(
                SftpDuplicatePolicy::Rename,
                None,
                SftpTransferOptions::default(),
            );
            let mut targets = Vec::new();
            for name in names {
                let target = resolve_local_download_target(SftpLocalDownloadTargetContext {
                    remote_path: name,
                    remote_path_raw: name.as_bytes(),
                    local_path: &root.join(name),
                    is_directory: false,
                    path_options: &options,
                })?
                .expect("selected target");
                targets.push((name, target));
            }
            assert_ne!(targets[0].1, targets[1].1);
            for (name, target) in &targets {
                crate::download_path::staged_write_file(&root, target, name.as_bytes())?;
            }
            assert_eq!(std::fs::read(root.join("foo"))?, b"existing");
            for (name, target) in &targets {
                assert_eq!(std::fs::read(target)?, name.as_bytes());
            }
            std::fs::remove_dir_all(root)?;
        }
        Ok(())
    }

    #[cfg(any(windows, target_os = "macos"))]
    #[test]
    fn normalized_names_use_reserved_rename_targets() -> anyhow::Result<()> {
        let root = std::env::temp_dir().join(format!(
            "nyaterm-normalized-reserved-{}",
            nyaterm_core::uuid()
        ));
        std::fs::create_dir(&root)?;
        std::fs::write(root.join("Readme"), b"existing")?;
        let options = SftpPathTransferOptions::new(
            SftpDuplicatePolicy::Rename,
            None,
            SftpTransferOptions::default(),
        );
        let first = resolve_local_download_target(SftpLocalDownloadTargetContext {
            remote_path: "/Readme",
            remote_path_raw: b"/Readme",
            local_path: &root.join("Readme"),
            is_directory: false,
            path_options: &options,
        })?
        .expect("first selected target");
        let second = resolve_local_download_target(SftpLocalDownloadTargetContext {
            remote_path: "/README",
            remote_path_raw: b"/README",
            local_path: &root.join("README"),
            is_directory: false,
            path_options: &options,
        })?
        .expect("second selected target");
        assert_ne!(first, second);
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn concurrent_batch_jobs_reserve_distinct_rename_candidates() -> anyhow::Result<()> {
        let root = std::env::temp_dir().join(format!(
            "nyaterm-concurrent-targets-{}",
            nyaterm_core::uuid()
        ));
        std::fs::create_dir(&root)?;
        let target = root.join("foo");
        std::fs::write(&target, b"existing")?;
        let options = SftpPathTransferOptions::new(
            SftpDuplicatePolicy::Rename,
            None,
            SftpTransferOptions::default(),
        );
        let barrier = std::sync::Barrier::new(4);
        let targets = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..4)
                .map(|_| {
                    let options = options.clone_for_download_batch();
                    let target = &target;
                    let barrier = &barrier;
                    scope.spawn(move || {
                        barrier.wait();
                        resolve_local_download_target(SftpLocalDownloadTargetContext {
                            remote_path: "/foo",
                            remote_path_raw: b"/foo",
                            local_path: target,
                            is_directory: false,
                            path_options: &options,
                        })
                        .unwrap()
                        .unwrap()
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .collect::<std::collections::HashSet<_>>()
        });
        assert_eq!(targets.len(), 4);
        assert!(!targets.contains(&target));
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn batch_reservations_reject_overwrite_allow_skip_and_preserve_retry_identity()
    -> anyhow::Result<()> {
        for policy in [SftpDuplicatePolicy::Overwrite, SftpDuplicatePolicy::Skip] {
            let root = std::env::temp_dir().join(format!(
                "nyaterm-reservation-policy-{}",
                nyaterm_core::uuid()
            ));
            std::fs::create_dir(&root)?;
            let target = root.join("entry");
            let options =
                SftpPathTransferOptions::new(policy, None, SftpTransferOptions::default());
            let resolve = |options: &SftpPathTransferOptions, is_directory| {
                resolve_local_download_target(SftpLocalDownloadTargetContext {
                    remote_path: "/entry",
                    remote_path_raw: b"/entry",
                    local_path: &target,
                    is_directory,
                    path_options: options,
                })
            };
            assert_eq!(resolve(&options, true)?, Some(target.clone()));
            let sibling = options.clone_for_download_batch();
            match policy {
                SftpDuplicatePolicy::Overwrite => assert!(resolve(&sibling, false).is_err()),
                SftpDuplicatePolicy::Skip => assert!(resolve(&sibling, false)?.is_none()),
                _ => unreachable!(),
            }
            let retry = options.with_transfer_options(SftpTransferOptions::default());
            assert_eq!(resolve(&retry, true)?, Some(target.clone()));
            assert_eq!(resolve(&options.clone(), true)?, Some(target));
            std::fs::remove_dir_all(root)?;
        }
        Ok(())
    }
}
