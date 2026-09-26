use std::collections::BTreeMap;
use std::path::PathBuf;

use gpui::{Context, Window};
use nyaterm_transport::{
    FileBrowserBackendKind, FileBrowserService, FileCopyRequest, RemoteFilePath,
    SftpDuplicatePolicy, SftpDuplicateResolver, SftpFileEntry, SftpFileType,
    SftpPathTransferOptions, SftpTransferControl, file_browser_join, file_browser_name,
    file_browser_parent, file_browser_path_is_root,
};

use crate::features::NyaTermApp;
use crate::features::transfers::TransferFileClipboard;
use crate::models::{
    TransferJobEvent, TransferJobKind, TransferJobOutput, TransferJobResult, TransferJobState,
    TransferJobStatus,
};

type FileInventory = BTreeMap<String, (SftpFileType, Option<u64>, Option<u32>)>;

fn remote_parts(path: &str) -> Option<Vec<&str>> {
    if !path.starts_with('/') || path.contains('\0') {
        return None;
    }
    let mut parts = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            _ => parts.push(part),
        }
    }
    Some(parts)
}

fn invalid_copy_destination(source: &str, target: &str, directory: bool) -> bool {
    let (Some(source), Some(target)) = (remote_parts(source), remote_parts(target)) else {
        return true;
    };
    source.is_empty() || source == target || (directory && target.starts_with(&source))
}

fn inventory(
    service: &FileBrowserService,
    path: &RemoteFilePath,
    control: &SftpTransferControl,
) -> Result<FileInventory, String> {
    let mut found = BTreeMap::new();
    let mut pending = vec![(String::new(), path.clone())];
    while let Some((relative, path)) = pending.pop() {
        control
            .check_cancelled()
            .map_err(|error| error.to_string())?;
        let props = service
            .remote_file_properties(&path)
            .map_err(|error| error.to_string())?;
        if !matches!(
            props.file_type,
            SftpFileType::File | SftpFileType::Directory
        ) {
            return Err("cut does not support symbolic links or special files".into());
        }
        found.insert(
            relative.clone(),
            (props.file_type, props.size, props.modified_at),
        );
        if props.file_type == SftpFileType::Directory {
            for entry in service
                .list_dir_path(&path)
                .map_err(|error| error.to_string())?
            {
                if entry.name == "." || entry.name == ".." {
                    continue;
                }
                let child = file_browser_join(service.kind(), &path.display_path, &entry.name);
                pending.push((
                    format!("{relative}/{}", entry.name),
                    RemoteFilePath::new(child),
                ));
            }
        }
    }
    Ok(found)
}

fn check_no_symlink_ancestors(service: &FileBrowserService, path: &str) -> Result<(), String> {
    let parts = remote_parts(path).ok_or("cut requires absolute remote paths")?;
    let mut prefix = String::new();
    for part in parts {
        prefix.push('/');
        prefix.push_str(part);
        let props = service
            .remote_file_properties(&RemoteFilePath::new(&prefix))
            .map_err(|error| error.to_string())?;
        if props.file_type == SftpFileType::Symlink {
            return Err("cut through a symbolic link is not supported".into());
        }
    }
    Ok(())
}

fn copied_tree_matches(before: &FileInventory, after: &FileInventory) -> bool {
    before.len() == after.len()
        && before.iter().all(|(path, (kind, size, _))| {
            after.get(path).is_some_and(|(other_kind, other_size, _)| {
                kind == other_kind && (kind == &SftpFileType::Directory || size == other_size)
            })
        })
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

fn prefer_external_paths(paths: &[PathBuf], internal: Option<&TransferFileClipboard>) -> bool {
    !paths.is_empty() && internal.is_none_or(|clipboard| clipboard.os_paths_at_capture != paths)
}

fn local_clipboard_entry(path: PathBuf) -> Option<SftpFileEntry> {
    let name = path.file_name()?.to_string_lossy().into_owned();
    if !path.is_absolute() || matches!(name.as_str(), "." | "..") {
        return None;
    }
    Some(SftpFileEntry {
        name,
        path: path.to_string_lossy().into_owned(),
        file_type: SftpFileType::File,
        size: None,
        permissions: None,
        owner: String::new(),
        group: String::new(),
        modified_at: None,
        raw_path_token: None,
        symlink_target_is_directory: false,
    })
}

impl NyaTermApp {
    pub(in crate::features::pages::transfers) fn can_paste_transfer_file_clipboard(
        &self,
        cx: &gpui::App,
    ) -> bool {
        self.transfer
            .file_clipboard()
            .is_some_and(|clipboard| !clipboard.entries.is_empty())
            || !external_file_paths(cx).is_empty()
    }

    pub(in crate::features) fn capture_transfer_file_clipboard(
        &mut self,
        cut: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(source_session_id) = self.session.active_id_owned() else {
            return;
        };
        if cut && self.session.active_file_browser_backend() != Some(FileBrowserBackendKind::Remote)
        {
            self.shell
                .set_status("cut is only available for remote files");
            cx.notify();
            return;
        }
        let backend = self
            .session
            .active_file_browser_backend()
            .unwrap_or(FileBrowserBackendKind::Remote);
        let selected = self.selected_transfer_entries();
        let entries = selected
            .iter()
            .filter(|entry| {
                !entry.path.trim().is_empty()
                    && !file_browser_path_is_root(backend, &entry.path)
                    && !matches!(entry.name.as_str(), "." | "..")
                    && (!cut || !entry.is_symlink())
                    && !selected.iter().any(|parent| {
                        parent.path != entry.path
                            && parent.is_directory()
                            && backend == FileBrowserBackendKind::Remote
                            && remote_parts(&parent.path)
                                .zip(remote_parts(&entry.path))
                                .is_some_and(|(parent, child)| child.starts_with(&parent))
                    })
            })
            .cloned()
            .collect::<Vec<_>>();
        if entries.is_empty() {
            self.shell.set_status("select files to copy or cut");
            cx.notify();
            return;
        }
        let count = entries.len();
        self.transfer
            .set_file_clipboard(source_session_id, entries, cut, external_file_paths(cx));
        self.shell.set_status(format!(
            "{} {count} file item(s)",
            if cut { "cut" } else { "copied" }
        ));
        cx.notify();
    }

    pub(in crate::features) fn paste_transfer_file_clipboard(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(target_session_id) = self.session.active_id_owned() else {
            return;
        };
        let os_paths = external_file_paths(cx);
        let internal = self.transfer.file_clipboard().cloned();
        let prefer_local = prefer_external_paths(&os_paths, internal.as_ref());
        let clipboard = if prefer_local {
            TransferFileClipboard {
                source_session_id: target_session_id.clone(),
                entries: os_paths
                    .into_iter()
                    .filter_map(local_clipboard_entry)
                    .collect(),
                cut: false,
                generation: 0,
                os_paths_at_capture: Vec::new(),
            }
        } else if let Some(internal) = internal {
            internal
        } else {
            self.shell.set_status("file clipboard is empty");
            cx.notify();
            return;
        };
        if clipboard.entries.is_empty() {
            self.shell.set_status("file clipboard has no usable paths");
            cx.notify();
            return;
        }
        if self.session.is_disconnected(&target_session_id)
            || (!prefer_local && self.session.is_disconnected(&clipboard.source_session_id))
        {
            self.shell.set_status("file clipboard session disconnected");
            cx.notify();
            return;
        }
        let source_service = if prefer_local {
            Some(FileBrowserService::local())
        } else {
            self.file_browser_service_for_session(&clipboard.source_session_id)
                .ok()
        };
        let target_service = self
            .file_browser_service_for_session(&target_session_id)
            .ok();
        let (Some(source_service), Some(target_service)) = (source_service, target_service) else {
            self.shell
                .set_status("file clipboard session is unavailable");
            cx.notify();
            return;
        };
        if clipboard.cut && target_service.kind() != FileBrowserBackendKind::Remote {
            self.shell.set_status("cut requires a remote destination");
            cx.notify();
            return;
        }
        let source_config = self
            .session
            .metadata(&clipboard.source_session_id)
            .and_then(|metadata| metadata.ssh_config.as_ref());
        let target_config = self
            .session
            .metadata(&target_session_id)
            .and_then(|metadata| metadata.ssh_config.as_ref());
        let same_endpoint = clipboard.source_session_id == target_session_id
            || matches!((source_config, target_config), (Some(a), Some(b))
                if a.host.eq_ignore_ascii_case(&b.host) && a.port == b.port && a.username == b.username)
            || (source_service.kind() == FileBrowserBackendKind::Local
                && target_service.kind() == FileBrowserBackendKind::Local);
        let target_dir = self.transfer_browser_operation_target_directory();
        let duplicate_policy = if clipboard.cut {
            SftpDuplicatePolicy::Skip
        } else {
            self.transfer.duplicate_policy()
        };
        let duplicate_resolver = (duplicate_policy == SftpDuplicatePolicy::Ask).then(|| {
            self.session.prompt_duplicate_broker() as std::sync::Arc<dyn SftpDuplicateResolver>
        });
        let options = SftpPathTransferOptions::new(
            duplicate_policy,
            duplicate_resolver,
            self.sftp_transfer_options(),
        );
        let mut started = 0;
        for entry in clipboard.entries {
            let name = file_browser_name(source_service.kind(), &entry.path);
            let target_path = file_browser_join(target_service.kind(), &target_dir, &name);
            if same_endpoint
                && source_service.kind() == FileBrowserBackendKind::Remote
                && invalid_copy_destination(&entry.path, &target_path, entry.is_directory())
            {
                continue;
            }
            if same_endpoint
                && source_service.kind() == FileBrowserBackendKind::Local
                && std::path::Path::new(&target_path).starts_with(&entry.path)
            {
                continue;
            }
            let id = self.transfer.next_transfer_job_id("file-clipboard");
            let control = SftpTransferControl::new();
            self.transfer.enqueue_transfer_job(TransferJobState {
                id: id.clone(),
                session_id: Some(clipboard.source_session_id.clone()),
                kind: TransferJobKind::SendTo {
                    source_path: entry.path.clone(),
                    target_session_id: target_session_id.clone(),
                    target_path: target_path.clone(),
                    target_parent_path: target_dir.clone(),
                },
                status: TransferJobStatus::Running,
                detail: format!(
                    "{} {}",
                    if clipboard.cut { "Moving" } else { "Copying" },
                    entry.path
                ),
                created_at_ms: TransferJobState::now_ms(),
                display_name: String::new(),
                entries: Vec::new(),
                summary: None,
                progress: None,
                control: Some(control.clone()),
                speed: Default::default(),
            });
            if clipboard.cut {
                self.transfer
                    .track_cut_job(id.clone(), clipboard.generation, entry.path.clone());
            }
            let request = ClipboardCopyRequest {
                source_service: source_service.clone(),
                target_service: target_service.clone(),
                source: entry.remote_path(),
                target_path,
                target_session_id: target_session_id.clone(),
                cut: clipboard.cut,
                control,
                options: options.clone(),
            };
            let tx = self.transfer.transfer_event_sender();
            self.submit_transfer_blocking_job(
                "file-clipboard",
                id.clone(),
                tx.clone(),
                move || {
                    let result = run_clipboard_copy(request);
                    let _ = tx.unbounded_send(TransferJobResult {
                        id,
                        event: TransferJobEvent::Finished(result),
                    });
                },
            );
            started += 1;
        }
        self.shell
            .set_status(format!("started {started} file clipboard job(s)"));
        cx.notify();
    }
}

struct ClipboardCopyRequest {
    source_service: FileBrowserService,
    target_service: FileBrowserService,
    source: RemoteFilePath,
    target_path: String,
    target_session_id: String,
    cut: bool,
    control: SftpTransferControl,
    options: SftpPathTransferOptions,
}

fn run_clipboard_copy(request: ClipboardCopyRequest) -> Result<TransferJobOutput, String> {
    let ClipboardCopyRequest {
        source_service,
        target_service,
        source,
        target_path,
        target_session_id,
        cut,
        control,
        options,
    } = request;
    let before = if cut {
        let parent = file_browser_parent(source_service.kind(), &source.display_path);
        check_no_symlink_ancestors(&source_service, &parent)?;
        let target_parent = file_browser_parent(target_service.kind(), &target_path);
        check_no_symlink_ancestors(&target_service, &target_parent)?;
        let target_name = file_browser_name(target_service.kind(), &target_path);
        if target_service
            .list_dir(&target_parent)
            .map_err(|error| error.to_string())?
            .iter()
            .any(|entry| entry.name == target_name)
        {
            return Err("destination already exists; source was kept".into());
        }
        Some(inventory(&source_service, &source, &control)?)
    } else {
        None
    };
    let summary = FileCopyRequest {
        source: source_service.transfer_endpoint(&source),
        destination: target_service.transfer_endpoint(&RemoteFilePath::new(&target_path)),
        control: control.clone(),
        options,
    }
    .execute()
    .map_err(|error| error.to_string())?;
    if !summary.copied {
        return Err("copy skipped; source was kept".into());
    }
    let mut source_parent_path = None;
    let mut source_entries = None;
    if let Some(before) = before {
        control
            .check_cancelled()
            .map_err(|error| error.to_string())?;
        let after_source = inventory(&source_service, &source, &control)?;
        if after_source != before {
            return Err("source changed during copy; source was kept".into());
        }
        let destination = RemoteFilePath::new(&summary.destination_path);
        let after_target = inventory(&target_service, &destination, &control)?;
        if !copied_tree_matches(&before, &after_target) {
            return Err("copy verification failed; source was kept".into());
        }
        control
            .check_cancelled()
            .map_err(|error| error.to_string())?;
        source_service
            .delete_remote_path(&source)
            .map_err(|error| error.to_string())?;
        let parent = file_browser_parent(source_service.kind(), &source.display_path);
        source_entries = source_service.list_dir(&parent).ok();
        source_parent_path = Some(parent);
    }
    let target_parent_path = file_browser_parent(target_service.kind(), &summary.destination_path);
    let entries = target_service
        .list_dir(&target_parent_path)
        .unwrap_or_default();
    Ok(TransferJobOutput::Sent {
        source_path: source.display_path,
        source_parent_path,
        source_entries,
        target_session_id,
        target_path: summary.destination_path,
        target_parent_path,
        bytes: summary.bytes,
        used_local_staging: summary.used_local_staging,
        entries,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        TransferFileClipboard, copied_tree_matches, invalid_copy_destination,
        local_clipboard_entry, prefer_external_paths,
    };
    use nyaterm_transport::SftpFileType;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    #[test]
    fn remote_copy_rejects_self_descendant_and_root_after_normalization() {
        assert!(invalid_copy_destination("/src", "/src/./", false));
        assert!(invalid_copy_destination("/src", "/src/child/file", true));
        assert!(invalid_copy_destination("/", "/other", true));
        assert!(!invalid_copy_destination("/src", "/other/file", true));
    }

    #[test]
    fn cut_verification_checks_every_file_not_only_total_bytes() {
        let before = BTreeMap::from([
            (String::new(), (SftpFileType::Directory, None, None)),
            ("/a".into(), (SftpFileType::File, Some(3), Some(1))),
        ]);
        let mut after = before.clone();
        assert!(copied_tree_matches(&before, &after));
        after.insert("/a".into(), (SftpFileType::File, Some(2), Some(1)));
        assert!(!copied_tree_matches(&before, &after));
    }

    #[test]
    fn newer_external_paths_override_internal_clipboard() {
        let original = PathBuf::from("/original.txt");
        let clipboard = TransferFileClipboard {
            source_session_id: "source".into(),
            entries: Vec::new(),
            cut: false,
            generation: 1,
            os_paths_at_capture: vec![original.clone()],
        };
        assert!(!prefer_external_paths(&[original], Some(&clipboard)));
        assert!(prefer_external_paths(
            &[PathBuf::from("/new.txt")],
            Some(&clipboard)
        ));
        assert!(prefer_external_paths(&[PathBuf::from("/new.txt")], None));
        assert!(!prefer_external_paths(&[], None));
    }

    #[test]
    fn local_clipboard_paths_require_absolute_named_entries() {
        assert!(local_clipboard_entry(PathBuf::from("relative.txt")).is_none());
        assert!(local_clipboard_entry(PathBuf::from("/")).is_none());
        let entry = local_clipboard_entry(std::env::temp_dir().join("example.txt")).unwrap();
        assert_eq!(entry.name, "example.txt");
        assert_eq!(entry.file_type, SftpFileType::File);
    }
}
