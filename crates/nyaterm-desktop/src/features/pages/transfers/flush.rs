use std::sync::Arc;

use gpui::Context;

use crate::features::NyaTermApp;
use crate::models::{TransferJobRowSnapshot, TransferJobStatus};

use super::panel::{
    TransferBrowserPresentation, TransferChrome, TransferQueuePresentation, TransferSnapshot,
};
use super::transfer_browser_availability;

impl NyaTermApp {
    /// Flush the transfers panel once the current effect cycle ends.
    ///
    /// `cx.listener` and `cx.processor` both lease `TransferPanel` for the whole
    /// callback -- GPUI takes the entity out of its map -- so a flush that ran inline
    /// would reach back for `panel.update` and double-lease it, which aborts the
    /// process rather than unwinding. Deferring returns the lease first and still
    /// lands before any paint, so the panel is never drawn stale. The two sibling
    /// panels reach their flushes the same way.
    pub(in crate::features) fn defer_transfer_panel_snapshot_flush(&self, cx: &mut Context<Self>) {
        self.defer_app_update(cx, |app, cx| {
            app.flush_transfer_panel_snapshot(cx);
        });
    }

    fn transfer_chrome(&self) -> TransferChrome {
        let palette = self.theme_palette();
        TransferChrome {
            transparent_surface: self.shell_transparent_color(palette.surface),
            transparent_section_header: self.shell_transparent_color(palette.section_header),
            surface: self.shell_surface_color(palette.surface),
            palette,
        }
    }

    /// Rebuild the transfers panel snapshot.
    ///
    /// Never called from a render. The panel used to be drawn by re-entering the app
    /// from the root's own render, which made a paint the reconciliation pump; this
    /// runs at the boundaries that change something instead -- every panel
    /// interaction (through `TransferPanel::with_app`), every store reply, the theme,
    /// the panel-stack transitions, and each coalesced transfer-event batch.
    ///
    /// Unconditional rather than key-compared. The two costly derivations are already
    /// memoised behind their real inputs -- the listing in the state's filter cache,
    /// which a progress batch never invalidates -- so a rebuild on the hot path is a
    /// refcount bump plus a handful of small clones. Comparing a key would mean
    /// hashing or cloning the very things the memo exists to avoid touching.
    pub(in crate::features) fn flush_transfer_panel_snapshot(&mut self, cx: &mut Context<Self>) {
        let snapshot = self.build_transfer_snapshot();
        let panel = self.transfer_panel.clone();
        panel.update(cx, |panel, cx| panel.set_snapshot(snapshot, cx));
    }

    fn build_transfer_snapshot(&mut self) -> TransferSnapshot {
        let chrome = self.transfer_chrome();
        let active_session_id = self.session.active_id().map(str::to_string);
        let availability = transfer_browser_availability(
            active_session_id.is_some(),
            active_session_id.as_deref().is_some_and(|session_id| {
                self.session
                    .file_browser_backend_support_for_session(session_id)
                    .is_some()
            }),
            active_session_id
                .as_deref()
                .is_some_and(|session_id| self.session.is_disconnected(session_id)),
        );
        let panel_height = self.transfer.panel_height().clamp(60., 600.);
        let height_is_resizing = self.transfer.panel_height_is_resizing();
        let resize_handle_highlighted = self
            .shell
            .resize_handle_is_highlighted(&gpui::SharedString::from("transfer-height-resize"));
        let auto_sync_cwd_enabled = self.transfer_browser_auto_sync_cwd_enabled();
        let cwd_sync_demand = self.transfer_cwd_sync_needs_polling();
        let connection_id = self.active_transfer_browser_connection_id();
        let download_path = self
            .resolved_transfer_download_dir()
            .map(|path| path.display().to_string());
        let visible_entries = self.visible_transfer_browser_entries();
        let queue = self.build_transfer_queue_presentation(active_session_id.as_deref());

        let search_field = self.existing_text_input("transfer.browser.search");
        let path_field = self.existing_text_input("transfer.browser.path");
        let rename = self.transfer.rename_dialog().cloned();
        let rename_field = rename.as_ref().and_then(|state| {
            self.existing_text_input(format!("transfer.rename.{}", state.old_path))
        });
        let show_hidden_files = self.settings.summary().ui_file_explorer_show_hidden_files;
        let browser = self.transfer.browser_view();
        let browser = TransferBrowserPresentation {
            local_backend: self.session.active_file_browser_backend()
                == Some(nyaterm_transport::FileBrowserBackendKind::Local),
            path: browser.path.clone(),
            home_dir: browser.home_dir.clone(),
            path_editing: browser.path_editing,
            all_entries: browser.entries.clone(),
            visible_entries,
            loading: browser.loading,
            error: browser.error.clone(),
            search: browser.search.clone(),
            search_expanded: browser.search_expanded,
            list_scroll: browser.list_scroll.clone(),
            horizontal_scroll: browser.horizontal_scroll.clone(),
            visited_history: browser.visited_history.clone(),
            favorites: browser.favorites.clone(),
            sort_column: browser.sort_column,
            sort_direction: browser.sort_direction,
            column_widths: browser.column_widths,
            column_resize: *browser.column_resize,
            selected_remote_path: browser.selected_remote_path.clone(),
            selected_remote_paths: browser.selected_remote_paths.clone(),
            external_drop_hover: browser.external_drop_hover,
            focus: browser.focus.clone(),
            rename,
            auto_sync_cwd_enabled,
            connection_id,
            search_field,
            path_field,
            rename_field,
            show_hidden_files,
        };

        TransferSnapshot {
            chrome,
            availability,
            panel_height,
            height_is_resizing,
            resize_handle_highlighted,
            has_session: active_session_id.is_some(),
            browser,
            cwd_sync_demand,
            queue: TransferQueuePresentation {
                download_path: download_path
                    .map(|path| nyaterm_core::truncate_preview(&path, 64))
                    .unwrap_or_else(|| {
                        format!("{}: -", rust_i18n::t!("fileTransfer.downloadPath"))
                    }),
                ..queue
            },
        }
    }

    /// The queue rows, filtered to the active session and ordered for display.
    ///
    /// Both used to happen in render, each with a full deep copy of every job --
    /// including the directory listing a navigation job carries. Rows keep only the
    /// eight fields drawn, so this is cheap enough to redo per progress batch.
    fn build_transfer_queue_presentation(
        &self,
        active_session_id: Option<&str>,
    ) -> TransferQueuePresentation {
        let visible = self
            .transfer
            .transfer_jobs()
            .iter()
            .filter(|job| job.is_visible_for_session(active_session_id))
            .collect::<Vec<_>>();

        let has_running = visible
            .iter()
            .any(|job| job.status == TransferJobStatus::Running && job.control.is_some());
        let has_paused = visible
            .iter()
            .any(|job| job.status == TransferJobStatus::Paused && job.control.is_some());
        let has_active = visible.iter().any(|job| {
            job.control.is_some()
                && matches!(
                    job.status,
                    TransferJobStatus::Running | TransferJobStatus::Paused
                )
        });
        let has_completed = visible
            .iter()
            .any(|job| job.status == TransferJobStatus::Completed);
        let has_stopped = visible.iter().any(|job| {
            !matches!(
                job.status,
                TransferJobStatus::Running
                    | TransferJobStatus::Paused
                    | TransferJobStatus::Cancelling
            )
        });

        let mut ordered = visible
            .iter()
            .enumerate()
            .map(|(index, job)| (index, job.row_snapshot()))
            .collect::<Vec<_>>();
        ordered.sort_by(|(left_index, left), (right_index, right)| {
            transfer_job_display_rank(left.status)
                .cmp(&transfer_job_display_rank(right.status))
                .then_with(|| right_index.cmp(left_index))
        });
        let rows: Arc<[TransferJobRowSnapshot]> = ordered.into_iter().map(|(_, job)| job).collect();

        TransferQueuePresentation {
            rows,
            has_running,
            has_paused,
            has_active,
            has_completed,
            has_stopped,
            selected_job_id: self.transfer.selected_transfer_job_id().map(str::to_string),
            download_path: String::new(),
            focus: self.transfer.queue_focus().clone(),
        }
    }
}

fn transfer_job_display_rank(status: TransferJobStatus) -> u8 {
    match status {
        TransferJobStatus::Running | TransferJobStatus::Cancelling => 0,
        TransferJobStatus::Paused => 1,
        TransferJobStatus::Cancelled | TransferJobStatus::Completed | TransferJobStatus::Failed => {
            2
        }
    }
}
