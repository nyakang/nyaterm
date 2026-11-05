use super::file_operations::FileOperationEndpoint;
use crate::features::NyaTermApp;
use gpui::{Context, Window};
use nyaterm_files::sftp::SftpFileEntry;

#[derive(Clone, Copy)]
pub(in crate::features) enum FileOpenMode {
    Default,
    Preview,
    Edit,
}

impl NyaTermApp {
    pub(in crate::features) fn open_file_for_endpoint(
        &mut self,
        endpoint: FileOperationEndpoint,
        entry: SftpFileEntry,
        mode: FileOpenMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Err(error) = self.file_browser_service_for_endpoint(&endpoint) {
            self.shell.set_status(error.to_string());
            cx.notify();
            return;
        }
        let session = endpoint.session_id().map(str::to_string);
        match mode {
            FileOpenMode::Preview => {
                self.open_transfer_preview_for_session(entry, session, window, cx)
            }
            FileOpenMode::Edit => self.open_transfer_editor_for_session(entry, session, window, cx),
            FileOpenMode::Default if self.settings.summary().transfer_editor_type == "internal" => {
                self.open_transfer_editor_for_session(entry, session, window, cx)
            }
            FileOpenMode::Default => self.open_transfer_external_for_session(entry, session, cx),
        }
    }
}
