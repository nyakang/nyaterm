use gpui::{
    Context, CursorStyle, DragMoveEvent, FontWeight, IntoElement, Point, Render, SharedString,
    Window, div, prelude::*, px, rgb, rgba,
};
use nyaterm_ui::icons::{connection_type_icon, mono_icon, resolve_connection_icon};
use nyaterm_ui::{NyaButton, NyaButtonVariant, NyaIconButton, NyaSwitch, NyaTooltip, ThemePalette};
use rust_i18n::t;

use crate::features::pages::settings::panel::SettingsPanel;
use crate::features::pages::settings::settings_form_section_with_action;
use crate::models::ConnectionKindTab;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::features::pages::settings) struct ConnectionTypeDropTarget {
    pub source: ConnectionKindTab,
    pub target: ConnectionKindTab,
    pub after: bool,
}

#[derive(Clone)]
struct ConnectionTypeDragPayload {
    kind: ConnectionKindTab,
    label: SharedString,
    palette: ThemePalette,
}

struct ConnectionTypeDragPreview {
    payload: ConnectionTypeDragPayload,
    position: Point<gpui::Pixels>,
}

impl Render for ConnectionTypeDragPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let palette = self.payload.palette;
        let icon_def = resolve_connection_icon(None, self.payload.kind.label());
        div()
            .absolute()
            .left(self.position.x - px(24.))
            .top(self.position.y - px(17.))
            .h(px(34.))
            .px_3()
            .flex()
            .items_center()
            .gap_2()
            .rounded_md()
            .border_1()
            .border_color(rgb(palette.primary))
            .bg(rgba((palette.surface_elevated << 8) | 0xf2))
            .shadow_lg()
            .child(mono_icon(
                "icons/drag.svg",
                rgb(palette.text_muted).into(),
                13.,
            ))
            .child(connection_type_icon(palette, icon_def, false, 15.))
            .child(
                div()
                    .text_size(px(13.))
                    .font_weight(FontWeight(600.))
                    .text_color(rgb(palette.text))
                    .whitespace_nowrap()
                    .child(self.payload.label.clone()),
            )
    }
}

/// Computes the post-removal destination index for dropping `from_index` before or after
/// `target_index`, returning `None` when the drop leaves the order unchanged.
fn drop_target_index(from_index: usize, target_index: usize, after: bool) -> Option<usize> {
    let slot = if after {
        target_index + 1
    } else {
        target_index
    };
    if slot == from_index || slot == from_index + 1 {
        return None;
    }
    Some(if from_index < slot { slot - 1 } else { slot })
}

impl SettingsPanel {
    fn ensure_connection_type_drop_clock(&mut self, cx: &mut Context<Self>) {
        if self.connection_type_drop_clock_armed || self.connection_type_drop_target.is_none() {
            return;
        }
        self.connection_type_drop_clock_armed = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(50))
                    .await;
                let Ok(keep_running) = this.update(cx, |this, cx| {
                    if !cx.has_active_drag() {
                        if this.connection_type_drop_target.take().is_some() {
                            cx.notify();
                        }
                        this.connection_type_drop_clock_armed = false;
                        return false;
                    }
                    if this.connection_type_drop_target.is_none() {
                        this.connection_type_drop_clock_armed = false;
                        return false;
                    }
                    true
                }) else {
                    break;
                };
                if !keep_running {
                    break;
                }
            }
        })
        .detach();
    }

    pub(super) fn connection_types_settings_section(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        if !cx.has_active_drag() {
            self.connection_type_drop_target = None;
        }
        let active_drop = self.connection_type_drop_target;
        let palette = self.theme_palette();
        let layout = &self.settings.summary().connection_type_layout;
        let kinds = ConnectionKindTab::editor_types(layout, true, &[]);
        let catalog = ConnectionKindTab::catalog_ids();
        let visible = layout.visible(&catalog);
        let default_visible_id = visible.first().copied();
        let is_default = layout.is_default_for(&catalog);

        let default_badge_label = SharedString::from(t!("settings.connectionTypesDefaultBadge"));
        let keep_one_hint = SharedString::from(t!("settings.connectionTypesKeepOneHint"));
        let move_up_tooltip = SharedString::from(t!("settings.startupMoveUp"));
        let move_down_tooltip = SharedString::from(t!("settings.startupMoveDown"));
        let drag_hint = SharedString::from(t!("credentialManager.dragToSort"));

        let reset_action = div()
            .debug_selector(|| "connection-types-reset".to_string())
            .child(
                NyaButton::new(
                    "connection-types-reset",
                    t!("settings.connectionTypesReset"),
                )
                .form_control()
                .variant(NyaButtonVariant::Ghost)
                .disabled(is_default)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.with_app(cx, |app, cx| app.reset_connection_type_layout(cx));
                })),
            )
            .into_any_element();

        let mut list = div()
            .debug_selector(|| "connection-types-list".to_string())
            .rounded_md()
            .border_1()
            .border_color(rgba((palette.border << 8) | 0x99))
            .bg(rgba((palette.bg << 8) | 0x73))
            .overflow_hidden()
            .flex()
            .flex_col();

        let total = kinds.len();
        for (index, kind) in kinds.iter().copied().enumerate() {
            let id_str = kind.persistence_id();
            let checked = visible.contains(&id_str);
            let enabled = !checked || visible.len() > 1;
            let is_default_kind = checked && default_visible_id == Some(id_str);
            let label = SharedString::from(kind.display_label());
            let row_group = SharedString::from(format!("connection-type-row-{id_str}"));
            let icon_def = resolve_connection_icon(None, kind.label());

            let indicator_after = active_drop.and_then(|drop| {
                if drop.target != kind {
                    return None;
                }
                let from_index = kinds
                    .iter()
                    .position(|candidate| *candidate == drop.source)?;
                drop_target_index(from_index, index, drop.after).map(|_| drop.after)
            });

            let drag_payload = ConnectionTypeDragPayload {
                kind,
                label: label.clone(),
                palette,
            };
            let drag_tooltip = drag_hint.clone();
            let drag_handle = div()
                .id(SharedString::from(format!("connection-type-drag-{id_str}")))
                .debug_selector(move || format!("connection-type-drag-{id_str}"))
                .min_w_0()
                .flex_1()
                .h_full()
                .py_1p5()
                .flex()
                .items_center()
                .gap_2p5()
                .cursor_grab()
                .tooltip(move |window, cx| NyaTooltip::new(drag_tooltip.clone()).build(window, cx))
                .on_drag(drag_payload, |payload, position, window, cx| {
                    window.defer(cx, |window, cx| {
                        cx.set_active_drag_cursor_style(CursorStyle::ClosedHand, window);
                    });
                    cx.new(|_| ConnectionTypeDragPreview {
                        payload: payload.clone(),
                        position,
                    })
                })
                .child(
                    div()
                        .size(px(16.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .opacity(0.55)
                        .group_hover(row_group.clone(), |style| style.opacity(1.))
                        .child(mono_icon(
                            "icons/drag.svg",
                            rgb(palette.text_muted).into(),
                            14.,
                        )),
                )
                .child(
                    div()
                        .size(px(18.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .when(!checked, |this| this.opacity(0.42))
                        .child(connection_type_icon(palette, icon_def, false, 16.)),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(px(13.))
                        .font_weight(FontWeight(500.))
                        .text_color(if checked {
                            rgb(palette.text)
                        } else {
                            rgb(palette.text_dimmed)
                        })
                        .child(label),
                )
                .when(is_default_kind, |this| {
                    this.child(
                        div()
                            .debug_selector(move || format!("connection-type-default-{id_str}"))
                            .flex_none()
                            .rounded(px(4.))
                            .border_1()
                            .border_color(rgba((palette.primary << 8) | 0x55))
                            .bg(rgba((palette.primary << 8) | 0x1f))
                            .px(px(6.))
                            .py(px(1.))
                            .text_size(px(11.))
                            .font_weight(FontWeight(500.))
                            .text_color(rgb(palette.link))
                            .child(default_badge_label.clone()),
                    )
                });

            let mut visibility_switch = NyaSwitch::new(format!("connection-type-visible-{id_str}"))
                .checked(checked)
                .disabled(!enabled)
                .on_click(cx.listener(move |this, _, _, cx| {
                    if enabled {
                        this.with_app(cx, |app, cx| {
                            app.set_connection_type_visible(kind, !checked, cx)
                        });
                    }
                }));
            if !enabled {
                visibility_switch = visibility_switch.tooltip(keep_one_hint.clone());
            }

            let controls = div()
                .flex_none()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_0p5()
                        .opacity(0.55)
                        .group_hover(row_group.clone(), |style| style.opacity(1.))
                        .child(
                            div()
                                .debug_selector(move || format!("connection-type-up-{id_str}"))
                                .child(
                                    NyaIconButton::new(
                                        format!("connection-type-up-{id_str}"),
                                        "icons/chevron-up.svg",
                                    )
                                    .size(px(24.))
                                    .icon_size(px(14.))
                                    .tooltip(move_up_tooltip.clone())
                                    .disabled(index == 0)
                                    .on_click(cx.listener(
                                        move |this, _, _, cx| {
                                            this.with_app(cx, |app, cx| {
                                                app.move_connection_type(kind, false, cx)
                                            });
                                        },
                                    )),
                                ),
                        )
                        .child(
                            div()
                                .debug_selector(move || format!("connection-type-down-{id_str}"))
                                .child(
                                    NyaIconButton::new(
                                        format!("connection-type-down-{id_str}"),
                                        "icons/chevron-down.svg",
                                    )
                                    .size(px(24.))
                                    .icon_size(px(14.))
                                    .tooltip(move_down_tooltip.clone())
                                    .disabled(index + 1 == total)
                                    .on_click(cx.listener(
                                        move |this, _, _, cx| {
                                            this.with_app(cx, |app, cx| {
                                                app.move_connection_type(kind, true, cx)
                                            });
                                        },
                                    )),
                                ),
                        ),
                )
                .child(
                    div()
                        .debug_selector(move || format!("connection-type-visible-{id_str}"))
                        .flex()
                        .items_center()
                        .child(visibility_switch),
                );

            let kinds_for_drop = kinds.clone();
            let row = div()
                .id(SharedString::from(format!("connection-type-row-{id_str}")))
                .debug_selector(move || format!("connection-type-row-{id_str}"))
                .group(row_group)
                .relative()
                .min_h(px(36.))
                .px_3()
                .flex()
                .items_center()
                .justify_between()
                .gap_3()
                .when(index + 1 < total, |this| {
                    this.border_b_1()
                        .border_color(rgba((palette.border << 8) | 0x66))
                })
                .hover(move |this| this.bg(rgba((palette.hover << 8) | 0xbf)))
                .drag_over::<ConnectionTypeDragPayload>(move |this, payload, _, _| {
                    if payload.kind == kind {
                        this
                    } else {
                        this.bg(rgba((palette.primary << 8) | 0x12))
                    }
                })
                .on_drag_move(cx.listener(
                    move |this, event: &DragMoveEvent<ConnectionTypeDragPayload>, _, cx| {
                        let source = event.drag(cx).kind;
                        if source == kind || !event.bounds.contains(&event.event.position) {
                            if this
                                .connection_type_drop_target
                                .is_some_and(|drop| drop.target == kind)
                            {
                                this.connection_type_drop_target = None;
                                cx.notify();
                            }
                            return;
                        }
                        let after = event.event.position.y
                            >= event.bounds.origin.y + event.bounds.size.height / 2.;
                        let next = Some(ConnectionTypeDropTarget {
                            source,
                            target: kind,
                            after,
                        });
                        if this.connection_type_drop_target != next {
                            this.connection_type_drop_target = next;
                            this.ensure_connection_type_drop_clock(cx);
                            cx.notify();
                        }
                    },
                ))
                .on_drop(
                    cx.listener(move |this, payload: &ConnectionTypeDragPayload, _, cx| {
                        let after = this
                            .connection_type_drop_target
                            .filter(|drop| drop.target == kind && drop.source == payload.kind)
                            .map(|drop| drop.after)
                            .unwrap_or(false);
                        this.connection_type_drop_target = None;
                        let Some(from_index) = kinds_for_drop
                            .iter()
                            .position(|candidate| *candidate == payload.kind)
                        else {
                            cx.notify();
                            return;
                        };
                        if let Some(destination) = drop_target_index(from_index, index, after) {
                            this.with_app(cx, |app, cx| {
                                app.move_connection_type_to(payload.kind, destination, cx);
                            });
                        } else {
                            cx.notify();
                        }
                    }),
                )
                .child(drag_handle)
                .child(controls)
                .when_some(indicator_after, |this, after| {
                    this.child(
                        div()
                            .debug_selector(move || {
                                format!(
                                    "connection-type-drop-indicator-{id_str}-{}",
                                    if after { "after" } else { "before" }
                                )
                            })
                            .absolute()
                            .left_2()
                            .right_2()
                            .h(px(2.))
                            .rounded_full()
                            .bg(rgb(palette.primary))
                            .when(!after, |line| line.top_0())
                            .when(after, |line| line.bottom_0()),
                    )
                });

            list = list.child(row);
        }

        settings_form_section_with_action(
            palette,
            Some(t!("settings.connectionTypes")),
            Some(t!("settings.connectionTypesDescription")),
            Some(reset_action),
            list,
        )
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use gpui::{
        AppContext as _, Bounds, Context, Entity, IntoElement, Modifiers, MouseButton,
        ParentElement as _, Render, Styled as _, TestAppContext, VisualTestContext, Window, div,
        point, px,
    };
    use nyaterm_core::{AppRuntime, RuntimeMode};

    use super::drop_target_index;
    use crate::entities::{OverlayStore, StartupRestoreStore, UiStoreHandles};
    use crate::features::NyaTermApp;
    use crate::features::pages::settings::panel::SettingsPanel;
    use crate::models::{ConnectionKindTab, NavItem, SettingsTab};
    use crate::test_support::{TestAppCleanup, TestConfigDir};

    #[test]
    fn drop_target_index_skips_no_op_slots_and_handles_both_directions() {
        // Dropping on self (before or after) or into an adjacent boundary is a no-op.
        assert_eq!(drop_target_index(0, 0, false), None);
        assert_eq!(drop_target_index(0, 0, true), None);
        assert_eq!(drop_target_index(0, 1, false), None);
        assert_eq!(drop_target_index(2, 1, true), None);

        // Dragging downward shifts intermediate items left by 1.
        assert_eq!(drop_target_index(0, 1, true), Some(1));
        assert_eq!(drop_target_index(0, 8, false), Some(7));
        assert_eq!(drop_target_index(0, 8, true), Some(8));

        // Dragging upward inserts directly at the slot index.
        assert_eq!(drop_target_index(8, 0, false), Some(0));
        assert_eq!(drop_target_index(8, 0, true), Some(1));
        assert_eq!(drop_target_index(5, 2, false), Some(2));
        assert_eq!(drop_target_index(5, 2, true), Some(3));
    }

    struct ConnectionTypesSectionHost {
        panel: Entity<SettingsPanel>,
    }

    impl Render for ConnectionTypesSectionHost {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let section = self.panel.update(cx, |panel, cx| {
                panel
                    .connection_types_settings_section(cx)
                    .into_any_element()
            });
            div().w(px(720.)).p_4().child(section)
        }
    }

    fn test_app(cx: &mut TestAppContext, root: &Path) -> Entity<NyaTermApp> {
        let runtime = AppRuntime::from_parts_for_test(
            RuntimeMode::Portable,
            root.to_path_buf(),
            root.join("config"),
            root.join("logs"),
            root.join("cache"),
            None,
        );
        let stores = UiStoreHandles {
            startup_restore: cx.new(|_| StartupRestoreStore::default()),
            overlays: cx.new(|_| OverlayStore::default()),
        };
        cx.new(|cx| NyaTermApp::new(runtime, stores, cx))
    }

    fn draw_settings(
        app: &Entity<NyaTermApp>,
        panel: &Entity<SettingsPanel>,
        cx: &mut VisualTestContext,
    ) {
        cx.run_until_parked();
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.flush_settings_panel_snapshots(cx);
                cx.notify();
            });
            panel.update(cx, |_, cx| cx.notify());
            let _ = window.draw(cx);
        });
        cx.run_until_parked();
    }

    fn bounds_center_left(bounds: Bounds<gpui::Pixels>) -> gpui::Point<gpui::Pixels> {
        point(
            bounds.origin.x + px(28.),
            bounds.origin.y + bounds.size.height / 2.,
        )
    }

    #[gpui::test]
    fn connection_types_default_badge_follows_first_visible_and_supports_drag_reorder(
        cx: &mut TestAppContext,
    ) {
        let test_dir = TestConfigDir::new("nyaterm-connection-types-settings-ui");
        let _cleanup = TestAppCleanup::new(cx);
        cx.update(nyaterm_ui::init_components);
        let app = test_app(cx, test_dir.path());
        cx.update_entity(&app, |app, cx| {
            app.sync_component_theme(cx);
            app.open_page(NavItem::Settings, cx);
            app.focus_settings_tab(SettingsTab::General, cx);
            app.flush_settings_panel_snapshots(cx);
        });
        let panel = cx.update_entity(&app, |app, _| app.settings_panel.clone());

        let host_panel = panel.clone();
        let (_, cx) = cx.add_window_view(move |window, _| {
            window.resize(gpui::size(px(800.), px(900.)));
            ConnectionTypesSectionHost { panel: host_panel }
        });
        draw_settings(&app, &panel, cx);

        // Initially SSH is first and carries the default badge.
        assert!(cx.debug_bounds("connection-type-default-ssh").is_some());
        assert!(cx.debug_bounds("connection-type-default-local").is_none());

        // Hide SSH: the default badge immediately moves to Local.
        app.update(cx, |app, cx| {
            app.set_connection_type_visible(ConnectionKindTab::Ssh, false, cx);
        });
        draw_settings(&app, &panel, cx);
        assert!(cx.debug_bounds("connection-type-default-ssh").is_none());
        assert!(cx.debug_bounds("connection-type-default-local").is_some());

        // Drag FTP from the bottom up onto the top half of SSH (index 0).
        let ftp_drag = cx
            .debug_bounds("connection-type-drag-ftp")
            .expect("ftp drag handle must be rendered");
        let ssh_row = cx
            .debug_bounds("connection-type-row-ssh")
            .expect("ssh row must be rendered");
        let start = bounds_center_left(ftp_drag);
        let before_ssh = point(ssh_row.origin.x + px(40.), ssh_row.origin.y + px(6.));

        cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_move(
            start + point(px(8.), px(0.)),
            MouseButton::Left,
            Modifiers::default(),
        );
        cx.simulate_mouse_move(before_ssh, MouseButton::Left, Modifiers::default());
        draw_settings(&app, &panel, cx);
        assert!(
            cx.debug_bounds("connection-type-drop-indicator-ssh-before")
                .is_some(),
            "hovering the upper half of SSH during drag should show the top drop indicator"
        );

        cx.simulate_mouse_up(before_ssh, MouseButton::Left, Modifiers::default());
        draw_settings(&app, &panel, cx);

        let catalog = ConnectionKindTab::catalog_ids();
        app.read_with(cx, |app, _| {
            let ordered = app
                .settings
                .summary()
                .connection_type_layout
                .ordered(&catalog);
            assert_eq!(ordered[0], "ftp");
            assert_eq!(ordered[1], "ssh");
        });
        // Because FTP is visible and now first, it becomes the default type!
        assert!(cx.debug_bounds("connection-type-default-ftp").is_some());
        assert!(cx.debug_bounds("connection-type-default-local").is_none());

        // Clicking Restore defaults restores SSH first and all types visible.
        let reset_bounds = cx
            .debug_bounds("connection-types-reset")
            .expect("reset action must be rendered in section header");
        cx.simulate_click(reset_bounds.center(), Modifiers::default());
        draw_settings(&app, &panel, cx);

        app.read_with(cx, |app, _| {
            assert!(
                app.settings
                    .summary()
                    .connection_type_layout
                    .is_default_for(&catalog)
            );
        });
        assert!(cx.debug_bounds("connection-type-default-ssh").is_some());
    }
}
