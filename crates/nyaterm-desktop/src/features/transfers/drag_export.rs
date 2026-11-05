use crate::{features::NyaTermApp, models::TransferJobResult};
use futures::channel::mpsc::UnboundedSender;
use gpui::{
    Context, DeferredVirtualFileDragPayload, ExternalDragPayload, FileDragPaths,
    PromisedFileDescriptor, PromisedFileDragPayload, VirtualFileDescriptor, VirtualFileDragPayload,
    VirtualFileProvider, VirtualFileStream, VirtualFileTreeProvider,
};
use nyaterm_files::drag_export::{
    DragExportSource, RemoteDragFile, SftpReadSource, SftpReadStream,
};
use nyaterm_files::file_browser::FileBrowserBackendKind;
use nyaterm_files::remote_file::RemoteFileService;
use nyaterm_files::sftp::{SftpFileEntry, SftpFileType};
use nyaterm_files::sftp_transfer_types::SftpTransferOptions;
use std::{
    cell::OnceCell,
    path::PathBuf,
    rc::Rc,
    sync::{Arc, Weak},
    time::{Duration, UNIX_EPOCH},
};

/// Immutable gesture snapshot: switching tabs or updating selection after the
/// drag starts cannot redirect its source connection or remote path.
#[derive(Clone)]
pub(in crate::features) struct TransferSelection {
    session_id: String,
    backend: FileBrowserBackendKind,
    entries: Vec<SftpFileEntry>,
    transfer_options: SftpTransferOptions,
    source_service: Option<Weak<RemoteFileService>>,
}

#[derive(Clone)]
pub(in crate::features) struct DraggedSelection {
    pub anchor: String,
    tree_entry: Option<SftpFileEntry>,
    pub selection: Rc<OnceCell<TransferSelection>>,
}
impl DraggedSelection {
    pub fn new(anchor: String) -> Self {
        Self {
            anchor,
            tree_entry: None,
            selection: Rc::new(OnceCell::new()),
        }
    }
    pub fn new_tree(entry: SftpFileEntry) -> Self {
        Self {
            anchor: entry.identity_key(),
            tree_entry: Some(entry),
            selection: Rc::new(OnceCell::new()),
        }
    }
    pub fn file_count(&self) -> usize {
        self.selection
            .get()
            .map_or(0, |selection| selection.entries.len())
    }
}

pub(in crate::features) fn transfer_drag_supported(
    local: bool,
    virtual_files: bool,
    promises: bool,
) -> bool {
    local || !cfg!(target_os = "linux") && (virtual_files || promises)
}

pub(in crate::features) struct TransferDragExportService;

impl TransferDragExportService {
    fn sources(selection: &TransferSelection) -> anyhow::Result<Vec<DragExportSource>> {
        anyhow::ensure!(!selection.entries.is_empty(), "no files selected");
        selection
            .entries
            .iter()
            .map(|entry| {
                if selection.backend == FileBrowserBackendKind::Local {
                    Ok(DragExportSource::Local {
                        path: PathBuf::from(&entry.path),
                        is_directory: entry.is_directory(),
                    })
                } else {
                    anyhow::ensure!(
                        matches!(
                            entry.file_type,
                            SftpFileType::File | SftpFileType::Directory
                        ),
                        "Links and special files cannot be dragged out; use Download"
                    );
                    Ok(DragExportSource::RemoteFile(RemoteDragFile {
                        display_name: entry.name.clone().into(),
                        size: entry.size,
                        remote_path: entry.remote_path(),
                        modified_at: entry
                            .modified_at
                            .map(|time| UNIX_EPOCH + Duration::from_secs(u64::from(time))),
                    }))
                }
            })
            .collect()
    }

    fn resolve(
        selection: &TransferSelection,
        service: Option<Weak<RemoteFileService>>,
        sender: UnboundedSender<TransferJobResult>,
        virtual_supported: bool,
        promise_supported: bool,
    ) -> anyhow::Result<ExternalDragPayload> {
        let sources = Self::sources(selection)?;
        if selection.backend == FileBrowserBackendKind::Local {
            return Ok(ExternalDragPayload::Files(FileDragPaths::new(
                sources.into_iter().filter_map(|source| {
                    if let DragExportSource::Local { path, is_directory } = source {
                        Some((path, is_directory))
                    } else {
                        None
                    }
                }),
            )));
        }
        anyhow::ensure!(
            !cfg!(target_os = "linux"),
            "{}",
            rust_i18n::t!("fileExplorer.dragPlatformUnsupported")
        );
        if promise_supported {
            let service = service.ok_or_else(|| anyhow::anyhow!("source session is closed"))?;
            let controls: Vec<_> = selection
                .entries
                .iter()
                .map(|_| nyaterm_files::sftp::SftpTransferControl::new())
                .collect();
            let lifetime = Arc::new(nyaterm_files::drag_export::DragSourceLifetime::new(
                service.clone(),
                controls.clone(),
            )?);
            let files = selection
                .entries
                .iter()
                .zip(controls)
                .map(|(entry, control)| PromisedFileDescriptor {
                    name: entry.name.clone().into(),
                    is_directory: entry.file_type == SftpFileType::Directory,
                    provider: Arc::new(super::drag_download::RemoteDownloadProvider::new(
                        selection.session_id.clone(),
                        service.clone(),
                        entry.clone(),
                        selection.transfer_options.clone(),
                        sender.clone(),
                        control,
                        lifetime.clone(),
                    )),
                })
                .collect();
            return Ok(ExternalDragPayload::PromisedFiles(
                PromisedFileDragPayload::new(files)?,
            ));
        }
        anyhow::ensure!(
            virtual_supported,
            "{}",
            rust_i18n::t!("fileExplorer.dragPlatformUnsupported")
        );
        let service = service.ok_or_else(|| anyhow::anyhow!("source session is closed"))?;
        let aggregate = super::drag_aggregate::DragAggregate::new(
            selection.session_id.clone(),
            &selection.entries,
            sender.clone(),
        );
        let controls = aggregate.controls();
        let enumeration_control = nyaterm_files::sftp::SftpTransferControl::new();
        let mut watched = controls.clone();
        watched.push(enumeration_control.clone());
        let lifetime = Arc::new(nyaterm_files::drag_export::DragSourceLifetime::new(
            service.clone(),
            watched,
        )?);
        if selection
            .entries
            .iter()
            .any(|entry| entry.file_type == SftpFileType::Directory)
        {
            return Ok(ExternalDragPayload::VirtualFileTree(
                DeferredVirtualFileDragPayload::new(Arc::new(RemoteTreeProvider {
                    selection: selection.clone(),
                    service,
                    aggregate: aggregate.clone(),
                    controls,
                    lifetime,
                    control: enumeration_control,
                }))
                .with_observer(aggregate),
            ));
        }
        let mut files = Vec::with_capacity(sources.len());
        for (root_index, source) in sources.into_iter().enumerate() {
            let DragExportSource::RemoteFile(file) = source else {
                anyhow::bail!("mixed local and remote selection is unsupported");
            };
            aggregate.register(root_index, &file);
            files.push(Self::descriptor(
                selection,
                service.clone(),
                aggregate.clone(),
                controls[root_index].clone(),
                lifetime.clone(),
                root_index,
                file,
            ));
        }
        Ok(ExternalDragPayload::VirtualFiles(
            VirtualFileDragPayload::new(files)?.with_observer(aggregate),
        ))
    }
    fn descriptor(
        selection: &TransferSelection,
        service: Weak<RemoteFileService>,
        aggregate: Arc<super::drag_aggregate::DragAggregate>,
        parent: nyaterm_files::sftp::SftpTransferControl,
        lifetime: Arc<nyaterm_files::drag_export::DragSourceLifetime>,
        root_index: usize,
        file: RemoteDragFile,
    ) -> VirtualFileDescriptor {
        let observed_file = file.clone();
        let factory = Arc::new(move || aggregate.observer(root_index, observed_file.clone()));
        VirtualFileDescriptor {
            name: file.display_name.clone(),
            is_directory: false,
            size: file.size,
            modified_at: file.modified_at,
            provider: Arc::new(RemoteProvider {
                source: SftpReadSource::new(service, file, factory)
                    .with_transfer_options(selection.transfer_options.clone())
                    .with_parent_control(parent),
                _lifetime: lifetime,
            }),
        }
    }
}

struct RemoteTreeProvider {
    selection: TransferSelection,
    service: Weak<RemoteFileService>,
    aggregate: Arc<super::drag_aggregate::DragAggregate>,
    controls: Vec<nyaterm_files::sftp::SftpTransferControl>,
    lifetime: Arc<nyaterm_files::drag_export::DragSourceLifetime>,
    control: nyaterm_files::sftp::SftpTransferControl,
}
impl VirtualFileTreeProvider for RemoteTreeProvider {
    fn load(&self) -> std::io::Result<VirtualFileDragPayload> {
        let service = self
            .service
            .upgrade()
            .ok_or_else(|| std::io::Error::other("source session is closed"))?;
        let source_snapshot = (*service).clone();
        drop(service);
        let entries = nyaterm_files::drag_export::tree::enumerate_remote_drag_with_root_controls(
            &source_snapshot,
            self.selection.entries.clone(),
            &self.control,
            self.controls.clone(),
        )
        .map_err(std::io::Error::other)?;
        let mut files = Vec::with_capacity(entries.len());
        for entry in entries {
            if !entry.is_directory {
                self.aggregate.register(entry.root_index, &entry.file);
            }
            let mut descriptor = TransferDragExportService::descriptor(
                &self.selection,
                self.service.clone(),
                self.aggregate.clone(),
                self.controls[entry.root_index].clone(),
                self.lifetime.clone(),
                entry.root_index,
                entry.file,
            );
            descriptor.name = entry.relative_path.into_os_string();
            descriptor.is_directory = entry.is_directory;
            files.push(descriptor);
        }
        VirtualFileDragPayload::new_tree(files)
    }
    fn cancel(&self) {
        self.control.cancel();
        for control in &self.controls {
            control.cancel();
        }
    }
}

struct RemoteProvider {
    source: SftpReadSource,
    _lifetime: Arc<nyaterm_files::drag_export::DragSourceLifetime>,
}
impl VirtualFileProvider for RemoteProvider {
    fn open(&self) -> std::io::Result<Box<dyn VirtualFileStream>> {
        Ok(Box::new(RemoteStream(self.source.open()?)))
    }
    fn cancel(&self) {
        self.source.cancel();
    }
}
struct RemoteStream(SftpReadStream);
impl VirtualFileStream for RemoteStream {
    fn read_at(&mut self, offset: u64, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.0.read_at(offset, buffer)
    }
    fn cancel(&self) {
        self.0.cancel();
    }
}

impl NyaTermApp {
    pub(in crate::features) fn capture_transfer_drag(
        &mut self,
        drag: &DraggedSelection,
        cx: &mut Context<Self>,
    ) {
        let Some(session_id) = self.session.active_id_owned() else {
            return;
        };
        let Some(backend) = self.session.active_file_browser_backend() else {
            return;
        };
        let entries = if let Some(anchor) = &drag.tree_entry {
            let selected = self.transfer.selected_tree_entries(&session_id);
            if selected
                .iter()
                .any(|entry| entry.matches_identity(&drag.anchor))
            {
                selected
            } else {
                vec![anchor.clone()]
            }
        } else {
            let browser = self.transfer.browser_view();
            let marked = browser.selected_remote_paths;
            browser
                .entries
                .iter()
                .filter(|entry| {
                    if marked.contains(&drag.anchor) {
                        marked.contains(&entry.identity_key())
                    } else {
                        entry.matches_identity(&drag.anchor)
                    }
                })
                .cloned()
                .collect()
        };
        let entries = match nyaterm_files::drag_export::tree::prune_nested_roots(entries) {
            Ok(entries) => entries,
            Err(error) => {
                self.shell.set_status(error.to_string());
                cx.notify();
                return;
            }
        };
        let _ = drag.selection.set(TransferSelection {
            source_service: self.session.weak_remote_file_service(&session_id),
            session_id,
            backend,
            entries,
            transfer_options: self.sftp_transfer_options(),
        });
        self.transfer.clear_browser_drag_selection();
        self.transfer.clear_browser_rename_click();
        self.transfer.cancel_browser_pending_rename();
        cx.notify();
    }

    pub(in crate::features) fn resolve_transfer_drag(
        &mut self,
        drag: &DraggedSelection,
        virtual_supported: bool,
        promise_supported: bool,
        cx: &mut Context<Self>,
    ) -> Option<ExternalDragPayload> {
        let selection = drag.selection.get()?;
        self.session.metadata(&selection.session_id)?;
        if selection.backend == FileBrowserBackendKind::Remote {
            let service = selection.source_service.as_ref().and_then(Weak::upgrade)?;
            if service.selected_backend()
                != Some(nyaterm_files::remote_file::RemoteFileBackendKind::Sftp)
            {
                self.shell
                    .set_status("Remote drag export requires SFTP; use Download".to_string());
                cx.notify();
                return None;
            }
        }
        match TransferDragExportService::resolve(
            selection,
            selection.source_service.clone(),
            self.transfer.transfer_event_sender(),
            virtual_supported,
            promise_supported,
        ) {
            Ok(payload) => Some(payload),
            Err(error) => {
                self.shell.set_status(error.to_string());
                cx.notify();
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{TransferDragExportService, TransferSelection};
    use nyaterm_files::drag_export::DragExportSource;
    use nyaterm_files::file_browser::FileBrowserBackendKind;
    use nyaterm_files::sftp::{SftpFileEntry, SftpFileType};
    fn entry(name: &str, kind: SftpFileType) -> SftpFileEntry {
        SftpFileEntry {
            name: name.into(),
            path: format!("/remote/{name}"),
            file_type: kind,
            size: Some(0),
            permissions: None,
            owner: String::new(),
            group: String::new(),
            modified_at: Some(17),
            raw_path_token: Some("L3JlbW90ZS9yYXc".into()),
            symlink_target_is_directory: false,
        }
    }
    #[test]
    fn list_and_tree_capability_keep_local_drags_and_disable_linux_remote_drags() {
        assert!(super::transfer_drag_supported(true, false, false));
        assert!(!super::transfer_drag_supported(false, false, false));
        assert_eq!(
            super::transfer_drag_supported(false, true, true),
            !cfg!(target_os = "linux")
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_resolver_rejects_remote_sources_even_when_native_flags_are_supplied() {
        let selection = TransferSelection {
            session_id: "session".into(),
            source_service: None,
            transfer_options: Default::default(),
            backend: FileBrowserBackendKind::Remote,
            entries: vec![entry("folder", SftpFileType::Directory)],
        };
        let (sender, mut receiver) = futures::channel::mpsc::unbounded();
        assert!(TransferDragExportService::resolve(&selection, None, sender, true, true).is_err());
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn remote_metadata_preserves_raw_path_and_has_no_destination() {
        let selection = TransferSelection {
            session_id: "session".into(),
            source_service: None,
            transfer_options: nyaterm_files::sftp_transfer_types::SftpTransferOptions::default(),
            backend: FileBrowserBackendKind::Remote,
            entries: vec![entry("你好.txt", SftpFileType::File)],
        };
        let sources = TransferDragExportService::sources(&selection).unwrap();
        let DragExportSource::RemoteFile(file) = &sources[0] else {
            panic!();
        };
        assert_eq!(file.display_name, std::ffi::OsString::from("你好.txt"));
        assert_eq!(file.size, Some(0));
        assert_eq!(file.remote_path, selection.entries[0].remote_path());
        assert_eq!(
            file.modified_at
                .unwrap()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            17
        );
    }
    #[test]
    fn remote_link_or_special_file_rejects_whole_selection() {
        for kind in [SftpFileType::Symlink, SftpFileType::Other] {
            let selection = TransferSelection {
                session_id: "session".into(),
                source_service: None,
                transfer_options: nyaterm_files::sftp_transfer_types::SftpTransferOptions::default(
                ),
                backend: FileBrowserBackendKind::Remote,
                entries: vec![entry("file", SftpFileType::File), entry("other", kind)],
            };
            assert!(TransferDragExportService::sources(&selection).is_err());
        }
    }
    #[cfg(not(target_os = "linux"))]
    #[test]
    fn directory_selection_uses_worker_tree_or_native_promise_without_touching_content() {
        let selection = TransferSelection {
            session_id: "session".into(),
            source_service: None,
            transfer_options: nyaterm_files::sftp_transfer_types::SftpTransferOptions::default(),
            backend: FileBrowserBackendKind::Remote,
            entries: vec![
                entry("folder", SftpFileType::Directory),
                entry("data.txt", SftpFileType::File),
            ],
        };
        let (sender, mut receiver) = futures::channel::mpsc::unbounded();
        let service = std::sync::Arc::new(nyaterm_files::remote_file::RemoteFileService::new(
            nyaterm_transport::SshSessionConfig::default(),
        ));
        let gpui::ExternalDragPayload::VirtualFileTree(tree) = TransferDragExportService::resolve(
            &selection,
            Some(std::sync::Arc::downgrade(&service)),
            sender.clone(),
            true,
            false,
        )
        .unwrap() else {
            panic!("directory requires tree");
        };
        assert!(receiver.try_recv().is_err());
        tree.cancel();
        assert!(tree.resolve().is_err());
        let gpui::ExternalDragPayload::PromisedFiles(promises) =
            TransferDragExportService::resolve(
                &selection,
                Some(std::sync::Arc::downgrade(&service)),
                sender,
                false,
                true,
            )
            .unwrap()
        else {
            panic!("Finder requires promises");
        };
        assert_eq!(promises.files().len(), 2);
        assert!(promises.files()[0].is_directory);
        assert!(!promises.files()[1].is_directory);
        assert!(receiver.try_recv().is_err());
        drop(service);
        promises.cancel();
        assert!(
            promises.files()[0]
                .provider
                .write_to(std::path::Path::new("unused"))
                .is_err()
        );
        assert!(receiver.try_recv().is_err());
    }
    #[test]
    fn local_selection_maps_to_real_paths_including_directories() {
        let selection = TransferSelection {
            session_id: "session".into(),
            source_service: None,
            transfer_options: nyaterm_files::sftp_transfer_types::SftpTransferOptions::default(),
            backend: FileBrowserBackendKind::Local,
            entries: vec![entry("dir", SftpFileType::Directory)],
        };
        let (sender, _) = futures::channel::mpsc::unbounded();
        let gpui::ExternalDragPayload::Files(paths) =
            TransferDragExportService::resolve(&selection, None, sender, false, false).unwrap()
        else {
            panic!();
        };
        assert_eq!(
            paths.entries(),
            &[(std::path::PathBuf::from("/remote/dir"), true)]
        );
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn remote_selection_is_deferred_and_keeps_native_content_order() {
        let selection = TransferSelection {
            session_id: "session".into(),
            source_service: None,
            transfer_options: nyaterm_files::sftp_transfer_types::SftpTransferOptions::default(),
            backend: FileBrowserBackendKind::Remote,
            entries: vec![
                entry("你好.txt", SftpFileType::File),
                entry("second", SftpFileType::File),
            ],
        };
        let (sender, mut receiver) = futures::channel::mpsc::unbounded();
        let service = std::sync::Arc::new(nyaterm_files::remote_file::RemoteFileService::new(
            nyaterm_transport::SshSessionConfig::default(),
        ));
        let gpui::ExternalDragPayload::VirtualFiles(files) = TransferDragExportService::resolve(
            &selection,
            Some(std::sync::Arc::downgrade(&service)),
            sender,
            true,
            false,
        )
        .unwrap() else {
            panic!("remote files must be deferred");
        };
        assert_eq!(files.files().len(), 2);
        assert_eq!(files.files()[0].name, std::ffi::OsString::from("你好.txt"));
        assert_eq!(files.files()[1].name, std::ffi::OsString::from("second"));
        assert!(receiver.try_recv().is_err());
        drop(service);
        // Closing the source before a consumer opens it fails without networking.
        assert!(files.files()[0].provider.open().is_err());
        files.cancel();
        assert!(files.files()[1].provider.open().is_err());
    }

    #[test]
    fn unsafe_remote_names_and_unsupported_platform_reject_before_content() {
        for name in ["../escape", "NUL.txt", "name:stream"] {
            let selection = TransferSelection {
                session_id: "session".into(),
                source_service: None,
                transfer_options: nyaterm_files::sftp_transfer_types::SftpTransferOptions::default(
                ),
                backend: FileBrowserBackendKind::Remote,
                entries: vec![entry(name, SftpFileType::File)],
            };
            let (sender, _) = futures::channel::mpsc::unbounded();
            assert!(
                TransferDragExportService::resolve(
                    &selection,
                    Some(std::sync::Weak::new()),
                    sender,
                    true,
                    false
                )
                .is_err()
            );
        }
        let selection = TransferSelection {
            session_id: "session".into(),
            source_service: None,
            transfer_options: nyaterm_files::sftp_transfer_types::SftpTransferOptions::default(),
            backend: FileBrowserBackendKind::Remote,
            entries: vec![entry("file", SftpFileType::File)],
        };
        let (sender, _) = futures::channel::mpsc::unbounded();
        assert!(
            TransferDragExportService::resolve(&selection, None, sender, false, false).is_err()
        );
    }

    #[test]
    fn gesture_snapshot_cannot_be_redirected_after_capture() {
        let drag = super::DraggedSelection::new("anchor".into());
        let service = std::sync::Arc::new(nyaterm_files::remote_file::RemoteFileService::new(
            nyaterm_transport::SshSessionConfig::default(),
        ));
        let selection = TransferSelection {
            session_id: "original".into(),
            source_service: Some(std::sync::Arc::downgrade(&service)),
            transfer_options: nyaterm_files::sftp_transfer_types::SftpTransferOptions::default(),
            backend: FileBrowserBackendKind::Remote,
            entries: vec![entry("original", SftpFileType::File)],
        };
        assert!(drag.selection.set(selection.clone()).is_ok());
        let mut changed = selection;
        changed.session_id = "other".into();
        changed.source_service = None;
        changed.entries.clear();
        changed.transfer_options = changed.transfer_options.with_download_threads(9);
        assert!(drag.clone().selection.set(changed).is_err());
        assert_eq!(drag.selection.get().unwrap().session_id, "original");
        assert_eq!(drag.selection.get().unwrap().entries.len(), 1);
        assert_eq!(
            drag.selection
                .get()
                .unwrap()
                .transfer_options
                .download_threads(),
            3
        );
        assert!(std::sync::Weak::ptr_eq(
            drag.selection
                .get()
                .unwrap()
                .source_service
                .as_ref()
                .unwrap(),
            &std::sync::Arc::downgrade(&service),
        ));
    }
}
