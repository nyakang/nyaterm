use nyaterm_transport::{SftpDuplicateDecision, SftpDuplicatePolicy, SftpTransferProgress};

use crate::models::TransferJobKind;

pub(in crate::features) fn duplicate_policy_label(policy: SftpDuplicatePolicy) -> &'static str {
    match policy {
        SftpDuplicatePolicy::Ask => "ask",
        SftpDuplicatePolicy::Overwrite => "overwrite",
        SftpDuplicatePolicy::Skip => "skip",
        SftpDuplicatePolicy::Rename => "rename",
    }
}

pub(in crate::features) fn transfer_job_title(kind: &TransferJobKind) -> String {
    match kind {
        TransferJobKind::ListTree { path, .. } => format!("List tree {}", path.display_path),
        TransferJobKind::ListDir { remote_path, .. } => format!("List {remote_path}"),
        TransferJobKind::ListChildren { remote_path } => {
            format!("List child directories in {remote_path}")
        }
        TransferJobKind::ResolveHome => "Resolve remote home".to_string(),
        TransferJobKind::SyncCwd => "Sync remote cwd".to_string(),
        TransferJobKind::Download {
            remote_path,
            local_path,
            ..
        } => format!("Download {remote_path} -> {}", local_path.display()),
        TransferJobKind::Upload {
            local_path,
            remote_path,
        } => format!("Upload {} -> {remote_path}", local_path.display()),
        TransferJobKind::Rename {
            old_path, new_path, ..
        } => format!("Rename {old_path} -> {new_path}"),
        TransferJobKind::Move {
            old_path, new_path, ..
        } => format!("Move {old_path} -> {new_path}"),
        TransferJobKind::Delete { remote_path, .. } => format!("Delete {remote_path}"),
        TransferJobKind::SendTo {
            source_path,
            target_path,
            ..
        } => format!("Send {source_path} -> {target_path}"),
        TransferJobKind::Mkdir { remote_path, .. } => format!("Create folder {remote_path}"),
        TransferJobKind::CreateFile { remote_path, .. } => format!("Create file {remote_path}"),
        TransferJobKind::Symlink {
            link_path,
            target_path,
            ..
        } => format!("Symlink {link_path} -> {target_path}"),
        TransferJobKind::LoadProperties { remote_path } => {
            format!("Load properties {remote_path}")
        }
        TransferJobKind::UpdateProperties { remote_path, .. } => {
            format!("Update properties {remote_path}")
        }
        TransferJobKind::LoadEditor { remote_path, .. } => format!("Open text {remote_path}"),
        TransferJobKind::LoadPreview { remote_path, .. } => format!("Preview {remote_path}"),
        TransferJobKind::RasterizePdfPage {
            remote_path,
            page_index,
            ..
        } => format!("Render PDF page {} {remote_path}", page_index + 1),
        TransferJobKind::SaveEditor { remote_path, .. } => format!("Save text {remote_path}"),
        TransferJobKind::OpenExternal {
            remote_path,
            local_path,
        } => format!("Open {remote_path} -> {}", local_path.display()),
        TransferJobKind::AiFileAction {
            remote_path,
            action_name,
            ..
        } => format!("AI {action_name} <- {remote_path}"),
        TransferJobKind::ZmodemUpload {
            file_name,
            session_id,
        } => format!("ZMODEM ↑ {file_name} ({session_id})"),
        TransferJobKind::XmodemUpload {
            file_name,
            session_id,
        } => {
            format!("XMODEM ↑ {file_name} ({session_id})")
        }
        TransferJobKind::YmodemUpload {
            file_name,
            session_id,
        } => {
            format!("YMODEM ↑ {file_name} ({session_id})")
        }
        TransferJobKind::ZmodemDownload {
            file_name,
            session_id,
        } => format!("ZMODEM ↓ {file_name} ({session_id})"),
        TransferJobKind::TrzszDownload {
            file_name,
            session_id,
        } => format!("trzsz ↓ {file_name} ({session_id})"),
        TransferJobKind::TrzszUpload {
            file_name,
            session_id,
        } => format!("trzsz ↑ {file_name} ({session_id})"),
        TransferJobKind::RdpClipboard { file_name } => format!("RDP clipboard {file_name}"),
        TransferJobKind::ZmodemConflictProbe {
            session_id,
            remote_dir,
        } => format!("ZMODEM probe {remote_dir} ({session_id})"),
    }
}

pub(in crate::features) fn format_transfer_progress(progress: &SftpTransferProgress) -> String {
    let transferred = format_file_size(Some(progress.bytes_transferred));
    match progress.total_bytes.filter(|total| *total > 0) {
        Some(total) => {
            let percent = (progress.bytes_transferred as f64 / total as f64 * 100.).clamp(0., 100.);
            format!(
                "{transferred} / {} ({percent:.0}%)",
                format_file_size(Some(total))
            )
        }
        None => format!("{transferred} transferred"),
    }
}

pub(in crate::features) fn format_file_size(size: Option<u64>) -> String {
    let Some(size) = size else {
        return String::new();
    };
    if size >= 1024 * 1024 {
        format!("{:.1} MiB", size as f64 / 1024. / 1024.)
    } else if size >= 1024 {
        format!("{:.1} KiB", size as f64 / 1024.)
    } else {
        format!("{size} B")
    }
}

pub(in crate::features) fn duplicate_decision_label(
    decision: SftpDuplicateDecision,
) -> &'static str {
    match decision {
        SftpDuplicateDecision::Overwrite => "overwrite",
        SftpDuplicateDecision::Skip => "skip",
        SftpDuplicateDecision::Rename => "rename",
    }
}
