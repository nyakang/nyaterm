//! tmux control-mode (`tmux -CC`) workspace projection.
//!
//! Transport drives the remote control client and reports the whole
//! window/pane tree as `SessionEvent::TmuxState`. This feature owns the
//! desktop projection: each remote pane is registered as a virtual session
//! (a hidden secondary leaf under the host SSH tab) and the active window's
//! layout is mirrored into the host tab's `WorkspacePaneNode` tree, so the
//! existing pane renderer, input routing, and close paths apply unchanged.

use std::collections::HashMap;

use gpui::{Context, IntoElement, SharedString, div, prelude::*, px, rgb, rgba, svg};
use nyaterm_transport::tmux::{TmuxStatePayload, TmuxUiNode, TmuxWindowInfo};
use nyaterm_ui::NyaScrollable as _;
use rust_i18n::t;

use super::NyaTermApp;
use super::text_inputs::TextInputSetup;
use crate::features::view_widgets::color_icon;
use crate::models::{WorkspacePaneNode, WorkspaceSplitDirection};

/// Per host-SSH-session control state the workspace renders from.
#[derive(Debug, Default)]
struct TmuxHostState {
    windows: Vec<TmuxWindowInfo>,
    active_window_id: Option<String>,
    active_pane_session_id: Option<String>,
    /// tmux pane id (`%N`) → virtual session id registered for it. The map is
    /// kept so a pane that disappears from a later layout can be torn down
    /// instead of leaking a hidden session.
    pane_sessions: HashMap<String, String>,
    /// Draft text mirrored from the command input — the field entity owns the
    /// text; this copy only filters the suggestion list.
    command_draft: String,
    command_suggestions_open: bool,
    /// The control client detached/exited while the host SSH session stayed
    /// alive. The entry is kept — its pane index is what lets a later
    /// `tmux -CC` reattach adopt the same virtual pane sessions and what lets
    /// session teardown reap them. Cleared when live state arrives again.
    exited: bool,
}

#[derive(Debug, Default)]
pub(in crate::features) struct TmuxFeatureState {
    /// Host SSH session id → control state. Entries persist across detach /
    /// reattach and only leave when the host session itself is removed.
    hosts: HashMap<String, TmuxHostState>,
}

impl TmuxFeatureState {
    pub(in crate::features) fn is_control_session(&self, session_id: &str) -> bool {
        self.hosts.get(session_id).is_some_and(|host| !host.exited)
    }

    /// Drop a host entry entirely (session closed). Returns the virtual pane
    /// session ids it owned so the caller can reap them too.
    pub(in crate::features) fn remove_host(&mut self, session_id: &str) -> Option<Vec<String>> {
        self.hosts
            .remove(session_id)
            .map(|host| host.pane_sessions.into_values().collect())
    }

    /// Drop a pane's index entry when the pane session itself is removed.
    pub(in crate::features) fn drop_pane_session(&mut self, session_id: &str) {
        for host in self.hosts.values_mut() {
            host.pane_sessions
                .retain(|_, pane_session| pane_session != session_id);
        }
    }

    pub(in crate::features) fn windows_for(&self, session_id: &str) -> &[TmuxWindowInfo] {
        self.hosts
            .get(session_id)
            .map_or(&[], |host| host.windows.as_slice())
    }

    pub(in crate::features) fn active_window_id(&self, session_id: &str) -> Option<&str> {
        self.hosts
            .get(session_id)
            .and_then(|host| host.active_window_id.as_deref())
    }

    pub(in crate::features) fn active_pane_session_id(&self, session_id: &str) -> Option<&str> {
        self.hosts
            .get(session_id)
            .and_then(|host| host.active_pane_session_id.as_deref())
    }

    /// `session_id` is a tmux pane leaf → `(host_session_id, "%N" pane id)`.
    pub(in crate::features) fn pane_for_session<'a>(
        &'a self,
        session_id: &str,
    ) -> Option<(&'a str, &'a str)> {
        self.hosts.iter().find_map(|(host_id, host)| {
            host.pane_sessions
                .iter()
                .find(|(_, pane_session)| pane_session.as_str() == session_id)
                .map(|(pane_id, _)| (host_id.as_str(), pane_id.as_str()))
        })
    }

    fn host_mut(&mut self, session_id: &str) -> Option<&mut TmuxHostState> {
        self.hosts.get_mut(session_id)
    }
}

/// Commands offered in the strip's input dropdown — the same list the Tauri
/// v1 strip presented (`TmuxWindowStrip.tsx`).
const TMUX_COMMON_COMMANDS: &[&str] = &[
    "resize-pane -Z",
    "select-layout even-horizontal",
    "select-layout even-vertical",
    "select-layout main-horizontal",
    "select-layout tiled",
    "kill-pane",
    "respawn-pane -k",
    "rename-window ",
    "break-pane",
    "swap-pane -U",
    "swap-pane -D",
    "last-window",
    "synchronize-panes",
];

impl NyaTermApp {
    /// Fold a transport `TmuxUiNode` into the workspace pane tree. `direction`
    /// uses the same vocabulary as `WorkspacePaneNode` restore (`"vertical"` =
    /// side-by-side columns, `"horizontal"` = stacked rows).
    fn workspace_node_from_tmux(node: &TmuxUiNode) -> Option<WorkspacePaneNode> {
        match node {
            TmuxUiNode::Pane { session_id, .. } => {
                Some(WorkspacePaneNode::leaf(session_id.clone()))
            }
            TmuxUiNode::Split {
                direction,
                ratio: _,
                first,
                second,
            } => {
                let direction = match *direction {
                    "vertical" => WorkspaceSplitDirection::Vertical,
                    "horizontal" => WorkspaceSplitDirection::Horizontal,
                    _ => return None,
                };
                let first = Self::workspace_node_from_tmux(first)?;
                let second = Self::workspace_node_from_tmux(second)?;
                // tmux-driven splits are locked at 50/50 by design (the user
                // asked for half-and-half only): tmux hands out odd cell
                // ratios (25/26 on a 51-row window) and the renderer↔
                // resize-pane feedback then visibly drifts before settling.
                // Pinning the display at half makes the projection stable;
                // sync_pane_sizes still reports the real pixel bounds so tmux
                // itself converges to an even split underneath. The
                // `tmux-split-` id prefix marks the divider as fixed —
                // `workspace_split_resize_handle` renders it non-interactive.
                Some(WorkspacePaneNode::Split {
                    id: format!("tmux-split-{}", crate::models::uuid_v4_like()),
                    direction,
                    ratio_percent: 50,
                    first: Box::new(first),
                    second: Box::new(second),
                })
            }
        }
    }

    fn collect_tmux_panes(
        node: &TmuxUiNode,
        out: &mut Vec<(String, String, String)>, // (pane_id, session_id, name)
    ) {
        match node {
            TmuxUiNode::Pane {
                pane_id,
                session_id,
                name,
            } => out.push((pane_id.clone(), session_id.clone(), name.clone())),
            TmuxUiNode::Split { first, second, .. } => {
                Self::collect_tmux_panes(first, out);
                Self::collect_tmux_panes(second, out);
            }
        }
    }

    /// tmux pane leaves are virtual sessions that share the host's SSH
    /// channel — connection-scoped resources (multiplex handle, remote file
    /// service, stats samplers, remote exec) are keyed on the host session
    /// id, so resolve `host::paneN` back to its host before those lookups.
    pub(in crate::features) fn ssh_scoped_session_id(&self, session_id: &str) -> String {
        self.tmux
            .pane_for_session(session_id)
            .map(|(host, _)| host.to_string())
            .unwrap_or_else(|| session_id.to_string())
    }

    /// Consume a `SessionEvent::TmuxState` update for `session_id` (the host
    /// SSH session running the control client). Returns `root_chrome_dirty`.
    pub(in crate::features) fn handle_session_tmux_state_event(
        &mut self,
        session_id: String,
        state: TmuxStatePayload,
        cx: &mut Context<Self>,
    ) -> bool {
        tracing::info!(
            session_id,
            exited = state.exited,
            windows = state.windows.len(),
            trees = state.windows.iter().filter(|w| w.tree.is_some()).count(),
            "tmux desktop: state event"
        );
        if state.exited {
            // The control client detached/exited; the host channel is back at
            // a plain shell. The pane sessions must NOT be marked
            // disconnected: the reconnect banner would stay as stale buffer
            // text on reattach, Enter would spawn a stray top-level SSH
            // session, and an ownerless pane that is merely disconnected gets
            // picked up as a normal tab by the next reconcile. Hide them
            // instead — they are adopted again when `tmux -CC` reattaches.
            let panes: Vec<String> = self
                .tmux
                .host_mut(&session_id)
                .map(|host| {
                    host.exited = true;
                    host.pane_sessions.values().cloned().collect()
                })
                .unwrap_or_default();
            // Fold only the tmux projection back into the host's shell leaf;
            // sibling splits the user opened manually keep their topology.
            if let Some(mut existing) = self.shell.workspace_pane_root(&session_id).cloned()
                && existing.collapse_tmux_subtree(&session_id)
            {
                if existing.is_split() {
                    self.shell
                        .insert_workspace_pane_root(session_id.clone(), existing);
                } else {
                    // Whole tab was the projection: a lone shell leaf needs no
                    // pane root.
                    self.shell.take_workspace_pane_root(&session_id);
                }
            }
            for pane_session_id in panes {
                self.shell.hide_workspace_session(&pane_session_id);
            }
            // Focus may still sit on a pane whose transport is gone; hand it
            // back to the host shell. Deferred — activation reads surface
            // entities and this handler runs inside an app update.
            if self
                .session
                .active_id()
                .is_some_and(|active| self.tmux.pane_for_session(active).is_some())
            {
                let host_id = session_id.clone();
                let app = cx.entity();
                cx.defer(move |cx| {
                    app.update(cx, |this, cx| {
                        this.activate_workspace_pane(host_id, cx);
                    });
                });
            }
            self.reconcile_terminal_windows();
            self.sync_workspace_split_from_active_tab();
            return true;
        }

        let current_panes: Vec<(String, String, String)> = {
            let mut panes = Vec::new();
            for window in &state.windows {
                if let Some(tree) = &window.tree {
                    Self::collect_tmux_panes(tree, &mut panes);
                }
            }
            panes
        };

        // The active window's pane tree becomes the host tab's pane root — a
        // single-pane window projects as `Leaf`, so the host tab renders the
        // pane's terminal instead of the hidden control-mode channel. tmux
        // stays the layout authority, so the tree is replaced wholesale. The
        // root is inserted BEFORE registering panes so each registration's
        // reconcile/persist already sees the pane as an owned leaf instead of
        // briefly treating it as a new top-level tab.
        let active_tree = state
            .active_window_id
            .as_ref()
            .and_then(|id| state.windows.iter().find(|w| &w.id == id))
            .or_else(|| state.windows.iter().find(|w| w.active))
            .or_else(|| state.windows.first())
            .and_then(|w| w.tree.as_ref());
        let projected_root = active_tree.and_then(Self::workspace_node_from_tmux);
        let projected_leaves: std::collections::HashSet<String> = projected_root
            .as_ref()
            .map(|root| root.session_ids().into_iter().collect())
            .unwrap_or_default();
        if let Some(root) = projected_root {
            tracing::info!(
                session_id,
                leaves = ?root.session_ids(),
                "tmux desktop: inserting pane root"
            );
            // Contain the projection inside the SSH leaf: a tab the user had
            // already split locally keeps its sibling panes (v1 P1 review —
            // overwriting the whole tree silently dropped them and detach
            // could not restore them). Refresh events replace only the prior
            // all-pane subtree, not the outer manual topology.
            let merged = match self.shell.workspace_pane_root(&session_id).cloned() {
                Some(mut existing) => {
                    if existing.replace_tmux_subtree(&session_id, root.clone()) {
                        existing
                    } else {
                        root
                    }
                }
                None => root,
            };
            self.shell
                .insert_workspace_pane_root(session_id.clone(), merged);
        } else if let Some(mut existing) = self.shell.workspace_pane_root(&session_id).cloned()
            && existing.collapse_tmux_subtree(&session_id)
        {
            if existing.is_split() {
                self.shell
                    .insert_workspace_pane_root(session_id.clone(), existing);
            } else {
                self.shell.take_workspace_pane_root(&session_id);
            }
        }

        // A pane absent from every known tree is dead (kill-pane, moved
        // window). Only reap when every window reported a tree — a window
        // whose layout has not arrived yet cannot prove its panes are gone.
        if state.windows.iter().all(|w| w.tree.is_some())
            && let Some(host) = self.tmux.hosts.get(&session_id)
        {
            let live: std::collections::HashSet<&str> = current_panes
                .iter()
                .map(|(_, pane_session_id, _)| pane_session_id.as_str())
                .collect();
            let dead: Vec<String> = host
                .pane_sessions
                .values()
                .filter(|pane_session_id| !live.contains(pane_session_id.as_str()))
                .cloned()
                .collect();
            for pane_session_id in dead {
                let _ = self.session.manager().close(&pane_session_id);
                self.remove_session_state(&pane_session_id, cx);
            }
        }

        let host_metadata = self.session.metadata(&session_id).cloned();
        for (_, pane_session_id, name) in &current_panes {
            // Re-claim unconditionally: a pane respawned by a reattached
            // control client has a fresh event queue with no consumer, and
            // claiming must also precede registration so Output events queued
            // ahead of this update (seed/capture) drain instead of stalling.
            self.session.claim_session_events(pane_session_id);
            if self.session.has_session(pane_session_id) {
                // A pane marked disconnected by an older teardown path gets
                // adopted again here — clear the flag and repaint so Enter
                // goes back to the pane instead of spawning a stray session.
                if self.session.clear_session_disconnected(pane_session_id) {
                    self.sync_terminal_surface_paint(pane_session_id, cx);
                }
            } else if let Some(metadata) = host_metadata.clone() {
                self.register_session(pane_session_id, metadata);
            }
            // Panes outside the projected window own no leaf; hidden_sessions
            // is what keeps them out of the tab strip. Leaves in the root are
            // unhidden so ownership alone controls their visibility.
            if projected_leaves.contains(pane_session_id.as_str()) {
                self.shell.workspace.hidden_sessions.remove(pane_session_id);
            } else {
                self.shell.hide_workspace_session(pane_session_id);
            }
            if !name.is_empty() {
                self.session
                    .set_dynamic_title(pane_session_id, Some(name.clone()));
            }
        }
        self.reconcile_terminal_windows();
        if self.session.restore_is_complete() {
            self.persist_open_tabs();
        }

        let focus_pane = state
            .active_pane_session_id
            .clone()
            .or_else(|| projected_leaves.iter().next().cloned());

        // A control client starts at tmux's default 80x24: windows and
        // `split-window` halves are laid out at that size, and tmux's resize
        // then piles all later client growth onto the last pane — locking in
        // the lopsided layout users see. Report the host surface's real grid
        // once on takeover so the remote window sizes against it from the
        // start (`host_rx` still drains `SshCommand::Resize` into
        // `refresh-client -C`; `sync_pane_sizes` keeps it current after).
        if !self.tmux.hosts.contains_key(&session_id) {
            let (rows, cols) = self.terminal_grid_size_for_session(Some(&session_id));
            if let Err(error) = self
                .session
                .manager()
                .resize(&session_id, cols as u16, rows as u16)
            {
                tracing::warn!(
                    session_id = %session_id,
                    %error,
                    "tmux client size report failed"
                );
            }
        }

        let host = self.tmux.hosts.entry(session_id.clone()).or_default();
        host.exited = false;
        host.windows = state.windows;
        host.active_window_id = state.active_window_id;
        host.active_pane_session_id = state.active_pane_session_id;
        host.pane_sessions = current_panes
            .iter()
            .map(|(pane_id, pane_session_id, _)| (pane_id.clone(), pane_session_id.clone()))
            .collect();

        // The host channel speaks control protocol, not a shell — keystrokes
        // routed there would be swallowed as garbage commands. While the user
        // is looking at this host's tab, focus must sit on the pane tmux
        // reports as active (or the only projected leaf as a fallback). Never
        // steal focus when another tab owns it. Activation touches terminal
        // surface entities, so it is deferred out of the event-pump update —
        // same lease-safety pattern TerminalSurface uses for its own
        // app callbacks.
        if let Some(pane_session_id) = focus_pane {
            let host_id = session_id.clone();
            let app = cx.entity();
            cx.defer(move |cx| {
                app.update(cx, |this, cx| {
                    let viewing_host = this
                        .session
                        .active_id()
                        .is_some_and(|active| this.tab_root_for_session(active) == host_id);
                    if viewing_host && this.session.active_id() != Some(pane_session_id.as_str()) {
                        this.activate_workspace_pane(pane_session_id, cx);
                    }
                });
            });
        }

        self.sync_workspace_split_from_active_tab();
        tracing::info!(
            session_id,
            active = ?self.session.active_id(),
            split_leaves = ?self.shell.workspace_split().map(|n| n.session_ids()),
            hidden = ?self.shell.session_tab_is_hidden(session_id.as_str()),
            "tmux desktop: projection synced"
        );
        true
    }

    /// Queue a tmux control-channel command (`select-window -t @N`,
    /// `new-window`, `split-window`, `kill-pane -t %N`, ...) on the host session.
    /// Transport rejects the send unless the session is actually in control
    /// mode, so a command can never leak into a plain shell.
    pub(in crate::features) fn tmux_send_command(&mut self, session_id: &str, line: &str) {
        if let Err(error) = self.session.manager().tmux_send_command(session_id, line) {
            self.shell
                .set_status(format!("tmux command failed: {error}"));
        }
    }

    /// Keep tmux's active pane in sync with the locally focused leaf so
    /// target-less commands (`split-window`, `kill-pane`) hit the pane the
    /// user is looking at.
    pub(in crate::features) fn tmux_sync_active_pane(&mut self, session_id: &str) {
        if let Some((host_id, pane_id)) = self.tmux.pane_for_session(session_id) {
            let (host_id, command) = (host_id.to_string(), format!("select-pane -t '{pane_id}'"));
            self.tmux_send_command(&host_id, &command);
        }
    }

    /// Detach the tmux control client; the remote tmux session keeps running
    /// and the channel returns to the interactive shell.
    pub(in crate::features) fn tmux_detach(&mut self, session_id: &str) {
        if let Err(error) = self.session.manager().tmux_detach(session_id) {
            self.shell
                .set_status(format!("tmux detach failed: {error}"));
        }
    }

    /// `tmux.command.<host>` text-input submission: send the line on the
    /// control channel and clear the draft box.
    pub(in crate::features) fn submit_tmux_command_input(
        &mut self,
        session_id: &str,
        line: &str,
        cx: &mut Context<Self>,
    ) {
        let line = line.trim();
        if !line.is_empty() {
            self.tmux_send_command(session_id, line);
        }
        self.reset_text_input(&format!("tmux.command.{session_id}"), "", cx);
        if let Some(host) = self.tmux.host_mut(session_id) {
            host.command_draft.clear();
            host.command_suggestions_open = false;
        }
    }

    /// `tmux.command.<host>` draft changes only feed the suggestion filter —
    /// typing also opens the list, like the Tauri strip's focus handler did.
    pub(in crate::features) fn on_tmux_command_draft(
        &mut self,
        session_id: &str,
        draft: String,
        cx: &mut Context<Self>,
    ) {
        if let Some(host) = self.tmux.host_mut(session_id) {
            host.command_draft = draft;
            host.command_suggestions_open = true;
            cx.notify();
        }
    }

    /// Focus leaving the command input closes its suggestion list.
    pub(in crate::features) fn on_tmux_command_blurred(
        &mut self,
        session_id: &str,
        cx: &mut Context<Self>,
    ) {
        if let Some(host) = self.tmux.host_mut(session_id) {
            host.command_suggestions_open = false;
            cx.notify();
        }
    }

    /// The tmux window strip rendered at the bottom of a control session's
    /// tab — the WindTerm-style bar from the Tauri port: tmux mark, detach,
    /// one sub-tab per remote window (select → `select-window`, × →
    /// `kill-window`), `+` for `new-window`, then pane splits and the
    /// free-form command input (the `-CC` channel swallows prefix keys, so
    /// tmux-level actions must go through the control channel).
    pub(in crate::features) fn tmux_window_strip(
        &mut self,
        host_session_id: &str,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let palette = self.theme_palette();
        let host_id = host_session_id.to_string();
        let windows = self.tmux.windows_for(&host_id).to_vec();
        let active_window_id = self
            .tmux
            .active_window_id(&host_id)
            .map(str::to_string)
            .or_else(|| windows.iter().find(|w| w.active).map(|w| w.id.clone()));

        let mut strip = div()
            .h(px(36.))
            .flex()
            .items_center()
            .gap_0p5()
            .px_2()
            .bg(rgb(palette.surface))
            .border_t_1()
            .border_color(rgb(palette.border));

        // Passive tmux mark — the control channel drives this tab. The asset
        // is the Tauri strip's inline SVG verbatim (clipPath gives the rounded
        // tile that absolutely-positioned div children cannot reproduce); the
        // light/dark fill pair is picked from the palette luminance like
        // `theme_mode`.
        let dark = tmux_strip_dark(palette);
        let mark = if dark {
            "color/session/tmux-mark-dark.svg"
        } else {
            "color/session/tmux-mark-light.svg"
        };
        strip = strip.child(color_icon(mark, 18.));

        // Detach is a session-level action, so it sits with the mark rather
        // than among the per-window controls; the warning tint marks it as
        // the one action that drops the whole pane set at once (same as the
        // Tauri strip, including the warning-tinted hover).
        let host = host_id.clone();
        strip = strip.child(
            div()
                .id(SharedString::from(format!("tmux-detach-{host_id}")))
                .cursor_pointer()
                .flex_none()
                .ml_1p5()
                .w(px(28.))
                .h(px(28.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_sm()
                .hover(|style| style.bg(rgba((palette.warning << 8) | 0x1a)))
                .child(
                    svg()
                        .size(px(16.))
                        .path("icons/session/unplug.svg")
                        .text_color(rgb(palette.warning)),
                )
                .child(
                    // GPUI has no `title` attribute on divs; keep the tooltip
                    // as a visually hidden label so localized strings stay
                    // wired (same pattern as `tmux_strip_icon_button`).
                    div()
                        .invisible()
                        .w_0()
                        .h_0()
                        .overflow_hidden()
                        .child(t!("tmux.detach").to_string()),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.tmux_detach(&host);
                    cx.notify();
                })),
        );
        strip = strip.child(div().mx_1p5().h(px(20.)).w(px(1.)).bg(rgb(palette.border)));

        for window in windows {
            let is_active = Some(window.id.as_str()) == active_window_id.as_deref();
            let window_id = window.id.clone();
            let host = host_id.clone();
            // The active underline spans the name and the close button's slot,
            // like the Tauri strip's group border.
            let mut tab = div()
                .id(SharedString::from(format!(
                    "tmux-window-{host_id}-{}",
                    window.id
                )))
                .flex()
                .items_center()
                .pr_1p5()
                .border_b_2()
                .border_color(if is_active {
                    rgb(palette.primary)
                } else {
                    rgb(0x00000000)
                });
            let name_button = div()
                .id(SharedString::from(format!(
                    "tmux-select-{host_id}-{}",
                    window.id
                )))
                .cursor_pointer()
                .flex()
                .items_center()
                .gap_1()
                .rounded_t_sm()
                .py_1()
                .pl_2p5()
                .pr_1()
                .text_xs()
                .text_color(if is_active {
                    rgb(palette.text)
                } else {
                    rgb(palette.text_muted)
                })
                .when(is_active, |b| b.font_weight(gpui::FontWeight::SEMIBOLD))
                .when(!is_active, |b| {
                    b.hover(|style| style.text_color(rgb(palette.text)))
                })
                .child(
                    div()
                        .text_color(rgb(palette.text_dimmed))
                        .child(format!("{}:", window.index)),
                )
                .child(window.name.clone())
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.tmux_send_command(&host, &format!("select-window -t '{window_id}'"));
                    cx.notify();
                }));
            tab = tab.child(name_button);
            let window_id = window.id.clone();
            let host = host_id.clone();
            tab = tab.child(tmux_strip_icon_button(
                format!("tmux-kill-window-{host_id}-{}", window.id),
                "icons/window/close.svg",
                palette.text_muted,
                palette,
                &t!("tmux.killWindow"),
                cx.listener(move |this, _, _, cx| {
                    this.tmux_send_command(&host, &format!("kill-window -t '{window_id}'"));
                    cx.notify();
                }),
            ));
            strip = strip.child(tab);
        }

        let host = host_id.clone();
        strip = strip.child(tmux_strip_icon_button(
            format!("tmux-new-window-{host_id}"),
            "icons/session/plus.svg",
            palette.text_muted,
            palette,
            &t!("tmux.newWindow"),
            cx.listener(move |this, _, _, cx| {
                this.tmux_send_command(&host, "new-window");
                cx.notify();
            }),
        ));

        strip = strip.child(div().flex_1());

        // tmux `-h` splits left/right (a vertical divider, Columns2 in the
        // Tauri strip); `-v` stacks top/bottom (Rows2).
        let active_pane_id = self
            .tmux
            .active_pane_session_id(&host_id)
            .and_then(|pane_session| {
                self.tmux
                    .pane_for_session(pane_session)
                    .map(|(_, pane_id)| pane_id.to_string())
            });
        let target = active_pane_id
            .map(|pane_id| format!(" -t '{pane_id}'"))
            .unwrap_or_default();
        let host = host_id.clone();
        let target_h = target.clone();
        strip = strip.child(tmux_strip_icon_button(
            format!("tmux-split-h-{host_id}"),
            "icons/session/columns-2.svg",
            palette.text_muted,
            palette,
            &t!("tmux.splitVertical"),
            cx.listener(move |this, _, _, cx| {
                this.tmux_send_command(&host, &format!("split-window -h{target_h}"));
                cx.notify();
            }),
        ));
        let host = host_id.clone();
        strip = strip.child(tmux_strip_icon_button(
            format!("tmux-split-v-{host_id}"),
            "icons/session/rows-2.svg",
            palette.text_muted,
            palette,
            &t!("tmux.splitHorizontal"),
            cx.listener(move |this, _, _, cx| {
                this.tmux_send_command(&host, &format!("split-window -v{target}"));
                cx.notify();
            }),
        ));

        strip = strip.child(div().mx_1p5().h(px(20.)).w(px(1.)).bg(rgb(palette.border)));

        let input = self.text_input_box(
            format!("tmux.command.{host_id}"),
            "",
            TextInputSetup::placeholder(t!("tmux.commandPlaceholder").to_string())
                .submit_on_enter(),
            cx,
        );
        let (suggestions_open, draft) = self
            .tmux
            .hosts
            .get(&host_id)
            .map(|host| (host.command_suggestions_open, host.command_draft.clone()))
            .unwrap_or((false, String::new()));
        let host = host_id.clone();
        let mut command_box = div()
            // Event listeners only register on stateful elements — without an
            // id the open-on-mouse-down handler below can never fire.
            .id(SharedString::from(format!("tmux-command-box-{host_id}")))
            .relative()
            .w(px(224.))
            .font_family(crate::features::shell::gpui_code_font_family())
            // Clicking into the box opens the suggestion list — v1 hooked the
            // same behavior to the input's focus event.
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    if let Some(host) = this.tmux.host_mut(&host) {
                        host.command_suggestions_open = true;
                        cx.notify();
                    }
                }),
            )
            .child(input);
        if suggestions_open {
            let query = draft.trim();
            let suggestions: Vec<&str> = TMUX_COMMON_COMMANDS
                .iter()
                .copied()
                .filter(|entry| entry.trim().contains(query))
                .collect();
            if !suggestions.is_empty() {
                // Popup rule: `overflow_*_scrollbar` must not sit on the
                // absolutely-positioned node itself — the wrapper only
                // inherits size/flex and the position styles would land on
                // the inner content, breaking placement. The shell carries
                // the chrome and the in-flow child carries the scroll.
                let list = div()
                    .id(SharedString::from(format!("tmux-suggestions-{host_id}")))
                    .absolute()
                    .bottom_full()
                    .right_0()
                    .mb_1()
                    .w(px(256.))
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(palette.border))
                    .bg(rgb(palette.surface_elevated))
                    .p_1()
                    .shadow_md();
                let mut entries = div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .max_h(px(256.))
                    .overflow_y_scrollbar();
                for entry in suggestions {
                    let host = host_id.clone();
                    entries = entries.child(
                        div()
                            .id(SharedString::from(format!(
                                "tmux-suggestion-{host_id}-{entry}"
                            )))
                            .w_full()
                            .rounded_sm()
                            .px_2()
                            .py_1p5()
                            .cursor_pointer()
                            .text_xs()
                            .text_color(rgb(palette.text))
                            .hover(|style| style.bg(rgb(palette.hover)))
                            // Picking a suggestion fires the command at mouse
                            // down, before the input's Blurred event can
                            // close the list — and must not bubble into the
                            // container's open handler and re-arm it.
                            .on_mouse_down(
                                gpui::MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.tmux_send_command(&host, entry);
                                    if let Some(host) = this.tmux.host_mut(&host) {
                                        host.command_draft.clear();
                                        host.command_suggestions_open = false;
                                    }
                                    this.reset_text_input(&format!("tmux.command.{host}"), "", cx);
                                    cx.notify();
                                }),
                            )
                            .child(entry.to_string()),
                    );
                }
                command_box = command_box.child(list.child(entries));
            }
        }
        strip = strip.child(command_box);

        strip.into_any_element()
    }
}

/// Light/dark pick for the strip's tmux mark — same luminance rule
/// `theme_mode` applies to the palette in `nyaterm-ui::theme_bridge`
/// (the helper there stays private, so this repeats the tiny formula).
fn tmux_strip_dark(palette: nyaterm_ui::ThemePalette) -> bool {
    fn channel(v: u32) -> f32 {
        let v = v as f32 / 255.0;
        if v <= 0.03928 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    }
    let luminance = 0.2126 * channel((palette.bg >> 16) & 0xff)
        + 0.7152 * channel((palette.bg >> 8) & 0xff)
        + 0.0722 * channel(palette.bg & 0xff);
    luminance < 0.5
}

/// Icon button used across the strip; the title carries the localized tooltip.
fn tmux_strip_icon_button(
    id: String,
    icon: &'static str,
    tint: u32,
    palette: nyaterm_ui::ThemePalette,
    title: &str,
    click: impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(SharedString::from(id))
        .cursor_pointer()
        .flex_none()
        .w(px(28.))
        .h(px(28.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_sm()
        .hover(|style| style.bg(rgb(palette.hover)))
        .child(svg().size(px(14.)).path(icon).text_color(rgb(tint)))
        .child(
            // GPUI has no `title` attribute on divs; keep the tooltip as a
            // visually hidden label so localized strings stay wired.
            div()
                .invisible()
                .w_0()
                .h_0()
                .overflow_hidden()
                .child(title.to_string()),
        )
        .on_click(click)
}
