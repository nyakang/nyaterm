use crate::features::NyaTermApp;
use crate::features::transfers::file_operations::{
    FileOperationEndpoint, FileTransferIntent, FileTransferOperation,
};
use gpui::{Context, Window};
use nyaterm_files::file_browser::FileBrowserBackendKind;
use nyaterm_files::sftp::{RemoteFilePath, SftpFileEntry, SftpFileType};
use rust_i18n::t;
use std::path::PathBuf;

#[cfg(test)]
fn invalid_copy_destination(
    kind: FileBrowserBackendKind,
    source: &str,
    target: &str,
    directory: bool,
) -> bool {
    nyaterm_files::file_browser::file_browser_copy_destination_invalid(
        kind, source, target, directory,
    )
}

fn external_file_paths(cx: &gpui::App) -> Vec<PathBuf> {
    cx.read_from_clipboard()
        .and_then(|item| {
            item.entries().iter().find_map(|entry| match entry {
                gpui::ClipboardEntry::ExternalPaths(paths) => Some(paths.paths().to_vec()),
                _ => None,
            })
        })
        .unwrap_or_default()
}

impl NyaTermApp {
    pub(in crate::features) fn can_paste_transfer_file_clipboard(&self, cx: &gpui::App) -> bool {
        self.transfer
            .file_clipboard()
            .is_some_and(|clipboard| clipboard.matches_system_clipboard(cx))
            || !external_file_paths(cx).is_empty()
    }

    pub(in crate::features) fn capture_transfer_file_clipboard(
        &mut self,
        cut: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self.session.active_id_owned() else {
            return;
        };
        let endpoint =
            if self.session.active_file_browser_backend() == Some(FileBrowserBackendKind::Local) {
                FileOperationEndpoint::Local
            } else {
                FileOperationEndpoint::Session(id)
            };
        let entries = self.selected_transfer_entries();
        self.capture_file_clipboard(endpoint, entries, cut, cx);
    }

    pub(in crate::features) fn capture_file_clipboard(
        &mut self,
        source: FileOperationEndpoint,
        entries: Vec<SftpFileEntry>,
        cut: bool,
        cx: &mut Context<Self>,
    ) {
        if entries.is_empty() {
            self.report_transfer_entry_error(
                "file-clipboard",
                source.kind(),
                &nyaterm_files::file_browser::FileTransferEntryError::NoSelection.into(),
                cx,
            );
            return;
        }
        if entries.iter().any(|entry| {
            nyaterm_files::file_browser::file_browser_path_is_root(source.kind(), &entry.path)
                || matches!(entry.name.as_str(), "." | "..")
        }) {
            self.report_transfer_entry_error(
                "file-clipboard",
                source.kind(),
                &nyaterm_files::file_browser::FileTransferEntryError::InvalidDestination.into(),
                cx,
            );
            return;
        }
        let entries = if source.kind() == FileBrowserBackendKind::Local {
            entries
                .iter()
                .filter(|entry| {
                    !entries.iter().any(|parent| {
                        parent.path != entry.path
                            && parent.is_directory()
                            && nyaterm_files::file_browser::file_browser_copy_destination_invalid(
                                FileBrowserBackendKind::Local,
                                &parent.path,
                                &entry.path,
                                true,
                            )
                    })
                })
                .cloned()
                .collect()
        } else {
            entries
        };
        // Local listings carry a local identity token, not a base64 SFTP wire path.
        let entries = if source.kind() == FileBrowserBackendKind::Local {
            entries
        } else {
            match nyaterm_files::drag_export::tree::prune_nested_roots(entries) {
                Ok(entries) => entries,
                Err(error) => {
                    self.report_browser_transfer_error("file-clipboard", &error, cx);
                    return;
                }
            }
        };
        if entries.iter().any(|entry| {
            !matches!(
                entry.file_type,
                SftpFileType::File | SftpFileType::Directory
            )
        }) {
            self.report_browser_transfer_error(
                "file-clipboard",
                &nyaterm_files::file_browser::FileTransferEntryError::UnsupportedFileType.into(),
                cx,
            );
            return;
        }
        if cfg!(target_os = "windows") && cx.clipboard_revision().is_some() && !cut {
            if let Err(error) = self.publish_file_clipboard(&source, &entries, cx) {
                self.report_browser_transfer_error("file-clipboard", &error, cx);
                return;
            }
        } else {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string_with_metadata(
                String::new(),
                nyaterm_core::uuid(),
            ));
        }
        let revision = cx.clipboard_revision();
        let item = if revision.is_none() {
            cx.read_from_clipboard()
        } else {
            None
        };
        self.transfer
            .set_file_clipboard(source, entries, cut, item, revision);
        self.notify_operation_at(
            cx.active_window(),
            "file-clipboard",
            nyaterm_ui::notification::NyaNotificationKind::Success,
            t!("common.operationSucceeded").to_string(),
            cx,
        );
        cx.notify();
    }

    pub(in crate::features) fn paste_transfer_file_clipboard(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self.session.active_id_owned() else {
            return;
        };
        let target =
            if self.session.active_file_browser_backend() == Some(FileBrowserBackendKind::Local) {
                FileOperationEndpoint::Local
            } else {
                FileOperationEndpoint::Session(id)
            };
        self.paste_file_clipboard(
            target,
            RemoteFilePath::new(self.transfer_browser_operation_target_directory()),
            cx,
        );
    }

    pub(in crate::features) fn paste_file_clipboard(
        &mut self,
        target: FileOperationEndpoint,
        directory: RemoteFilePath,
        cx: &mut Context<Self>,
    ) {
        let internal = self
            .transfer
            .file_clipboard()
            .cloned()
            .filter(|clipboard| clipboard.matches_system_clipboard(cx));
        let Some(clipboard) = internal else {
            let paths = external_file_paths(cx);
            if !paths.is_empty() {
                self.upload_external_paths_to_endpoint(target, directory, paths, cx);
            } else {
                self.notify_operation_at(
                    cx.active_window(),
                    "file-clipboard",
                    nyaterm_ui::notification::NyaNotificationKind::Warning,
                    t!("fileExplorer.clipboardEmpty").to_string(),
                    cx,
                );
            }
            return;
        };
        let entries = clipboard
            .entries
            .into_iter()
            .filter(|entry| {
                !clipboard.cut
                    || !self
                        .transfer
                        .cut_path_in_flight(clipboard.generation, &entry.identity_key())
            })
            .collect::<Vec<_>>();
        if entries.is_empty() {
            self.notify_operation_at(
                cx.active_window(),
                "file-clipboard",
                nyaterm_ui::notification::NyaNotificationKind::Warning,
                t!("fileExplorer.clipboardMoving").to_string(),
                cx,
            );
            return;
        }
        self.submit_file_transfer(
            "file-clipboard".into(),
            FileTransferIntent {
                source: clipboard.source,
                entries,
                target,
                target_directory: directory,
                operation: if clipboard.cut {
                    FileTransferOperation::Move
                } else {
                    FileTransferOperation::Copy
                },
            },
            cx,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::invalid_copy_destination;
    use nyaterm_files::file_browser::FileBrowserBackendKind;
    #[test]
    fn remote_copy_rejects_self_descendant_and_root_after_normalization() {
        for (source, target, directory) in [
            ("/src", "/src/./", false),
            ("/src", "/src/child/file", true),
            ("/", "/other", true),
        ] {
            assert!(invalid_copy_destination(
                FileBrowserBackendKind::Remote,
                source,
                target,
                directory
            ));
        }
        assert!(!invalid_copy_destination(
            FileBrowserBackendKind::Remote,
            "/src",
            "/other/file",
            true
        ));
    }
}
