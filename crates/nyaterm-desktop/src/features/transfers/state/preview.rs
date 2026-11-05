//! Read-only remote-preview workspace, tab lifecycle and window tracking.
//!
//! Mirrors the editor state module, minus every write concern: a preview tab is
//! never dirty, never saved, and never in conflict, so closing one needs no
//! confirmation. The one piece it keeps from the editor is generation-guarded
//! completion: a load result is only applied when its `generation` still matches
//! the tab, so a slow fetch for a tab the user refreshed or closed is dropped.

use super::{TransferFeatureState, TransferPreviewFeatureState};
use crate::models::{
    PreviewContent, PreviewViewport, TransferPreviewState, TransferPreviewWorkspaceState,
};
use gpui::FocusHandle;
use nyaterm_files::remote_file::RemoteTextGeneration;
use nyaterm_ui::{ChildWindowSlot, NyaWindowHandle};
use std::sync::Arc;

/// A request to rasterize one PDF page on a background thread, carrying
/// everything the job needs so the caller does not re-read the tab.
pub(in crate::features) struct PdfPageRequest {
    pub(in crate::features) tab_id: String,
    pub(in crate::features) session_id: Option<String>,
    pub(in crate::features) remote_path: String,
    pub(in crate::features) generation: RemoteTextGeneration,
    pub(in crate::features) page_index: usize,
    pub(in crate::features) bytes: Arc<Vec<u8>>,
}

/// What closing a preview tab or workspace did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::features) enum TransferPreviewCloseOutcome {
    Missing,
    Closed,
}

impl TransferFeatureState {
    pub(in crate::features) fn preview_focus(&self) -> &FocusHandle {
        &self.preview.focus
    }

    pub(in crate::features) fn preview_workspace(&self) -> Option<&TransferPreviewWorkspaceState> {
        self.preview.workspace.as_ref()
    }

    pub(in crate::features) fn preview_has_workspace(&self) -> bool {
        self.preview.workspace.is_some()
    }

    pub(in crate::features) fn active_preview_tab(&self) -> Option<&TransferPreviewState> {
        self.preview
            .workspace
            .as_ref()
            .and_then(TransferPreviewWorkspaceState::active_tab)
    }

    pub(in crate::features) fn preview_tab_snapshot(
        &self,
        tab_id: &str,
    ) -> Option<TransferPreviewState> {
        self.preview
            .workspace
            .as_ref()?
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .cloned()
    }

    /// Open `tab`, or focus it and return `true` if a tab with the same id is
    /// already open (so the caller can skip re-loading it).
    pub(in crate::features) fn open_preview_tab(&mut self, tab: TransferPreviewState) -> bool {
        let tab_id = tab.id.clone();
        if let Some(workspace) = self.preview.workspace.as_mut() {
            let already_open = workspace.tabs.iter().any(|current| current.id == tab_id);
            if !already_open {
                let insert_after = workspace
                    .tabs
                    .iter()
                    .position(|current| {
                        current.id == workspace.active_tab_id
                            && current.session_id == tab.session_id
                    })
                    .or_else(|| {
                        workspace
                            .tabs
                            .iter()
                            .rposition(|current| current.session_id == tab.session_id)
                    });
                let index = insert_after.map_or(workspace.tabs.len(), |index| index + 1);
                workspace.tabs.insert(index, tab);
            }
            workspace.active_tab_id = tab_id;
            already_open
        } else {
            self.preview.workspace = Some(TransferPreviewWorkspaceState::new(tab));
            false
        }
    }

    pub(in crate::features) fn activate_preview_tab(&mut self, tab_id: &str) -> bool {
        let Some(workspace) = self.preview.workspace.as_mut() else {
            return false;
        };
        if !workspace.tabs.iter().any(|tab| tab.id == tab_id) {
            return false;
        }
        workspace.active_tab_id = tab_id.to_string();
        true
    }

    pub(in crate::features) fn close_preview_tab(
        &mut self,
        tab_id: &str,
    ) -> TransferPreviewCloseOutcome {
        let Some(workspace) = self.preview.workspace.as_mut() else {
            return TransferPreviewCloseOutcome::Missing;
        };
        if !workspace.remove_tab(tab_id) {
            return TransferPreviewCloseOutcome::Missing;
        }
        if workspace.tabs.is_empty() {
            self.preview.workspace = None;
            self.preview.window.cancel_open();
        }
        TransferPreviewCloseOutcome::Closed
    }

    pub(in crate::features) fn close_preview(&mut self) -> TransferPreviewCloseOutcome {
        if self.preview.workspace.is_none() {
            return TransferPreviewCloseOutcome::Missing;
        }
        self.preview.workspace = None;
        self.preview.window.cancel_open();
        TransferPreviewCloseOutcome::Closed
    }

    /// Re-arm a tab for reload: bump the generation so any in-flight load for the
    /// old generation is discarded, and reset it to `Loading`. Returns the new
    /// generation the caller should start the load job with.
    pub(in crate::features) fn begin_preview_tab_reload(
        &mut self,
        tab_id: &str,
    ) -> Option<RemoteTextGeneration> {
        let tab = self
            .preview
            .workspace
            .as_mut()?
            .tabs
            .iter_mut()
            .find(|tab| tab.id == tab_id)?;
        let generation = RemoteTextGeneration::next();
        tab.generation = generation;
        tab.content = PreviewContent::Loading;
        tab.viewport = PreviewViewport::default();
        Some(generation)
    }

    /// Apply a finished load to `tab_id`, but only if `generation` still matches:
    /// a stale result for a superseded generation is dropped.
    pub(in crate::features) fn complete_preview_tab(
        &mut self,
        tab_id: &str,
        generation: RemoteTextGeneration,
        content: PreviewContent,
    ) -> bool {
        let Some(tab) = self
            .preview
            .workspace
            .as_mut()
            .and_then(|workspace| workspace.tabs.iter_mut().find(|tab| tab.id == tab_id))
        else {
            return false;
        };
        if tab.generation != generation {
            return false;
        }
        tab.content = content;
        true
    }

    pub(in crate::features) fn fail_preview_tab(
        &mut self,
        tab_id: &str,
        generation: RemoteTextGeneration,
        error: String,
    ) -> bool {
        self.complete_preview_tab(tab_id, generation, PreviewContent::Error(error))
    }

    pub(in crate::features) fn preview_zoom_active_tab(&mut self, factor: f32) -> bool {
        let Some(tab) = self
            .preview
            .workspace
            .as_mut()
            .and_then(TransferPreviewWorkspaceState::active_tab_mut)
        else {
            return false;
        };
        tab.viewport.zoom_by(factor);
        true
    }

    /// Zoom the active tab by `factor`, then clamp into `[min, max]`. Used for
    /// PDF, whose zoom range (50%–300%) is narrower than the image range.
    pub(in crate::features) fn preview_zoom_active_tab_bounded(
        &mut self,
        factor: f32,
        min: f32,
        max: f32,
    ) -> bool {
        let Some(tab) = self
            .preview
            .workspace
            .as_mut()
            .and_then(TransferPreviewWorkspaceState::active_tab_mut)
        else {
            return false;
        };
        tab.viewport.zoom_by(factor);
        tab.viewport.set_zoom(tab.viewport.zoom.clamp(min, max));
        true
    }

    pub(in crate::features) fn preview_reset_active_viewport(&mut self) -> bool {
        let Some(tab) = self
            .preview
            .workspace
            .as_mut()
            .and_then(TransferPreviewWorkspaceState::active_tab_mut)
        else {
            return false;
        };
        tab.viewport.reset();
        true
    }

    pub(in crate::features) fn preview_rotate_active_tab(&mut self, clockwise: bool) -> bool {
        let Some(tab) = self
            .preview
            .workspace
            .as_mut()
            .and_then(TransferPreviewWorkspaceState::active_tab_mut)
        else {
            return false;
        };
        if clockwise {
            tab.viewport.rotate_clockwise();
        } else {
            tab.viewport.rotate_counter_clockwise();
        }
        let turns = tab.viewport.rotation_quarter_turns;
        // Rebuild the paint surface from the unrotated source pixels so repeated
        // rotations do not accumulate resampling. This runs on a rotate click,
        // never in a render path. For a PDF only the currently-shown page is
        // rebuilt; other cached pages are rebuilt when they next become active.
        match &mut tab.content {
            PreviewContent::Image(image) => {
                if let Some((rendered, width, height)) =
                    crate::features::transfers::preview::decode::rotated_render_image(
                        &image.pixels,
                        image.src_width,
                        image.src_height,
                        turns,
                    )
                {
                    image.image = rendered;
                    image.width = width;
                    image.height = height;
                }
            }
            PreviewContent::Pdf(document) => {
                // Dropping the cache forces every page (including ones the user
                // pages back to) to re-rasterize at the new rotation, so
                // orientation stays consistent without holding stale surfaces.
                document.cache.clear();
                document.cache_order.clear();
                document.pending.clear();
            }
            _ => {}
        }
        true
    }

    /// Rotate a single decoded PDF page's surface by `turns` before caching it,
    /// so a page rasterized after a rotation is shown at the same orientation.
    fn rotate_pdf_page_surface(page: &mut crate::models::PreviewPdfPage, turns: u8) {
        if turns.is_multiple_of(4) {
            return;
        }
        if let Some((rendered, width, height)) =
            crate::features::transfers::preview::decode::rotated_render_image(
                &page.pixels,
                page.src_width,
                page.src_height,
                turns,
            )
        {
            page.image = rendered;
            page.width = width;
            page.height = height;
        }
    }

    /// Toggle "first row is header" on the active delimited preview.
    pub(in crate::features) fn preview_toggle_delimited_header(&mut self) -> bool {
        let Some(tab) = self
            .preview
            .workspace
            .as_mut()
            .and_then(TransferPreviewWorkspaceState::active_tab_mut)
        else {
            return false;
        };
        if let PreviewContent::Delimited(data) = &mut tab.content {
            data.toggle_header();
            true
        } else {
            false
        }
    }

    /// Cycle the sort of a delimited column (asc → desc → cleared).
    pub(in crate::features) fn preview_cycle_delimited_sort(&mut self, column: usize) -> bool {
        let Some(tab) = self
            .preview
            .workspace
            .as_mut()
            .and_then(TransferPreviewWorkspaceState::active_tab_mut)
        else {
            return false;
        };
        if let PreviewContent::Delimited(data) = &mut tab.content {
            data.cycle_sort(column);
            true
        } else {
            false
        }
    }

    pub(in crate::features) fn preview_next_pdf_page(&mut self) -> Option<PdfPageRequest> {
        let tab = self
            .preview
            .workspace
            .as_mut()
            .and_then(TransferPreviewWorkspaceState::active_tab_mut)?;
        let PreviewContent::Pdf(document) = &mut tab.content else {
            return None;
        };
        document.next_page();
        self.pdf_page_request_for_active_tab()
    }

    pub(in crate::features) fn preview_previous_pdf_page(&mut self) -> Option<PdfPageRequest> {
        let tab = self
            .preview
            .workspace
            .as_mut()
            .and_then(TransferPreviewWorkspaceState::active_tab_mut)?;
        let PreviewContent::Pdf(document) = &mut tab.content else {
            return None;
        };
        document.previous_page();
        self.pdf_page_request_for_active_tab()
    }

    /// If the active PDF's current page needs rendering and has no request in
    /// flight, mark it pending and return the request describing the job.
    pub(in crate::features) fn pdf_page_request_for_active_tab(
        &mut self,
    ) -> Option<PdfPageRequest> {
        let tab = self
            .preview
            .workspace
            .as_mut()
            .and_then(TransferPreviewWorkspaceState::active_tab_mut)?;
        let generation = tab.generation;
        let tab_id = tab.id.clone();
        let session_id = tab.session_id.clone();
        let remote_path = tab.remote_path.clone();
        let PreviewContent::Pdf(document) = &mut tab.content else {
            return None;
        };
        if !document.active_page_needs_render() {
            return None;
        }
        let page_index = document.current_page;
        document.mark_pending(page_index);
        Some(PdfPageRequest {
            tab_id,
            session_id,
            remote_path,
            generation,
            page_index,
            bytes: document.bytes.clone(),
        })
    }

    /// Apply a freshly rasterized PDF page under the tab's generation guard.
    /// A page for a superseded generation (refresh/rotate) is discarded.
    pub(in crate::features) fn complete_pdf_page(
        &mut self,
        tab_id: &str,
        generation: RemoteTextGeneration,
        page_index: usize,
        page: crate::models::PreviewPdfPage,
    ) -> bool {
        let Some(tab) = self
            .preview
            .workspace
            .as_mut()
            .and_then(|workspace| workspace.tabs.iter_mut().find(|tab| tab.id == tab_id))
        else {
            return false;
        };
        if tab.generation != generation {
            return false;
        }
        let turns = tab.viewport.rotation_quarter_turns;
        let PreviewContent::Pdf(document) = &mut tab.content else {
            return false;
        };
        let mut page = page;
        Self::rotate_pdf_page_surface(&mut page, turns);
        document.insert_page(page_index, page);
        true
    }

    /// Clear the pending marker for a page whose rasterization failed, so the
    /// view can retry on the next visit rather than being stuck.
    pub(in crate::features) fn fail_pdf_page(
        &mut self,
        tab_id: &str,
        generation: RemoteTextGeneration,
        page_index: usize,
    ) -> bool {
        let Some(tab) = self
            .preview
            .workspace
            .as_mut()
            .and_then(|workspace| workspace.tabs.iter_mut().find(|tab| tab.id == tab_id))
        else {
            return false;
        };
        if tab.generation != generation {
            return false;
        }
        if let PreviewContent::Pdf(document) = &mut tab.content {
            document.pending.retain(|pending| *pending != page_index);
            true
        } else {
            false
        }
    }

    pub(in crate::features) fn remove_preview_tabs_for_session(
        &mut self,
        session_id: &str,
    ) -> usize {
        let Some(workspace) = self.preview.workspace.as_mut() else {
            return 0;
        };
        let before = workspace.tabs.len();
        let active_removed = workspace
            .active_tab()
            .is_some_and(|tab| tab.session_id.as_deref() == Some(session_id));
        workspace
            .tabs
            .retain(|tab| tab.session_id.as_deref() != Some(session_id));
        let removed = before.saturating_sub(workspace.tabs.len());
        if active_removed {
            workspace.active_tab_id = workspace
                .tabs
                .first()
                .map(|tab| tab.id.clone())
                .unwrap_or_default();
        }
        if workspace.tabs.is_empty() {
            self.preview.workspace = None;
            self.preview.window.cancel_open();
        }
        removed
    }

    pub(in crate::features) fn preview_window(&self) -> Option<NyaWindowHandle> {
        self.preview.window.handle()
    }

    pub(in crate::features) fn preview_window_is_open(&self) -> bool {
        self.preview.window.is_open()
    }

    pub(in crate::features) fn preview_window_open_is_pending(&self) -> bool {
        self.preview.window.is_pending()
    }

    pub(in crate::features) fn preview_window_slot(&mut self) -> &mut ChildWindowSlot {
        &mut self.preview.window
    }

    pub(in crate::features) fn begin_preview_window_open(&mut self) -> bool {
        if self.preview.workspace.is_none() {
            return false;
        }
        self.preview.window.begin_open()
    }

    pub(in crate::features) fn finish_preview_window_open(&mut self, handle: NyaWindowHandle) {
        self.preview.window.finish_open(handle);
    }

    pub(in crate::features) fn finish_preview_window_activation(
        &mut self,
        handle: NyaWindowHandle,
        activated: bool,
    ) -> bool {
        self.preview.window.cancel_open();
        if activated {
            return false;
        }
        self.preview.window.clear_if(handle)
    }

    pub(in crate::features) fn clear_preview_window_tracking(&mut self) -> bool {
        let changed = self.preview.window.is_open_or_pending();
        self.preview.window.clear();
        changed
    }
}

impl TransferPreviewFeatureState {
    pub(super) fn new(focus: FocusHandle) -> Self {
        Self {
            workspace: None,
            focus,
            window: ChildWindowSlot::default(),
        }
    }
}
