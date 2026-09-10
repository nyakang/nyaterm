use std::time::{Duration, Instant};

use crate::models::{MainMode, NavItem, PanelResizeSide, PanelSide};
use gpui::{
    AnyElement, App, ClickEvent, Context, Div, IntoElement, KeyDownEvent, MouseButton,
    MouseMoveEvent, MouseUpEvent, NavigationDirection, ObjectFit, Render, SharedString, Stateful,
    Window, canvas, deferred, div, img, prelude::*, px, rgb, rgba, svg,
};
use rust_i18n::t;

use super::NyaTermApp;
use super::ai::AiFullAccessSetting;
use super::mcp::{McpApprovalDecision, McpApprovalRequest};
use super::terminal::{FULL_SHELL_PAINT_COUNT, terminal_surface_paint_count};
use super::view_widgets::{
    full_window_input_layer, full_window_overlay_layer, modal_scrim_is_drawn, passive_overlay_layer,
};
use crate::features::perf::{GpuiPerfContext, record_gpui_perf_sample};
use crate::features::runtime_jobs::ActivitySide;
use crate::theme::ThemePalette;

const WALLPAPER_TILE_ELEMENT_LIMIT: usize = 8192;
const WALLPAPER_TILE_MIN_SIZE: f32 = 8.;
const SSH_AUTH_PROMPT_PRIORITY: usize = usize::MAX;

/// Which overlays the root chrome should render this frame.
///
/// Computed from `NyaTermApp` directly. This used to be read back from a
/// snapshot the same `Render` pass had just published into `OverlayStore`,
/// with a fallback that recomputed exactly these expressions.
struct OverlayFlags {
    tab_actions_open: bool,
    color_picker_open: bool,
    session_info_open: bool,
    multi_line_paste_open: bool,
    terminal_actions_open: bool,
    action_link_menu_open: bool,
    action_link_tooltip_open: bool,
    command_suggestions_open: bool,
    credential_suggestions_open: bool,
    locked: bool,
}

impl NyaTermApp {
    /// Capture the initiating window before deferring across entity leases.
    pub(in crate::features) fn notify_operation(
        &self,
        key: &'static str,
        kind: nyaterm_ui::notification::NyaNotificationKind,
        message: String,
        cx: &mut Context<Self>,
    ) {
        let target = cx
            .active_window()
            .or_else(|| self.shell.main_window().map(Into::into));
        self.notify_operation_at(target, key, kind, message, cx);
    }

    pub(in crate::features) fn notify_background_operation(
        &self,
        key: &'static str,
        kind: nyaterm_ui::notification::NyaNotificationKind,
        message: String,
        cx: &mut Context<Self>,
    ) {
        self.notify_operation_at(
            self.shell.main_window().map(Into::into),
            key,
            kind,
            message,
            cx,
        );
    }

    pub(in crate::features) fn notify_operation_at(
        &self,
        target: Option<gpui::AnyWindowHandle>,
        key: &'static str,
        kind: nyaterm_ui::notification::NyaNotificationKind,
        message: String,
        cx: &mut Context<Self>,
    ) {
        if let Some(target) = target {
            cx.defer(move |cx| {
                let _ = target.update(cx, |_, window, cx| {
                    use nyaterm_ui::notification::NyaNotificationWindowExt as _;
                    window.notify_operation(key, kind, message, cx);
                });
            });
        }
    }

    /// Run an app update after the current GPUI entity leases are released.
    ///
    /// The weak handle lets the deferred work disappear normally during application
    /// teardown instead of keeping the root entity alive until the callback runs.
    pub(in crate::features) fn defer_app_update(
        &self,
        cx: &mut Context<Self>,
        update: impl FnOnce(&mut Self, &mut Context<Self>) + 'static,
    ) {
        let app = cx.entity().downgrade();
        cx.defer(move |cx| {
            let Some(app) = app.upgrade() else {
                return;
            };
            app.update(cx, update);
        });
    }

    pub(in crate::features) fn gpui_perf_context(
        &self,
        flat_row_count: usize,
        cache_hit: Option<bool>,
    ) -> GpuiPerfContext {
        GpuiPerfContext {
            connection_count: self.connection_state.connections().len(),
            group_count: self.connection_state.groups().len(),
            flat_row_count,
            cache_hit,
            full_shell_paint_count: FULL_SHELL_PAINT_COUNT
                .load(std::sync::atomic::Ordering::Relaxed),
            surface_paint_count: terminal_surface_paint_count(),
            left_panel: self
                .current_left_panel()
                .map(|panel| panel.persistence_id()),
            right_panel: self
                .current_right_panel()
                .map(|panel| panel.persistence_id()),
            resize_active: self.shell.panel_resize_active(),
        }
    }

    pub(crate) fn start_after_window_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_component_theme(cx);
        // The connections panel renders from a snapshot, so it needs one before its
        // first paint. Later ones come from store replies and panel interactions.
        self.flush_connection_panel_snapshot(cx);
        self.flush_transfer_panel_snapshot(cx);
        self.flush_ai_panel_snapshot(cx);
        self.ensure_visible_notes_loaded(cx);
        self.refresh_window_render_inputs(window, cx);
        self.ensure_shortcut_interceptor(cx);
        self.queue_cloud_sync_history_refresh(None, cx);
        self.queue_wallpaper_refresh(cx);
        self.start_runtime_data_plane_drain(cx);
        self.start_tunnel_event_drain(cx);
        self.start_translation_event_drain(cx);
        self.start_update_event_drain(cx);
        self.start_github_gist_auth_event_drain(cx);
        self.start_command_persistence_event_drain(cx);
        self.start_stats_event_drain(cx);
        self.start_gpu_event_drain(cx);
        self.start_npu_event_drain(cx);
        self.start_process_event_drain(cx);
        self.start_docker_event_drain(cx);
        self.start_transfer_event_drain(cx);
        self.start_ai_chat_event_drain(cx);
        self.start_ai_discovery_event_drain(cx);
        self.start_mcp_host_request_drain(cx);
        self.start_recording_event_drain(cx);
        self.start_session_start_event_drain(cx);
        self.start_credential_autofill_match_drain(cx);
        self.start_remote_desktop_event_drain(cx);
        self.start_prompt_activation_drain(window, cx);
        self.start_shell_persistence_debounce(cx);
        // The restored panel width decides which process-table sort columns exist, and
        // nothing has resized yet, so this is where the initial value goes in.
        self.reconcile_remote_process_sort_columns();
        // The first snapshot. Nothing has mutated yet, so no boundary has fired, and a
        // panel with no snapshot renders empty.
        self.flush_remote_panel_snapshots(cx);
        self.ensure_cursor_blink_clock(cx);
        self.ensure_header_status_clock(cx);
        self.ensure_idle_lock_clock(cx);
        // A focus request can only be honoured once its element exists, which is a
        // result of this paint; arming here is both cheap and the earliest correct
        // point.
        self.ensure_pending_focus_clock(cx);
        self.ensure_post_start_work_clock(cx);
        self.try_restore_open_tabs(window, cx);
        let pending_session_start = self.session.start_has_pending();
        let should_pump = !self.session.restore_is_complete()
            && self
                .stores
                .startup_restore
                .update(cx, |store, _| store.can_pump_queue(pending_session_start));
        if should_pump {
            self.pump_startup_restore_queue(window, cx);
        }

        self.ensure_terminal_focus_reporting(window, cx);
        self.ensure_rdp_focus_reporting(window, cx);
    }

    fn root_chrome(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Stateful<Div> {
        self.ensure_paint_theme_caches();
        let palette = self.theme_palette();
        let wallpaper = self.shell.wallpaper_asset().cloned();
        let wallpaper_opacity =
            (self.settings.summary().background_image_opacity.min(100) as f32) / 100.0;
        let wallpaper_fit = self.settings.summary().background_image_fit.clone();
        let root = div()
            .id(SharedString::from("nyaterm-root"))
            .key_context(crate::shortcuts::WORKSPACE_KEY_CONTEXT)
            .size_full()
            .relative()
            .bg(self.shell_transparent_color(palette.bg))
            .text_color(rgb(palette.text))
            .font(self.gpui_ui_font().font())
            .text_size(px(self.settings.summary().ui_font_size.clamp(12, 24) as f32))
            .on_click(cx.listener(|this, _, _, _| {
                this.mark_user_activity();
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    let transfer_rename_dismissed = this.dismiss_transfer_rename_if_open(cx);
                    let remote_menus_open = this.remote_ops.docker_menus_open();
                    let ai_menus_open = this.ai.transient_menus_are_open();
                    let changed =
                        this.shell.close_root_menus() || remote_menus_open || ai_menus_open;
                    if transfer_rename_dismissed {
                        // 传输面板独立维护快照；根层点击关闭行内重命名后也要同步它。
                        this.defer_transfer_panel_snapshot_flush(cx);
                    }
                    if changed {
                        this.remote_ops.close_docker_menus();
                        if remote_menus_open {
                            this.defer_remote_panel_snapshot_flush(cx);
                        }
                        if ai_menus_open {
                            this.ai.close_transient_menus();
                            this.defer_ai_panel_snapshot_flush(cx);
                        }
                        cx.notify();
                    }
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, _, cx| {
                    if this.dismiss_transfer_rename_if_open(cx) {
                        // 右键点击同样可能只关闭行内重命名，不能依赖后续菜单刷新。
                        this.defer_transfer_panel_snapshot_flush(cx);
                    }
                }),
            )
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if this.handle_global_shortcut(event, window, cx) {
                    cx.stop_propagation();
                }
            }))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                this.reconcile_root_pointer_interactions(event, cx);
                if event.dragging() {
                    this.update_transfer_browser_column_resize(event, cx);
                    this.update_asset_column_resize(f32::from(event.position.x), cx);
                    this.update_panel_resize(event, cx);
                    this.update_transfer_height_resize(event, cx);
                    this.update_bottom_panel_resize(event, cx);
                    this.update_panel_stack_resize(event, cx);
                    this.update_workspace_split_resize(event, cx);
                }
                if this.maybe_send_terminal_any_motion_report(event, cx) {
                    return;
                }
                this.update_terminal_selection_drag(event, cx);
                if event.dragging() {
                    this.update_terminal_scrollbar_drag(event, cx);
                }
                this.update_action_link_hover(event, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, event: &MouseUpEvent, _, cx| {
                    this.finish_transfer_browser_column_resize(cx);
                    this.finish_asset_column_resize(cx);
                    this.finish_panel_resize(cx);
                    this.finish_transfer_height_resize(cx);
                    this.finish_bottom_panel_resize(cx);
                    this.finish_panel_stack_resize(cx);
                    this.finish_workspace_split_resize(cx);
                    this.finish_terminal_selection(event, cx);
                    this.finish_terminal_scrollbar_drag(cx);
                    this.clear_terminal_window_drop(cx);
                    this.clear_session_tab_drag(cx);
                }),
            )
            .on_mouse_up(
                MouseButton::Navigate(NavigationDirection::Back),
                cx.listener(|this, _event: &MouseUpEvent, window, cx| {
                    if this.current_left_panel() == Some(NavItem::Transfers) {
                        cx.stop_propagation();
                        this.open_transfer_browser_history(1, window, cx);
                    }
                }),
            )
            .on_mouse_up(
                MouseButton::Navigate(NavigationDirection::Forward),
                cx.listener(|this, _event: &MouseUpEvent, window, cx| {
                    if this.current_left_panel() == Some(NavItem::Transfers) {
                        cx.stop_propagation();
                        this.open_transfer_browser_history(-1, window, cx);
                    }
                }),
            )
            .on_mouse_up(
                MouseButton::Right,
                cx.listener(|this, event: &MouseUpEvent, _, cx| {
                    if this.finish_terminal_mouse_report(event, cx) {
                        cx.stop_propagation();
                    }
                }),
            )
            .on_mouse_up(
                MouseButton::Middle,
                cx.listener(|this, event: &MouseUpEvent, _, cx| {
                    if this.finish_terminal_mouse_report(event, cx) {
                        cx.stop_propagation();
                    }
                }),
            )
            .when_some(wallpaper, |this, wallpaper| {
                if wallpaper_fit == "tile" {
                    let image = wallpaper.image().clone();
                    let dimensions = wallpaper.dimensions();
                    let layer = div()
                        .absolute()
                        .inset_0()
                        .overflow_hidden()
                        .opacity(wallpaper_opacity)
                        .child(
                            canvas(
                                |_, _, _| (),
                                move |bounds, (), window, _| {
                                    let viewport = (
                                        f32::from(bounds.size.width),
                                        f32::from(bounds.size.height),
                                    );
                                    let (tile_width, tile_height) = fit_wallpaper_tile_size(
                                        viewport,
                                        (dimensions.0 as f32, dimensions.1 as f32),
                                    );
                                    let (columns, rows) =
                                        wallpaper_tile_grid(viewport, (tile_width, tile_height));
                                    for row in 0..rows {
                                        for column in 0..columns {
                                            let tile_bounds = gpui::Bounds::new(
                                                gpui::point(
                                                    bounds.origin.x
                                                        + px(column as f32 * tile_width),
                                                    bounds.origin.y + px(row as f32 * tile_height),
                                                ),
                                                gpui::size(px(tile_width), px(tile_height)),
                                            );
                                            let _ = window.paint_image(
                                                tile_bounds,
                                                tile_bounds,
                                                gpui::Corners::default(),
                                                image.clone(),
                                                0,
                                                false,
                                            );
                                        }
                                    }
                                },
                            )
                            .size_full(),
                        );
                    return this.child(layer);
                }
                let object_fit = match wallpaper_fit.as_str() {
                    "contain" => ObjectFit::Contain,
                    "stretch" | "fill" => ObjectFit::Fill,
                    _ => ObjectFit::Cover,
                };
                let image = img(wallpaper.image().clone())
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .object_fit(object_fit)
                    .opacity(wallpaper_opacity);
                this.child(image)
            })
            .child(
                div()
                    .flex()
                    .flex_col()
                    .size_full()
                    .child(self.title_bar(window, cx))
                    .child(self.workspace_surface(palette, window, cx)),
            );
        self.with_shortcut_action_handlers(root, cx)
    }

    fn workspace_surface(
        &mut self,
        palette: ThemePalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let started_at = Instant::now();
        let output = if self.shell.main_mode() == MainMode::Page
            && self.shell.selected_nav() == NavItem::Settings
        {
            div()
                .flex()
                .flex_1()
                .min_h_0()
                .bg(self.shell_surface_color(palette.bg))
                .child(self.settings_view(cx))
                .into_any_element()
        } else {
            let compact_layout = !cfg!(target_os = "macos");
            let has_left_activity_items = self.activity_side_has_items(ActivitySide::Left);
            let has_right_activity_items = self.activity_side_has_items(ActivitySide::Right);
            let left_overlay_mode = compact_layout && self.shell.viewport_size().0 < 1024.;
            let right_overlay_mode = compact_layout && self.shell.viewport_size().0 < 768.;
            // Floating selections are transient and independent from the
            // persisted docked active/open stacks.
            let floating_mode = self.shell.panel_is_floating();
            let left_floating = floating_mode
                .then(|| self.shell.floating_panel(PanelSide::Left))
                .flatten();
            let right_floating = floating_mode
                .then(|| self.shell.floating_panel(PanelSide::Right))
                .flatten();
            let left_drawer_open = has_left_activity_items
                && left_overlay_mode
                && self.shell.mobile_left_panel_open()
                && self.left_side_open();
            let right_drawer_open = has_right_activity_items
                && right_overlay_mode
                && self.shell.mobile_right_panel_open()
                && self.right_side_open();
            let mut surface = div()
                .flex()
                .flex_1()
                .min_h_0()
                .relative()
                .overflow_hidden()
                .bg(self.shell_transparent_color(palette.bg))
                .when(has_left_activity_items, |this| {
                    this.child(self.activity_bar(ActivitySide::Left, cx))
                })
                .when(
                    has_left_activity_items
                        && self.left_side_open()
                        && !left_overlay_mode
                        && !floating_mode,
                    |this| {
                        this.child(self.sidebar(false, window, cx))
                            .child(self.panel_resize_handle(PanelResizeSide::Left, cx))
                    },
                )
                .child(self.main_surface(cx))
                .when(
                    has_right_activity_items
                        && self.right_side_open()
                        && !right_overlay_mode
                        && !floating_mode,
                    |this| {
                        this.child(self.panel_resize_handle(PanelResizeSide::Right, cx))
                            .child(self.right_panel(false, window, cx))
                    },
                )
                .when(has_right_activity_items, |this| {
                    this.child(self.activity_bar(ActivitySide::Right, cx))
                });

            if let Some(panel) = left_floating {
                let width = self.shell.left_panel_width();
                surface = surface.child(
                    div()
                        .id("floating-left-panel")
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left(px(40.))
                        .w(px(width))
                        .flex()
                        .border_r_1()
                        .border_color(rgb(palette.border))
                        .bg(self.shell_surface_color(palette.surface))
                        .shadow_lg()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(div().flex_1().min_w_0().child(self.floating_side_panel(
                            PanelSide::Left,
                            panel,
                            window,
                            cx,
                        )))
                        .child(self.panel_resize_handle(PanelResizeSide::Left, cx))
                        .child(
                            div()
                                .id("floating-left-panel-close")
                                .absolute()
                                .top(px(6.))
                                .right(px(8.))
                                .w(px(20.))
                                .h(px(20.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .child("×")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.close_floating_panel(PanelSide::Left, cx);
                                })),
                        ),
                );
            }

            if let Some(panel) = right_floating {
                let width = self.shell.right_panel_width();
                surface = surface.child(
                    div()
                        .id("floating-right-panel")
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .right(px(40.))
                        .w(px(width))
                        .flex()
                        .border_l_1()
                        .border_color(rgb(palette.border))
                        .bg(self.shell_surface_color(palette.surface))
                        .shadow_lg()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(self.panel_resize_handle(PanelResizeSide::Right, cx))
                        .child(div().flex_1().min_w_0().child(self.floating_side_panel(
                            PanelSide::Right,
                            panel,
                            window,
                            cx,
                        )))
                        .child(
                            div()
                                .id("floating-right-panel-close")
                                .absolute()
                                .top(px(6.))
                                .right(px(8.))
                                .w(px(20.))
                                .h(px(20.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .child("×")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.close_floating_panel(PanelSide::Right, cx);
                                })),
                        ),
                );
            }

            if left_drawer_open || right_drawer_open {
                surface = surface.child(
                    full_window_input_layer("mobile-panel-backdrop")
                        .bg(rgba(0x00000080))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.shell.close_mobile_panels();
                            cx.notify();
                        })),
                );
            }

            if left_drawer_open {
                surface = surface.child(
                    div()
                        .id("mobile-left-drawer")
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left(px(40.))
                        .flex()
                        .shadow_lg()
                        .on_click(|_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .h_full()
                                .flex()
                                .flex_col()
                                .child(self.mobile_drawer_header("mobile-left-close", true, cx))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_h_0()
                                        .flex()
                                        .child(self.sidebar(true, window, cx)),
                                ),
                        ),
                );
            }

            if right_drawer_open {
                surface = surface.child(
                    div()
                        .id("mobile-right-drawer")
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .right(px(40.))
                        .flex()
                        .shadow_lg()
                        .on_click(|_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .h_full()
                                .flex()
                                .flex_col()
                                .child(self.mobile_drawer_header("mobile-right-close", false, cx))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_h_0()
                                        .flex()
                                        .child(self.right_panel(true, window, cx)),
                                ),
                        ),
                );
            }

            surface.into_any_element()
        };
        record_gpui_perf_sample(
            "workspace_surface",
            started_at.elapsed(),
            self.gpui_perf_context(0, None),
        );
        output
    }

    fn mobile_drawer_header(
        &self,
        id: &'static str,
        left: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        div()
            .h(px(40.))
            .flex_none()
            .flex()
            .items_center()
            .justify_end()
            .px_2()
            .border_b_1()
            .border_color(rgb(palette.border))
            .bg(self.shell_surface_color(palette.surface))
            .child(
                div()
                    .id(id)
                    .size(px(26.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_sm()
                    .text_color(rgb(palette.text_muted))
                    .cursor_pointer()
                    .hover(|this| this.bg(rgb(palette.hover)).text_color(rgb(palette.text)))
                    .child(
                        svg()
                            .size(px(16.))
                            .path("icons/window/close.svg")
                            .text_color(rgb(palette.text_muted)),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if left {
                            this.shell.close_mobile_panel(PanelSide::Left);
                        } else {
                            this.shell.close_mobile_panel(PanelSide::Right);
                        }
                        cx.notify();
                    })),
            )
    }

    fn overlay_host(
        &mut self,
        content: Stateful<Div>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let terminal_overlays = self.terminal.overlay_visibility();
        let overlay = OverlayFlags {
            tab_actions_open: self.session.dialog_tab_actions_session_id().is_some(),
            color_picker_open: self.session.dialog_color_picker_is_open(),
            session_info_open: self.session.dialog_session_info_is_open(),
            multi_line_paste_open: terminal_overlays.paste_review,
            terminal_actions_open: terminal_overlays.actions,
            action_link_menu_open: terminal_overlays.action_link_menu,
            action_link_tooltip_open: terminal_overlays.action_link_tooltip,
            command_suggestions_open: self.terminal.command_suggestions_open(),
            credential_suggestions_open: self.terminal.credential_suggestions_open(),
            locked: self.security.screen_locked(),
        };
        let quick_switch_open = self.quick_switch_open(cx);
        let transfer_editor_open = self.transfer.editor_inline_overlay_is_open();
        let transfer_external_sync_open = self.active_external_editor_sync_prompt().is_some();
        let ssh_auth_prompt_open = self.session.prompt_has_active_ssh_auth();
        let full_access_confirmation = self.pending_ai_full_access();
        let mcp_approval = self.mcp_pending_approval_requests().into_iter().next();

        content
            .when(overlay.tab_actions_open, |this| {
                this.child(full_window_overlay_layer(
                    "tab-actions-input-layer",
                    self.tab_actions_overlay(cx),
                ))
            })
            .when(overlay.color_picker_open, |this| {
                this.child(full_window_overlay_layer(
                    "tab-color-input-layer",
                    self.tab_color_picker_overlay(cx),
                ))
            })
            .when(overlay.session_info_open, |this| {
                this.child(full_window_overlay_layer(
                    "session-info-input-layer",
                    self.session_info_overlay(cx),
                ))
            })
            .when(self.transfer.transfer_job_menu().is_some(), |this| {
                this.child(full_window_overlay_layer(
                    "transfer-job-menu-input-layer",
                    self.transfer_job_menu_overlay(cx),
                ))
            })
            .when(transfer_editor_open, |this| {
                this.child(full_window_overlay_layer(
                    "transfer-editor-input-layer",
                    self.transfer_editor_overlay(cx),
                ))
            })
            .when(transfer_external_sync_open, |this| {
                this.child(full_window_overlay_layer(
                    "transfer-external-sync-input-layer",
                    self.transfer_external_sync_prompt_overlay(cx),
                ))
            })
            .when(
                self.transfer.browser_view().favorites_menu.is_some(),
                |this| {
                    this.child(full_window_overlay_layer(
                        "transfer-favorites-menu-input-layer",
                        self.transfer_browser_favorites_menu_overlay(cx),
                    ))
                },
            )
            .when(self.transfer.browser_view().path_menu.is_some(), |this| {
                this.child(full_window_overlay_layer(
                    "transfer-path-menu-input-layer",
                    self.transfer_browser_path_menu_overlay(cx),
                ))
            })
            .when(self.transfer.browser_view().upload_menu.is_some(), |this| {
                this.child(full_window_overlay_layer(
                    "transfer-upload-menu-input-layer",
                    self.transfer_browser_upload_menu_overlay(cx),
                ))
            })
            .when(overlay.multi_line_paste_open, |this| {
                this.child(full_window_overlay_layer(
                    "multi-line-paste-input-layer",
                    self.multi_line_paste_overlay(cx),
                ))
            })
            .when(overlay.terminal_actions_open, |this| {
                this.child(full_window_overlay_layer(
                    "terminal-actions-input-layer",
                    self.terminal_actions_overlay(cx),
                ))
            })
            .when(overlay.action_link_menu_open, |this| {
                this.child(full_window_overlay_layer(
                    "action-link-menu-input-layer",
                    self.action_link_menu_overlay(cx),
                ))
            })
            .when(
                overlay.action_link_tooltip_open
                    && !overlay.action_link_menu_open
                    && !self.translation.dialog_is_open(),
                |this| this.child(passive_overlay_layer(self.action_link_tooltip_overlay(cx))),
            )
            .when(overlay.command_suggestions_open, |this| {
                this.child(passive_overlay_layer(self.command_suggestions_overlay(cx)))
            })
            .when(overlay.credential_suggestions_open, |this| {
                this.child(passive_overlay_layer(
                    self.credential_suggestions_overlay(cx),
                ))
            })
            .when(self.sync_input.is_open(), |this| {
                this.child(full_window_overlay_layer(
                    "sync-groups-input-layer",
                    self.sync_groups_overlay(cx),
                ))
            })
            .when_some(
                self.connection_state.inline_editor_panel_draft(),
                |this, editor| {
                    this.child(full_window_overlay_layer(
                        "connection-editor-input-layer",
                        self.connection_editor_panel(editor, cx),
                    ))
                },
            )
            .when(self.commands.quick_editor_is_inline(), |this| {
                this.child(full_window_overlay_layer(
                    "quick-command-editor-input-layer",
                    self.quick_command_editor_overlay(cx),
                ))
            })
            .when(self.commands.quick_details().is_some(), |this| {
                this.child(full_window_overlay_layer(
                    "quick-command-details-input-layer",
                    self.quick_command_details_overlay(cx),
                ))
            })
            .when(self.commands.quick_variable_prompt().is_some(), |this| {
                this.child(full_window_overlay_layer(
                    "quick-command-variable-input-layer",
                    self.quick_command_variable_prompt_overlay(cx),
                ))
            })
            .when(quick_switch_open, |this| {
                this.child(full_window_overlay_layer(
                    "quick-switch-input-layer",
                    self.quick_switch_overlay(cx),
                ))
            })
            .when(self.shell.activity_bar_context_menu().is_some(), |this| {
                this.child(full_window_overlay_layer(
                    "activity-context-input-layer",
                    self.activity_bar_context_menu_overlay(cx),
                ))
            })
            .when_some(full_access_confirmation, |this, setting| {
                this.child(full_window_overlay_layer(
                    "ai-full-access-confirmation-layer",
                    self.ai_full_access_confirmation_overlay(setting, cx),
                ))
            })
            .when_some(mcp_approval, |this, request| {
                this.child(full_window_overlay_layer(
                    "mcp-approval-input-layer",
                    self.mcp_approval_overlay(request, cx),
                ))
            })
            // Below the lock screen on purpose: both layers share
            // `APP_OVERLAY_PRIORITY`, so insertion order decides, and a layer
            // painted above the lock screen would swallow the pointer input the
            // lock screen needs.
            .when(
                modal_scrim_is_drawn() && self.modal_child_window_is_open_or_pending(),
                |this| this.child(passive_overlay_layer(self.modal_owner_scrim())),
            )
            .when(overlay.locked, |this| {
                this.child(full_window_overlay_layer(
                    "lock-screen-input-layer",
                    self.lock_screen_overlay(window, cx),
                ))
            })
            // Background SSH operations can request credentials while another
            // overlay is open, so authentication must be the topmost overlay.
            .when(ssh_auth_prompt_open, |this| {
                this.child(
                    deferred(full_window_overlay_layer(
                        "ssh-auth-prompt-input-layer",
                        self.ssh_auth_prompt_overlay(cx),
                    ))
                    .with_priority(SSH_AUTH_PROMPT_PRIORITY),
                )
            })
    }

    fn ai_full_access_confirmation_overlay(
        &mut self,
        setting: AiFullAccessSetting,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let target = match setting {
            AiFullAccessSetting::ExternalAgent => t!("ai.externalPermission"),
            AiFullAccessSetting::Codex => t!("ai.codex.title"),
            AiFullAccessSetting::ClaudeCode => t!("ai.claude.title"),
            AiFullAccessSetting::McpHost => t!("ai.mcp.title"),
        };
        div()
            .id("ai-full-access-confirmation")
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x00000080))
            .on_click(cx.listener(|this, _, _, cx| this.cancel_ai_full_access(cx)))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    this.cancel_ai_full_access(cx);
                    cx.stop_propagation();
                }
            }))
            .child(
                div()
                    .id("ai-full-access-confirmation-card")
                    .w(px(440.))
                    .p_5()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(palette.danger))
                    .bg(rgb(palette.surface))
                    .shadow_lg()
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .text_size(px(16.))
                            .text_color(rgb(palette.danger))
                            .child(t!("ai.fullAccess.confirmTitle")),
                    )
                    .child(format!(
                        "{}\n\n{}",
                        target,
                        t!("ai.fullAccess.confirmMessage")
                    ))
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap_2()
                            .child(overlay_button(
                                "ai-full-access-cancel",
                                t!("common.cancel"),
                                palette,
                                false,
                                cx.listener(|this, _, _, cx| this.cancel_ai_full_access(cx)),
                            ))
                            .child(overlay_button(
                                "ai-full-access-confirm",
                                t!("common.confirm"),
                                palette,
                                true,
                                cx.listener(|this, _, _, cx| this.confirm_ai_full_access(cx)),
                            )),
                    ),
            )
    }

    fn mcp_approval_overlay(
        &mut self,
        request: McpApprovalRequest,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let deny_id = request.request_id.clone();
        let once_id = request.request_id.clone();
        let session_id = request.request_id.clone();
        let backdrop_id = request.request_id.clone();
        let escape_id = request.request_id.clone();
        div()
            .id("mcp-approval-overlay")
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x00000080))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.respond_to_mcp_approval(&backdrop_id, McpApprovalDecision::Deny, cx)
            }))
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    this.respond_to_mcp_approval(&escape_id, McpApprovalDecision::Deny, cx);
                    cx.stop_propagation();
                }
            }))
            .child(
                div()
                    .id("mcp-approval-card")
                    .w(px(520.))
                    .p_5()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(if request.destructive {
                        palette.danger
                    } else {
                        palette.warning
                    }))
                    .bg(rgb(palette.surface))
                    .shadow_lg()
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .text_size(px(16.))
                            .text_color(rgb(if request.destructive {
                                palette.danger
                            } else {
                                palette.warning
                            }))
                            .child(t!("ai.mcp.approvalTitle")),
                    )
                    .child(mcp_detail_row(t!("ai.mcp.client"), request.client))
                    .child(mcp_detail_row(t!("ai.mcp.capability"), request.capability))
                    .when_some(request.target, |this, target| {
                        this.child(mcp_detail_row(t!("ai.mcp.target"), target))
                    })
                    .child(mcp_detail_row(
                        t!("ai.mcp.parameters"),
                        request.parameter_summary,
                    ))
                    .child(mcp_detail_row(t!("ai.mcp.risk"), request.risk_level))
                    .when(request.destructive, |this| {
                        this.child(
                            div()
                                .text_color(rgb(palette.danger))
                                .child(t!("ai.mcp.destructiveWarning")),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap_2()
                            .child(overlay_button(
                                "mcp-approval-deny",
                                t!("ai.mcp.deny"),
                                palette,
                                true,
                                cx.listener(move |this, _, _, cx| {
                                    this.respond_to_mcp_approval(
                                        &deny_id,
                                        McpApprovalDecision::Deny,
                                        cx,
                                    )
                                }),
                            ))
                            .child(overlay_button(
                                "mcp-approval-once",
                                t!("ai.mcp.allowOnce"),
                                palette,
                                false,
                                cx.listener(move |this, _, _, cx| {
                                    this.respond_to_mcp_approval(
                                        &once_id,
                                        McpApprovalDecision::AllowOnce,
                                        cx,
                                    )
                                }),
                            ))
                            .when(!request.destructive, |this| {
                                this.child(overlay_button(
                                    "mcp-approval-session",
                                    t!("ai.mcp.allowSession"),
                                    palette,
                                    false,
                                    cx.listener(move |this, _, _, cx| {
                                        this.respond_to_mcp_approval(
                                            &session_id,
                                            McpApprovalDecision::AllowSession,
                                            cx,
                                        )
                                    }),
                                ))
                            }),
                    ),
            )
    }

    fn reconcile_root_pointer_interactions(
        &mut self,
        event: &MouseMoveEvent,
        cx: &mut Context<Self>,
    ) {
        self.recover_terminal_mouse_report_after_lost_mouse_up(event, cx);
        if event.dragging() {
            return;
        }
        self.finish_transfer_browser_column_resize(cx);
        self.finish_panel_resize(cx);
        self.finish_transfer_height_resize(cx);
        self.finish_bottom_panel_resize(cx);
        self.finish_panel_stack_resize(cx);
        self.finish_workspace_split_resize(cx);
        self.recover_terminal_selection_after_lost_mouse_up(cx);
        self.finish_terminal_scrollbar_drag(cx);
        self.clear_terminal_window_drop(cx);
        self.clear_session_tab_drag(cx);
    }

    /// Whether the main window should be held back for a modal child window.
    ///
    /// The three windows below hold one shared draft slot each, so the main
    /// window must not be edited underneath them. Deliberately absent: the remote
    /// file editor, which is an independent document window, and the
    /// external-editor prompt, which is a topmost `PopUp` that must not block the
    /// workspace it is reporting on.
    ///
    /// The pending state counts: a child window is opened from a deferred
    /// callback, and without it the main window would be live for the frames in
    /// between.
    fn modal_child_window_is_open_or_pending(&self) -> bool {
        self.shell.settings_window_is_open_or_pending()
            || self.connection_state.editor_window_is_open_or_pending()
            || self.commands.quick_editor_window_is_open_or_pending()
    }

    /// Dims the main window while a modal child window owns a draft.
    ///
    /// Deliberately inert: the child window is a `WindowKind::Dialog`, so the
    /// platform has already stopped input from reaching this window, and a scrim
    /// that handled clicks would be claiming a job it never gets asked to do.
    fn modal_owner_scrim(&self) -> impl IntoElement {
        div()
            .id("modal-owner-scrim")
            .absolute()
            .inset_0()
            .bg(rgba(0x00000066))
    }

    fn ssh_auth_prompt_overlay(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let host_key_prompt = self.session.prompt_active_host_key().cloned();
        let agent_prompt = self.session.prompt_active_agent().cloned();
        let credential_prompt = self.session.prompt_active_credential().cloned();
        let keyboard_interactive_prompt =
            self.session.prompt_active_keyboard_interactive().cloned();
        let dialog_width = if keyboard_interactive_prompt.is_some() {
            384.
        } else {
            416.
        };
        div()
            .id(SharedString::from("ssh-auth-prompt-overlay"))
            .absolute()
            .top_0()
            .bottom_0()
            .left_0()
            .right_0()
            .bg(rgba(0x00000080))
            .flex()
            .items_center()
            .justify_center()
            .p_3()
            .track_focus(self.session.prompt_credential_focus())
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Middle, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(|this, _, window, cx| {
                cx.stop_propagation();
                if !this.focus_active_ssh_prompt_input(window, cx) {
                    window.focus(this.session.prompt_credential_focus(), cx);
                }
                cx.notify();
            }))
            .child(
                div()
                    .w(px(dialog_width))
                    .max_w_full()
                    .when_some(host_key_prompt, |this, prompt| {
                        this.child(self.host_key_prompt_banner(prompt, cx))
                    })
                    .when_some(agent_prompt, |this, prompt| {
                        this.child(self.agent_prompt_banner(prompt, cx))
                    })
                    .when_some(credential_prompt, |this, prompt| {
                        this.child(self.credential_prompt_banner(prompt, cx))
                    })
                    .when_some(keyboard_interactive_prompt, |this, prompt| {
                        this.child(self.keyboard_interactive_prompt_banner(prompt, cx))
                    }),
            )
    }
}

fn wallpaper_tile_grid(viewport: (f32, f32), tile: (f32, f32)) -> (usize, usize) {
    let columns = ((viewport.0.max(1.) / tile.0.max(WALLPAPER_TILE_MIN_SIZE)).ceil() as usize)
        .clamp(1, WALLPAPER_TILE_ELEMENT_LIMIT);
    let requested_rows =
        ((viewport.1.max(1.) / tile.1.max(WALLPAPER_TILE_MIN_SIZE)).ceil() as usize).max(1);
    let rows = requested_rows.min((WALLPAPER_TILE_ELEMENT_LIMIT / columns).max(1));
    (columns, rows)
}

fn fit_wallpaper_tile_size(viewport: (f32, f32), intrinsic: (f32, f32)) -> (f32, f32) {
    let mut tile = (
        intrinsic.0.max(WALLPAPER_TILE_MIN_SIZE),
        intrinsic.1.max(WALLPAPER_TILE_MIN_SIZE),
    );
    for _ in 0..4 {
        let columns = (viewport.0.max(1.) / tile.0).ceil().max(1.);
        let rows = (viewport.1.max(1.) / tile.1).ceil().max(1.);
        let count = columns * rows;
        if count <= WALLPAPER_TILE_ELEMENT_LIMIT as f32 {
            break;
        }
        let scale = (count / WALLPAPER_TILE_ELEMENT_LIMIT as f32).sqrt() * 1.01;
        tile.0 *= scale;
        tile.1 *= scale;
    }
    tile
}

impl Render for NyaTermApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let render_started_at = Instant::now();
        // Reconcile viewport size and cell metrics here rather than on the runtime
        // tick. `render` already holds the window, and the chrome built below reads
        // `shell.viewport_size()`, so this paint sees the fresh values instead of
        // whatever the last tick recorded. Nothing here notifies *this* entity, so it
        // cannot loop: surfaces are separate entities.
        self.refresh_window_render_inputs(window, cx);
        // Safety net for the idle screen-lock deadline. Every unlock and every
        // settings change arms the clock directly; this catches a path added later
        // that forgets to, because a clock that failed to arm means the screen never
        // locks. Costs one bool compare when it is already running, and arming
        // redundantly is harmless -- the clock just re-checks and defers.
        self.ensure_idle_lock_clock(cx);
        // Each polling panel owns its own refresh clock; this starts the ones that are
        // wanted and stops the rest. `render` because every input it reads -- which
        // panels are on screen, what the header is showing, which panels settings
        // enable, whether an SSH session is active -- changes alongside a repaint, and
        // no single event covers all four.
        self.sync_remote_panel_demand(cx);
        FULL_SHELL_PAINT_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let full_shell_paint_count = self.shell.note_full_shell_paint();
        let root_started_at = Instant::now();
        let content = self.root_chrome(window, cx);
        let root_duration = root_started_at.elapsed();
        let overlay_started_at = Instant::now();
        let output = self.overlay_host(content, window, cx);
        let overlay_duration = overlay_started_at.elapsed();
        let render_duration = render_started_at.elapsed();
        record_gpui_perf_sample(
            "root_chrome",
            root_duration,
            self.gpui_perf_context(0, None),
        );
        record_gpui_perf_sample(
            "root_render",
            render_duration,
            self.gpui_perf_context(0, None),
        );
        if render_duration >= Duration::from_millis(12)
            && self.should_log_slow_diagnostic("root_render", Instant::now())
        {
            tracing::warn!(
                diagnostic = "root_render",
                total_ms = render_duration.as_millis(),
                root_chrome_ms = root_duration.as_millis(),
                overlay_host_ms = overlay_duration.as_millis(),
                active_session_id = self.session.active_id().unwrap_or(""),
                visible_session_count = self.visible_terminal_session_ids().len(),
                connect_settle_active = self.shell.connect_settle_active(Instant::now()),
                output_pressure = self.runtime_output_pressure_active(),
                full_shell_paint_count,
                surface_paint_count = terminal_surface_paint_count(),
                "slow root render"
            );
        }
        output
    }
}

fn overlay_button(
    id: &'static str,
    label: impl Into<SharedString>,
    palette: ThemePalette,
    danger: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .px_3()
        .py_2()
        .rounded_md()
        .border_1()
        .border_color(rgb(if danger {
            palette.danger
        } else {
            palette.border
        }))
        .bg(rgb(if danger {
            palette.danger
        } else {
            palette.surface_elevated
        }))
        .text_color(rgb(if danger { 0xffffff } else { palette.text }))
        .cursor_pointer()
        .hover(|this| this.opacity(0.85))
        .on_click(on_click)
        .child(label.into())
}

fn mcp_detail_row(
    label: impl Into<SharedString>,
    value: impl Into<SharedString>,
) -> impl IntoElement {
    div()
        .flex()
        .gap_2()
        .child(
            div()
                .w(px(112.))
                .text_color(rgba(0xffffff88))
                .child(label.into()),
        )
        .child(div().flex_1().min_w_0().child(value.into()))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    use gpui::{
        AppContext as _, IntoElement, ParentElement as _, Render, Styled as _, TestAppContext,
        VisualTestContext, div,
    };
    use nyaterm_core::{AiExecutionProfile, AppRuntime, RuntimeMode, uuid};
    use nyaterm_transport::LocalSessionConfig;

    use crate::entities::{OverlayStore, StartupRestoreStore, UiStoreHandles};
    use crate::models::{SessionLaunchConfig, SessionRuntimeMetadata, TabDockEdge, TabDockZone};

    use super::{fit_wallpaper_tile_size, wallpaper_tile_grid};

    #[test]
    fn wallpaper_tiles_cover_viewport_and_cap_extreme_counts() {
        assert_eq!(wallpaper_tile_grid((1280., 800.), (256., 256.)), (5, 4));
        assert_eq!(wallpaper_tile_grid((1280., 800.), (2048., 2048.)), (1, 1));
        let (columns, rows) = wallpaper_tile_grid((100_000., 100_000.), (1., 1.));
        assert!(columns * rows <= 8192);
        let tile = fit_wallpaper_tile_size((3840., 2160.), (1., 1.));
        let (columns, rows) = wallpaper_tile_grid((3840., 2160.), tile);
        assert!(columns * rows <= 8192);
        assert!(columns as f32 * tile.0 >= 3840.);
        assert!(rows as f32 * tile.1 >= 2160.);
    }

    /// A bare app entity, enough for the pure state predicates.
    ///
    /// The temp directory is keyed by pid *and* a uuid: a clock-derived name lets
    /// two parallel fixtures share one settings database on Windows.
    fn modal_predicate_app(cx: &mut TestAppContext) -> gpui::Entity<super::NyaTermApp> {
        let root = std::env::temp_dir().join(format!(
            "nyaterm-modal-predicate-{}-{}",
            std::process::id(),
            uuid()
        ));
        let runtime = AppRuntime::from_parts_for_test(
            RuntimeMode::Portable,
            root.clone(),
            root.join("config"),
            root.join("logs"),
            root.join("cache"),
            None,
        );
        let stores = UiStoreHandles {
            startup_restore: cx.new(|_| StartupRestoreStore::default()),
            overlays: cx.new(|_| OverlayStore::default()),
        };
        cx.new(|cx| super::NyaTermApp::new(runtime, stores, cx))
    }

    /// The three windows that hold a draft shared with the main window each hold
    /// it back, and each does so from the moment the open is requested -- a child
    /// window is opened from a deferred callback, so the pending state is the only
    /// thing covering the frames in between.
    #[gpui::test]
    fn every_draft_owning_child_window_holds_the_main_window_back(cx: &mut TestAppContext) {
        let app = modal_predicate_app(cx);

        cx.update_entity(&app, |app, _| {
            assert!(
                !app.modal_child_window_is_open_or_pending(),
                "nothing is open yet"
            );

            assert!(app.shell.begin_settings_window_open());
            assert!(app.modal_child_window_is_open_or_pending());
            app.shell.cancel_settings_window_open();
            assert!(!app.modal_child_window_is_open_or_pending());

            assert!(app.connection_state.begin_editor_window_open());
            assert!(app.modal_child_window_is_open_or_pending());
            app.connection_state.clear_editor_window_pending();
            assert!(!app.modal_child_window_is_open_or_pending());

            app.commands
                .open_quick_editor(crate::models::QuickCommandEditorState::blank());
            assert!(app.commands.request_quick_editor_window());
            assert!(app.modal_child_window_is_open_or_pending());
            app.commands.cancel_quick_editor_window_request();
            assert!(!app.modal_child_window_is_open_or_pending());
        });
    }

    fn root_render_benchmark_dir() -> PathBuf {
        std::env::temp_dir().join(format!(
            "nyaterm-root-render-benchmark-{}-{}",
            std::process::id(),
            uuid()
        ))
    }

    fn root_render_benchmark_session(name: String) -> SessionRuntimeMetadata {
        SessionRuntimeMetadata {
            ssh_config: None,
            ssh_multiplex_key: None,
            source_connection_id: None,
            ai_execution_profile: AiExecutionProfile::Posix,
            launch_config: SessionLaunchConfig::Local(LocalSessionConfig {
                name,
                ..LocalSessionConfig::default()
            }),
            disconnected: false,
        }
    }

    struct RootRenderBenchmarkFixture {
        app: gpui::Entity<super::NyaTermApp>,
    }

    impl Render for RootRenderBenchmarkFixture {
        fn render(
            &mut self,
            _window: &mut gpui::Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            div().size_full().child(self.app.clone())
        }
    }

    fn forced_root_draw(
        app: &gpui::Entity<super::NyaTermApp>,
        cx: &mut VisualTestContext,
    ) -> Duration {
        cx.run_until_parked();
        cx.update(|window, cx| {
            app.update(cx, |_, cx| cx.notify());
            let started_at = Instant::now();
            _ = window.draw(cx);
            started_at.elapsed()
        })
    }

    fn run_root_render_benchmark(cx: &mut TestAppContext) {
        let root = root_render_benchmark_dir();
        let runtime = AppRuntime::from_parts_for_test(
            RuntimeMode::Portable,
            root.clone(),
            root.join("config"),
            root.join("logs"),
            root.join("cache"),
            None,
        );
        let stores = UiStoreHandles {
            startup_restore: cx.new(|_| StartupRestoreStore::default()),
            overlays: cx.new(|_| OverlayStore::default()),
        };
        let app = cx.new(|cx| super::NyaTermApp::new(runtime, stores, cx));
        cx.update_entity(&app, |app, cx| {
            app.sync_component_theme(cx);
            let session_ids = (0..100)
                .map(|index| format!("benchmark-session-{index:03}"))
                .collect::<Vec<_>>();
            for (index, session_id) in session_ids.iter().enumerate() {
                app.session.register_session_metadata(
                    session_id,
                    root_render_benchmark_session(format!("Local session {index:03}")),
                );
                app.terminal
                    .seed_session_view(session_id.clone(), String::new(), "UTF-8");
            }
            let active_session_id = session_ids[0].clone();
            app.session.select_active_session(active_session_id.clone());
            app.shell.show_workspace();
            let target_leaf_id = app
                .terminal
                .ensure_terminal_windows_root(session_ids.clone(), Some(active_session_id))
                .expect("benchmark terminal window root");
            let mut leaf_ids = vec![target_leaf_id.clone()];
            for session_id in session_ids.iter().skip(1).take(7) {
                let result = app.terminal.dock_tab_on_terminal_window_leaf(
                    session_id,
                    &target_leaf_id,
                    TabDockZone::Edge(TabDockEdge::Right),
                );
                let crate::features::terminal::TerminalWindowDockResult::Docked {
                    focused_leaf_id: Some(leaf_id),
                } = result
                else {
                    panic!("benchmark session should create a terminal leaf");
                };
                leaf_ids.push(leaf_id);
            }
            for (index, session_id) in session_ids.iter().skip(8).enumerate() {
                let leaf_id = &leaf_ids[index % leaf_ids.len()];
                if leaf_id == &target_leaf_id {
                    continue;
                }
                let result = app.terminal.dock_tab_on_terminal_window_leaf(
                    session_id,
                    leaf_id,
                    TabDockZone::Center,
                );
                assert!(matches!(
                    result,
                    crate::features::terminal::TerminalWindowDockResult::Docked { .. }
                ));
            }
        });
        let fixture_app = app.clone();
        let (_, cx) =
            cx.add_window_view(move |_, _| RootRenderBenchmarkFixture { app: fixture_app });
        let cx: &mut VisualTestContext = cx;

        for _ in 0..12 {
            _ = forced_root_draw(&app, cx);
        }
        let mut samples = (0..120)
            .map(|_| forced_root_draw(&app, cx))
            .collect::<Vec<_>>();
        samples.sort_unstable();
        let total = samples.iter().copied().sum::<Duration>();
        let average = total / samples.len() as u32;
        let p95_index = ((samples.len() * 95).div_ceil(100)).saturating_sub(1);
        let p95 = samples[p95_index];
        let max = *samples.last().expect("benchmark samples");

        eprintln!(
            "root render benchmark: sessions=100 leaves=8 samples=120 average={average:?} p95={p95:?} max={max:?}"
        );
    }

    #[test]
    #[ignore = "performance benchmark; run manually with --ignored --nocapture"]
    fn root_render_hundred_sessions_eight_terminal_leaves_benchmark() {
        std::thread::Builder::new()
            .name("nyaterm-root-render-benchmark".to_string())
            .stack_size(16 * 1024 * 1024)
            .spawn(|| {
                let mut cx = TestAppContext::single();
                run_root_render_benchmark(&mut cx);
            })
            .expect("spawn root render benchmark thread")
            .join()
            .expect("root render benchmark thread");
    }
}
