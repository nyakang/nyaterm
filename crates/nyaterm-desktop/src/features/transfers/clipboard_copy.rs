//! Map file-service results to the application's transfer queue events.
use crate::models::TransferJobOutput;
use nyaterm_files::operations::{FileOperationResult, FileOperationSummary};

pub(in crate::features) fn file_operation_output(
    result: FileOperationResult,
    target_session_id: String,
) -> TransferJobOutput {
    match result {
        FileOperationResult::Skipped(summary) => TransferJobOutput::Summary(summary),
        FileOperationResult::Applied {
            source_path,
            target_path,
        } => TransferJobOutput::Sent {
            source_path,
            target_path,
            source_parent_path: None,
            source_entries: None,
            target_session_id: String::new(),
            target_parent_path: String::new(),
            bytes: 0,
            used_local_staging: false,
            entries: Vec::new(),
        },
        FileOperationResult::Completed(FileOperationSummary {
            source_path,
            target_path,
            source_parent_path,
            source_entries,
            target_parent_path,
            bytes,
            used_local_staging,
            entries,
        }) => TransferJobOutput::Sent {
            source_path,
            target_path,
            source_parent_path,
            source_entries,
            target_session_id,
            target_parent_path,
            bytes,
            used_local_staging,
            entries,
        },
    }
}
