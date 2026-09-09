//! Local and remote target selection for SFTP transfers.

use std::path::{Path, PathBuf};

use russh_sftp::client::SftpSession;

use crate::sftp_transfer_types::SftpDuplicateCacheKey;

use super::{
    SftpDuplicateDecision, SftpDuplicatePolicy, SftpDuplicateRequest, SftpDuplicateResolver,
    SftpPathCodec, SftpPathTransferOptions, SftpTransferDirection,
};

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
    if !context.local_path.exists() {
        return Ok(Some(context.local_path.to_path_buf()));
    }

    let decision = resolve_duplicate_decision_for_path(
        context.path_options,
        SftpDuplicateCacheKey::Download {
            remote_path: context.remote_path_raw.to_vec(),
            local_path: context.local_path.to_path_buf(),
            is_directory: context.is_directory,
        },
        SftpTransferDirection::Download,
        context.remote_path,
        &context.local_path.display().to_string(),
        context.is_directory,
    )?;
    match decision {
        SftpDuplicateDecision::Overwrite => Ok(Some(context.local_path.to_path_buf())),
        SftpDuplicateDecision::Skip => Ok(None),
        SftpDuplicateDecision::Rename => resolve_renamed_local_target(context.local_path).map(Some),
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
}

pub(super) async fn resolve_remote_write_target(
    context: SftpRemoteWriteTargetContext<'_>,
) -> anyhow::Result<Option<String>> {
    if !context
        .sftp
        .try_exists_bytes(context.codec.encode_path(context.remote_path)?)
        .await?
    {
        return Ok(Some(context.remote_path.to_string()));
    }

    let local_path = context.local_path.display().to_string();
    let decision = resolve_duplicate_decision_for_path(
        context.path_options,
        SftpDuplicateCacheKey::Upload {
            local_path: context.local_path.to_path_buf(),
            remote_path: context.remote_path.to_string(),
            is_directory: context.is_directory,
        },
        SftpTransferDirection::Upload,
        &local_path,
        context.remote_path,
        context.is_directory,
    )?;
    match decision {
        SftpDuplicateDecision::Overwrite => Ok(Some(context.remote_path.to_string())),
        SftpDuplicateDecision::Skip => Ok(None),
        SftpDuplicateDecision::Rename => Ok(Some(
            resolve_renamed_remote_target(context.sftp, context.codec, context.remote_path).await?,
        )),
    }
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

fn resolve_renamed_local_target(local_path: &Path) -> anyhow::Result<PathBuf> {
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
        if !candidate.exists() {
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
