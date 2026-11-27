use super::DesktopController;
use super::WorkspaceTarget;
use crate::app_shell::tab_drag::TabDragCoordinator;
use crate::app_shell::tab_drag::TabDragSource;
use crate::app_shell::tab_drag::detach_placement;
use crate::app_shell::window_state::MainWindowPlacement;
use gpui::Context;
use nyaterm_core::MoveTabTreeRequest;
use nyaterm_core::OpenWorkspaceRequest;
use nyaterm_core::WorkspaceId;

impl DesktopController {
    pub fn move_tab_tree(
        &mut self,
        request: MoveTabTreeRequest,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if self.process_quitting
            || self
                .closing_workspaces
                .contains(&request.source_workspace_id)
            || self
                .closing_workspaces
                .contains(&request.target_workspace_id)
        {
            return Err("a workspace is closing".into());
        }
        if request.source_workspace_id == request.target_workspace_id {
            return Err("source and target workspaces must differ".into());
        }
        let source = self
            .windows
            .get(&request.source_workspace_id)
            .ok_or("the source window closed during the drag")?;
        let target = self
            .windows
            .get(&request.target_workspace_id)
            .ok_or("the target window closed during the drag")?;
        let source_app = source
            .shell
            .update(cx, |shell, _| shell.app.clone())
            .map_err(|_| "the source window closed during the drag")?
            .ok_or("the source window is still loading")?;
        let target_app = target
            .shell
            .update(cx, |shell, _| shell.app.clone())
            .map_err(|_| "the target window closed during the drag")?
            .ok_or("the target window is still loading")?;
        let session_ids = source_app.read(cx).can_transfer_tab_tree(
            &request.root_tab_id,
            request.source_revision,
            cx,
        )?;
        target_app
            .read(cx)
            .can_accept_tab_tree(&session_ids, &request.placement)?;
        let bundle = source_app.update(cx, |app, cx| {
            app.detach_tab_tree_for_transfer(&request.root_tab_id, request.source_revision, cx)
        })?;
        if let Err(failure) = target_app.update(cx, |app, cx| {
            app.attach_tab_tree_from_transfer(bundle, &request.placement, cx)
        }) {
            let (error, bundle) = *failure;
            source_app
                .update(cx, |app, cx| {
                    app.restore_tab_tree_after_failed_transfer(bundle, cx)
                })
                .map_err(|restore_error| format!("{error}; rollback failed: {restore_error}"))?;
            return Err(error);
        }
        let target = self
            .windows
            .get(&request.target_workspace_id)
            .ok_or("the target window closed during the move")?;
        let _ = target.handle.update(cx, |_, window, cx| {
            window.activate_window();
            cx.activate(true);
            target_app.update(cx, |app, cx| {
                app.focus_terminal_session(&request.root_tab_id, window, cx);
            });
        });
        self.mark_active(request.target_workspace_id);
        Ok(())
    }

    pub fn open_workspace_for_tab(
        &mut self,
        source_workspace_id: WorkspaceId,
        root_tab_id: String,
        source_revision: u64,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<WorkspaceId> {
        self.open_workspace_for_tab_at(source_workspace_id, root_tab_id, source_revision, None, cx)
    }

    pub(super) fn open_workspace_for_tab_at(
        &mut self,
        source_workspace_id: WorkspaceId,
        root_tab_id: String,
        source_revision: u64,
        placement: Option<MainWindowPlacement>,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<WorkspaceId> {
        anyhow::ensure!(
            !self.closing_workspaces.contains(&source_workspace_id),
            "the source workspace is closing"
        );
        let source = self
            .windows
            .get(&source_workspace_id)
            .ok_or_else(|| anyhow::anyhow!("the source window closed during the drag"))?;
        let app = source
            .shell
            .update(cx, |shell, _| shell.app.clone())?
            .ok_or_else(|| anyhow::anyhow!("the source window is still loading"))?;
        app.read(cx)
            .can_transfer_tab_tree(&root_tab_id, source_revision, cx)
            .map_err(anyhow::Error::msg)?;
        let target_workspace_id = self.open_workspace_at(
            OpenWorkspaceRequest {
                layout_source_workspace_id: Some(source_workspace_id),
                ..Default::default()
            },
            placement,
            cx,
        )?;
        self.pending_tab_moves.insert(
            target_workspace_id,
            MoveTabTreeRequest {
                source_workspace_id,
                target_workspace_id,
                root_tab_id,
                source_revision,
                placement: Default::default(),
            },
        );
        Ok(target_workspace_id)
    }

    pub fn workspace_ready(&mut self, workspace_id: WorkspaceId, cx: &mut Context<Self>) {
        self.http_mcp_workspace_ready(workspace_id, cx);
        let Some(request) = self.pending_tab_moves.remove(&workspace_id) else {
            return;
        };
        let source = request.source_workspace_id;
        if let Err(error) = self.move_tab_tree(request, cx) {
            tracing::warn!(%error, "could not move tab to newly opened window");
            self.report_tab_move_failure(source, error, cx);
            let empty = self
                .windows
                .get(&workspace_id)
                .and_then(|entry| {
                    entry
                        .shell
                        .update(cx, |shell, _| shell.app.clone())
                        .ok()
                        .flatten()
                })
                .is_some_and(|app| app.read(cx).can_close_failed_tab_workspace());
            if empty {
                self.tab_drag.queue_failed_window(workspace_id);
                self.close_failed_tab_windows(cx);
            }
        }
    }

    pub(super) fn close_failed_tab_windows(&mut self, cx: &mut Context<Self>) {
        // Window manifests are saved serially. If another window is closing,
        // retry after it finishes, and recheck that the user has not started
        // a session in this newly created window in the meantime.
        while !self.process_quitting && self.closing_workspaces.is_empty() {
            let Some(workspace_id) = self.tab_drag.next_failed_window() else {
                break;
            };
            let empty = self
                .windows
                .get(&workspace_id)
                .and_then(|entry| {
                    entry
                        .shell
                        .update(cx, |shell, _| shell.app.clone())
                        .ok()
                        .flatten()
                })
                .is_some_and(|app| app.read(cx).can_close_failed_tab_workspace());
            if empty {
                self.request_close_unready_workspace(workspace_id, cx);
            }
        }
    }

    pub(crate) fn update_tab_drag<R>(
        &mut self,
        update: impl FnOnce(&mut TabDragCoordinator) -> R,
    ) -> R {
        update(&mut self.tab_drag)
    }

    pub(crate) fn tab_drag_source(&self) -> Option<&TabDragSource> {
        self.tab_drag.source()
    }

    pub(crate) fn remember_tab_window_size(
        &mut self,
        workspace_id: WorkspaceId,
        size: gpui::Size<gpui::Pixels>,
    ) {
        if self.windows.contains_key(&workspace_id)
            && !self.closing_workspaces.contains(&workspace_id)
        {
            self.tab_drag.remember_normal_size(workspace_id, size);
        }
    }

    pub(crate) fn finish_tab_drag_release(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some((source, release)) = self.tab_drag.finish(id) else {
            return;
        };
        if self.process_quitting || self.closing_workspaces.contains(&source.workspace_id) {
            return;
        }
        let placement = detach_placement(&source, release, cx).or_else(|| {
            // Wayland cannot position a toplevel. Supply a normal size and let
            // the compositor choose its origin through the usual opening path.
            let mut placement = self.startup.main_window_placement(cx);
            placement.display_id = None;
            placement.window_bounds = gpui::WindowBounds::Windowed(gpui::Bounds::new(
                placement.window_bounds.get_bounds().origin,
                source.normal_size,
            ));
            Some(placement)
        });
        if let Err(error) = self.open_workspace_for_tab_at(
            source.workspace_id,
            source.root_tab_id,
            source.revision,
            placement,
            cx,
        ) {
            self.report_tab_move_failure(source.workspace_id, error.to_string(), cx);
        }
    }

    pub(super) fn report_tab_move_failure(
        &self,
        workspace_id: WorkspaceId,
        error: String,
        cx: &mut Context<Self>,
    ) {
        let Some(entry) = self.windows.get(&workspace_id) else {
            return;
        };
        let shell = entry.shell.clone();
        cx.defer(move |cx| {
            let _ = shell.update(cx, |shell, cx| {
                if let Some(app) = &shell.app {
                    app.update(cx, |app, cx| app.report_tab_move_failure(error, cx));
                }
            });
        });
    }
}
pub(super) fn workspace_targets_from_order(
    source: WorkspaceId,
    window_order: &[WorkspaceId],
    is_open: impl Fn(WorkspaceId) -> bool,
) -> Vec<WorkspaceTarget> {
    window_order
        .iter()
        .copied()
        .enumerate()
        .filter(|(_, workspace_id)| *workspace_id != source && is_open(*workspace_id))
        .map(|(index, workspace_id)| WorkspaceTarget {
            workspace_id,
            ordinal: index + 1,
        })
        .collect()
}
