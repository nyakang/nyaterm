use gpui::Context;
use nyaterm_core::{MoveTabDockEdge, MoveTabPlacement, MoveTabTreeRequest, WorkspaceId};
use nyaterm_transport::SessionKind;

use super::NyaTermApp;
use super::session::SessionCatalogTransferBundle;
use super::sync_input::SyncInputTransferBundle;
use super::terminal::TerminalSessionTransferBundle;
use super::transfers::TransferSessionTransferBundle;
use crate::models::{TabDockEdge, TabDockZone, TerminalWindowNode, WorkspacePaneNode};

#[cfg(test)]
mod tests;

pub(crate) struct WorkspaceTabTransferBundle {
    root_tab_id: String,
    session_ids: Vec<String>,
    pane_root: Option<WorkspacePaneNode>,
    catalog: SessionCatalogTransferBundle,
    terminal: TerminalSessionTransferBundle,
    transfer: TransferSessionTransferBundle,
    sync_input: SyncInputTransferBundle,
    source_index: usize,
    source_active_id: Option<String>,
    source_terminal_tree: Option<TerminalWindowNode>,
}

impl NyaTermApp {
    pub(crate) fn has_live_sessions(&self) -> bool {
        !self.session.ordered_sessions().is_empty()
    }
    pub(crate) fn live_session_count(&self) -> usize {
        self.session.ordered_sessions().len()
    }

    pub(crate) fn can_close_failed_tab_workspace(&self) -> bool {
        !self.has_live_sessions()
            && !self.session.start_has_pending()
            && self.transfer.editor_documents().is_empty()
    }

    pub(crate) fn report_tab_move_failure(&mut self, error: String, cx: &mut Context<Self>) {
        self.shell
            .set_status(format!("Could not move tab: {error}"));
        cx.notify();
    }
    pub(in crate::features) fn request_tab_tree_move(
        &mut self,
        payload: &super::shell::WorkspaceTabDragPayload,
        placement: MoveTabPlacement,
        cx: &mut Context<Self>,
    ) {
        if payload.tab_id.is_start() {
            self.clear_session_tab_drag(cx);
            return;
        }
        self.move_tab_tree_to_workspace(
            payload.source_workspace_id,
            payload.root_tab_id.clone(),
            payload.source_revision,
            self.workspace_id,
            placement,
            cx,
        );
    }

    pub(in crate::features) fn move_tab_tree_to_workspace(
        &mut self,
        source_workspace_id: WorkspaceId,
        root_tab_id: String,
        source_revision: u64,
        target_workspace_id: WorkspaceId,
        placement: MoveTabPlacement,
        cx: &mut Context<Self>,
    ) {
        if !crate::models::WorkspaceTabId::key_is_session(&root_tab_id)
            || (source_workspace_id == self.workspace_id
                && self
                    .tab_tree_session_ids(&root_tab_id)
                    .iter()
                    .any(|id| self.workspace_references_session(id, cx)))
        {
            self.shell.set_status(
                "Close the referencing file editors before moving this session to another window",
            );
            cx.notify();
            return;
        }
        let Some(controller) = self.desktop_controller.clone() else {
            return;
        };
        let source_app = cx.entity().downgrade();
        cx.defer(move |cx| {
            let result = controller.update(cx, |controller, cx| {
                controller.move_tab_tree(
                    MoveTabTreeRequest {
                        source_workspace_id,
                        target_workspace_id,
                        root_tab_id,
                        source_revision,
                        placement,
                    },
                    cx,
                )
            });
            let error = match result {
                Ok(Ok(())) => return,
                Ok(Err(error)) => error,
                Err(error) => error.to_string(),
            };
            let _ = source_app.update(cx, |app, cx| {
                app.shell.set_status(format!("Could not move tab: {error}"));
                cx.notify();
            });
        });
    }

    pub(in crate::features) fn move_tab_tree_to_new_window(
        &mut self,
        root_tab_id: String,
        cx: &mut Context<Self>,
    ) {
        let Some(controller) = self.desktop_controller.clone() else {
            return;
        };
        let source_workspace_id = self.workspace_id;
        let source_revision = self.workspace_revision;
        let source_app = cx.entity().downgrade();
        cx.defer(move |cx| {
            let result = controller.update(cx, |controller, cx| {
                controller.open_workspace_for_tab(
                    source_workspace_id,
                    root_tab_id,
                    source_revision,
                    cx,
                )
            });
            let error = match result {
                Ok(Ok(_)) => return,
                Ok(Err(error)) => error.to_string(),
                Err(error) => error.to_string(),
            };
            let _ = source_app.update(cx, |app, cx| app.report_tab_move_failure(error, cx));
        });
    }

    pub(crate) fn can_accept_tab_tree(
        &self,
        session_ids: &[String],
        placement: &MoveTabPlacement,
    ) -> Result<(), String> {
        if session_ids.iter().any(|id| self.session.has_session(id)) {
            return Err("the target workspace already contains this session".into());
        }
        match placement {
            MoveTabPlacement::Append => {}
            MoveTabPlacement::BeforeTab(id) | MoveTabPlacement::AfterTab(id) => {
                if !self.workspace_tab_ids().contains(id) {
                    return Err("the target tab no longer exists".into());
                }
            }
            MoveTabPlacement::TerminalLeaf { leaf_id, .. } => {
                if !self.shell.terminal_window_tree_is_some()
                    || !self.shell.terminal_window_has_leaf(leaf_id)
                {
                    return Err("the target split pane no longer exists".into());
                }
            }
        }
        Ok(())
    }

    pub(crate) fn workspace_revision(&self) -> u64 {
        self.workspace_revision
    }

    pub(crate) fn tab_tree_transfer_session_ids(&self, root_tab_id: &str) -> Option<Vec<String>> {
        let root = self.tab_root_for_session(root_tab_id);
        (root == root_tab_id && self.session.has_session(root_tab_id))
            .then(|| self.tab_tree_session_ids(root_tab_id))
    }

    pub(crate) fn can_transfer_tab_tree(
        &self,
        root_tab_id: &str,
        source_revision: u64,
        cx: &gpui::App,
    ) -> Result<Vec<String>, String> {
        if self.workspace_revision != source_revision {
            return Err("the source workspace changed during the drag".to_string());
        }
        let session_ids = self
            .tab_tree_transfer_session_ids(root_tab_id)
            .ok_or_else(|| "the source tab no longer exists".to_string())?;
        if session_ids
            .iter()
            .any(|id| self.workspace_references_session(id, cx))
        {
            return Err(
                "Close the referencing file editors before moving this session to another window"
                    .to_string(),
            );
        }
        self.session
            .can_transfer_sessions(&session_ids)
            .map_err(str::to_string)?;
        if session_ids.iter().any(|session_id| {
            self.session
                .session_info(session_id)
                .is_some_and(|session| matches!(session.kind, SessionKind::Rdp | SessionKind::Vnc))
        }) {
            return Err("RDP and VNC tabs cannot be moved while connected yet".to_string());
        }
        if session_ids
            .iter()
            .any(|session_id| self.recording.is_recording(session_id))
        {
            return Err("stop the active recording before moving this tab".to_string());
        }
        if self.transfer.session_has_active_transfer(&session_ids) {
            return Err(
                "wait for active file transfers to finish before moving this tab".to_string(),
            );
        }
        Ok(session_ids)
    }

    pub(crate) fn detach_tab_tree_for_transfer(
        &mut self,
        root_tab_id: &str,
        source_revision: u64,
        cx: &mut Context<Self>,
    ) -> Result<WorkspaceTabTransferBundle, String> {
        let session_ids = self.can_transfer_tab_tree(root_tab_id, source_revision, cx)?;
        let source_index = self.session.session_index(root_tab_id).unwrap_or(0);
        let source_active_id = self.session.active_id_owned();
        let source_terminal_tree = self.shell.terminal_window_tree();
        if let Some(active) = self.session.active_id_owned()
            && session_ids.contains(&active)
        {
            self.cache_transfer_browser_session(&active);
        }
        let bridge_events = self.session.pause_sessions_for_transfer(&session_ids);
        let frames = match self.terminal.prepare_sessions_for_transfer(&session_ids) {
            Ok(frames) => frames,
            Err(error) => {
                self.session
                    .resume_sessions_after_failed_transfer(&session_ids, bridge_events);
                return Err(error.to_string());
            }
        };
        let terminal = self.terminal.detach_sessions_for_transfer(frames);
        let transfer = self.transfer.detach_sessions_for_transfer(&session_ids);
        let sync_input = self.sync_input.detach_sessions_for_transfer(&session_ids);
        let catalog = self
            .session
            .detach_sessions_for_transfer(&session_ids, bridge_events)
            .ok_or_else(|| "the source tab changed during transfer".to_string())?;
        for id in &session_ids {
            self.file_manager.remove_sftp_session(id);
        }
        let pane_root = self.shell.take_workspace_pane_root(root_tab_id);
        self.rebuild_session_tab_owners();
        self.reconcile_terminal_windows();
        if source_active_id
            .as_ref()
            .is_some_and(|id| session_ids.contains(id))
        {
            self.reset_transfer_browser_for_active_session();
        }
        debug_assert!(session_ids.iter().all(|id| {
            !self.terminal.retains_transfer_session(id)
                && !self.transfer.retains_transfer_session(id)
                && !self.sync_input.retains_transfer_session(id)
        }));
        self.sync_workspace_split_from_active_tab();
        if self.session.active_id().is_none()
            && let Some(next) = self.session.next_live_session()
        {
            self.activate_session_id_with_surface_sync(&next, cx);
        }
        self.workspace_revision = self.workspace_revision.saturating_add(1);
        self.persist_open_tabs();
        self.persist_terminal_window_layout();
        self.persist_workspace_pane_layout();
        cx.notify();
        Ok(WorkspaceTabTransferBundle {
            root_tab_id: root_tab_id.to_string(),
            session_ids,
            pane_root,
            catalog,
            terminal,
            transfer,
            sync_input,
            source_index,
            source_active_id,
            source_terminal_tree,
        })
    }

    pub(crate) fn attach_tab_tree_from_transfer(
        &mut self,
        bundle: WorkspaceTabTransferBundle,
        placement: &MoveTabPlacement,
        cx: &mut Context<Self>,
    ) -> Result<(), Box<(String, WorkspaceTabTransferBundle)>> {
        if let Err(error) = self.can_accept_tab_tree(&bundle.session_ids, placement) {
            return Err(Box::new((error, bundle)));
        }
        let insert_index = match placement {
            MoveTabPlacement::BeforeTab(tab_id) => self.session.session_index(tab_id),
            MoveTabPlacement::AfterTab(tab_id) => {
                self.session.session_index(tab_id).map(|index| index + 1)
            }
            MoveTabPlacement::Append | MoveTabPlacement::TerminalLeaf { .. } => None,
        };
        let WorkspaceTabTransferBundle {
            root_tab_id,
            catalog,
            terminal,
            transfer,
            sync_input,
            pane_root,
            source_index,
            source_active_id,
            source_terminal_tree,
            session_ids,
        } = bundle;
        if let Err(terminal) = self.terminal.attach_sessions_from_transfer(terminal) {
            return Err(Box::new((
                "the target terminal pipeline is unavailable".to_string(),
                WorkspaceTabTransferBundle {
                    root_tab_id,
                    session_ids,
                    pane_root,
                    catalog,
                    terminal,
                    transfer,
                    sync_input,
                    source_index,
                    source_active_id,
                    source_terminal_tree,
                },
            )));
        }
        self.session
            .attach_sessions_from_transfer(catalog, insert_index);
        for id in &session_ids {
            self.ensure_sftp_session_view(id, cx);
        }
        self.rebind_zmodem_recording();
        self.transfer.attach_sessions_from_transfer(transfer);
        self.sync_input.attach_sessions_from_transfer(sync_input);
        if let Some(pane_root) = pane_root {
            self.shell
                .insert_workspace_pane_root(root_tab_id.clone(), pane_root);
        }
        self.rebuild_session_tab_owners();
        self.reconcile_terminal_windows();
        if let MoveTabPlacement::BeforeTab(tab_id) = placement {
            let _ = self
                .shell
                .place_tab_before_in_terminal_windows(&root_tab_id, tab_id);
        }
        if let MoveTabPlacement::AfterTab(tab_id) = placement {
            self.reorder_session_relative(root_tab_id.clone(), tab_id.clone(), true, cx);
        }
        if let MoveTabPlacement::TerminalLeaf { leaf_id, edge } = placement {
            let zone = edge.map_or(TabDockZone::Center, |edge| {
                TabDockZone::Edge(match edge {
                    MoveTabDockEdge::Left => TabDockEdge::Left,
                    MoveTabDockEdge::Right => TabDockEdge::Right,
                    MoveTabDockEdge::Top => TabDockEdge::Top,
                    MoveTabDockEdge::Bottom => TabDockEdge::Bottom,
                })
            });
            self.ensure_terminal_windows_root();
            let _ = self
                .shell
                .dock_tab_on_terminal_window_leaf(&root_tab_id, leaf_id, zone);
        }
        self.activate_session_id_with_surface_sync(&root_tab_id, cx);
        self.workspace_revision = self.workspace_revision.saturating_add(1);
        self.persist_open_tabs();
        self.persist_terminal_window_layout();
        self.persist_workspace_pane_layout();
        self.shell
            .set_status("tab moved from another window".to_string());
        cx.notify();
        Ok(())
    }

    pub(crate) fn restore_tab_tree_after_failed_transfer(
        &mut self,
        bundle: WorkspaceTabTransferBundle,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        self.terminal
            .attach_sessions_from_transfer(bundle.terminal)
            .map_err(|_| "the source terminal pipeline is unavailable".to_string())?;
        self.session
            .attach_sessions_from_transfer(bundle.catalog, Some(bundle.source_index));
        for id in &bundle.session_ids {
            self.ensure_sftp_session_view(id, cx);
        }
        self.rebind_zmodem_recording();
        self.transfer.attach_sessions_from_transfer(bundle.transfer);
        self.sync_input
            .restore_sessions_after_failed_transfer(bundle.sync_input);
        if let Some(pane_root) = bundle.pane_root {
            self.shell
                .insert_workspace_pane_root(bundle.root_tab_id, pane_root);
        }
        self.shell
            .restore_terminal_window_tree(bundle.source_terminal_tree);
        self.rebuild_session_tab_owners();
        self.reconcile_terminal_windows();
        if let Some(active_id) = bundle.source_active_id {
            self.activate_session_id_with_surface_sync(&active_id, cx);
        }
        self.workspace_revision = self.workspace_revision.saturating_add(1);
        self.persist_open_tabs();
        self.persist_terminal_window_layout();
        self.persist_workspace_pane_layout();
        cx.notify();
        Ok(())
    }
}
