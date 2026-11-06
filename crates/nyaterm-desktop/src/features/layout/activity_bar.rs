use rust_i18n::t;

use std::borrow::Cow;

use gpui::{
    Context, FontWeight, IntoElement, MouseButton, MouseDownEvent, SharedString, div, prelude::*,
    px, rgb, rgba, svg,
};

use crate::features::NyaTermApp;
use crate::features::runtime_jobs::ActivitySide;
use crate::features::shell::{ActivityBarDragPayload, ActivityBarDragPreview};
use crate::features::view_widgets::activity_icon;
use crate::models::{
    ActivityBarContextTarget, ActivityBarEntry, ActivityBarZone, PanelOpenMode, PanelSide,
};
use nyaterm_ui::NyaTooltip;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ActivityBarMenuRow {
    Floating,
    Submenu,
    Hide,
    Labels,
    Reset,
    Separator,
}

const ENTRY_MENU_ROWS: &[ActivityBarMenuRow] = &[
    ActivityBarMenuRow::Submenu,
    ActivityBarMenuRow::Separator,
    ActivityBarMenuRow::Hide,
    ActivityBarMenuRow::Separator,
    ActivityBarMenuRow::Labels,
];

const BAR_MENU_ROWS: &[ActivityBarMenuRow] = &[
    ActivityBarMenuRow::Floating,
    ActivityBarMenuRow::Separator,
    ActivityBarMenuRow::Submenu,
    ActivityBarMenuRow::Separator,
    ActivityBarMenuRow::Labels,
    ActivityBarMenuRow::Separator,
    ActivityBarMenuRow::Reset,
];

fn activity_bar_menu_rows(is_entry: bool) -> &'static [ActivityBarMenuRow] {
    if is_entry {
        ENTRY_MENU_ROWS
    } else {
        BAR_MENU_ROWS
    }
}

fn activity_bar_menu_height(is_entry: bool) -> f32 {
    activity_bar_menu_rows(is_entry)
        .iter()
        .map(|row| match row {
            ActivityBarMenuRow::Separator => 7.,
            _ => 30.,
        })
        .sum::<f32>()
        + 10.
}

fn activity_bar_submenu_height(row_count: usize) -> f32 {
    row_count.max(1) as f32 * 30. + 10.
}

impl NyaTermApp {
    fn hidden_activity_entry_ids_for_target(
        &self,
        target: &ActivityBarContextTarget,
    ) -> Vec<String> {
        match target {
            ActivityBarContextTarget::Bar { side } => self
                .shell
                .activity_bar_layout()
                .hidden_entries_on_side(*side)
                .into_iter()
                .filter(|id| ActivityBarEntry::from_persistence_id(id).is_some())
                .collect(),
            ActivityBarContextTarget::Entry { .. } => Vec::new(),
        }
    }

    pub(in crate::features) fn activity_bar_context_menu_overlay(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let Some(menu) = self.shell.activity_bar_context_menu().cloned() else {
            return div().into_any_element();
        };
        let show_labels = self.shell.activity_bar_layout().show_labels;
        let hidden_ids = self.hidden_activity_entry_ids_for_target(&menu.target);
        let has_hidden = !hidden_ids.is_empty();
        let is_entry = matches!(menu.target, ActivityBarContextTarget::Entry { .. });
        let submenu_row_count = if is_entry {
            ActivityBarZone::all().len()
        } else {
            hidden_ids.len().max(1)
        };
        let (viewport_w, viewport_h) = self.shell.viewport_size();
        let menu_w = 200.;
        let submenu_w = 176.;
        let margin = 8.;
        let menu_h = activity_bar_menu_height(is_entry);
        let menu_x = f32::from(menu.x).clamp(margin, (viewport_w - menu_w - margin).max(margin));
        let menu_y = f32::from(menu.y).clamp(margin, (viewport_h - menu_h - margin).max(margin));
        let move_to_label = t!("activityBar.moveTo").to_string();
        let show_labels_label = t!("activityBar.showLabel").to_string();
        let show_hidden_label = t!("activityBar.hiddenItems").to_string();
        let reset_label = t!("activityBar.resetLayout").to_string();

        let entry_id = menu.entry_id().map(str::to_string);
        let entry_label = entry_id
            .as_deref()
            .and_then(ActivityBarEntry::from_persistence_id)
            .map(|entry| {
                entry
                    .i18n_key()
                    .map(|key| t!(key).to_string())
                    .unwrap_or_else(|| entry.label().to_string())
            })
            .unwrap_or_default();
        let hide_label = t!("activityBar.hideItem", name = entry_label).to_string();
        let entry_zone = menu.entry_zone();

        // Move-to submenu (entry target) or show-hidden submenu (bar target).
        let mut submenu = div().flex().flex_col();
        if is_entry {
            for zone in ActivityBarZone::all() {
                let target = zone;
                let id = entry_id.clone().unwrap_or_default();
                let selected = Some(zone) == entry_zone;
                submenu = submenu.child(
                    div()
                        .id(SharedString::from(format!(
                            "activity-move-{}",
                            zone.persistence_key()
                        )))
                        .h(px(30.))
                        .mx_1()
                        .px_2()
                        .rounded_sm()
                        .flex()
                        .items_center()
                        .text_xs()
                        .text_color(rgb(if selected {
                            palette.text_muted
                        } else {
                            palette.text
                        }))
                        .when(!selected, |this| {
                            this.cursor_pointer()
                                .hover(|this| this.bg(rgb(palette.hover)))
                        })
                        .child(t!(zone.i18n_key()))
                        .when(!selected, |this| {
                            this.on_click(cx.listener(move |this, _, _, cx| {
                                this.move_activity_entry(id.clone(), target, None, cx);
                            }))
                        }),
                );
            }
        } else if hidden_ids.is_empty() {
            submenu = submenu.child(
                div()
                    .h(px(30.))
                    .mx_1()
                    .px_2()
                    .rounded_sm()
                    .flex()
                    .items_center()
                    .text_xs()
                    .text_color(rgb(palette.text_muted))
                    .child(t!("activityBar.noHiddenItems")),
            );
        } else {
            for hidden_id in hidden_ids {
                let label = crate::models::ActivityBarEntry::from_persistence_id(&hidden_id)
                    .map(|entry| {
                        entry
                            .i18n_key()
                            .map(|key| t!(key).to_string())
                            .unwrap_or_else(|| entry.label().to_string())
                    })
                    .unwrap_or_else(|| hidden_id.clone());
                let id = hidden_id.clone();
                submenu = submenu.child(
                    div()
                        .id(SharedString::from(format!("activity-show-{hidden_id}")))
                        .h(px(30.))
                        .mx_1()
                        .px_2()
                        .rounded_sm()
                        .flex()
                        .items_center()
                        .text_xs()
                        .text_color(rgb(palette.text))
                        .cursor_pointer()
                        .hover(|this| this.bg(rgb(palette.hover)))
                        .child(label)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.show_activity_entry(id.clone(), cx);
                        })),
                );
            }
        }

        let submenu_label = if is_entry {
            move_to_label
        } else {
            show_hidden_label.clone()
        };
        let submenu_x = if menu_x + menu_w + 4. + submenu_w <= viewport_w - margin {
            menu_x + menu_w + 4.
        } else {
            (menu_x - submenu_w - 4.).max(margin)
        };
        let submenu_h = activity_bar_submenu_height(submenu_row_count);
        let submenu_y = menu_y.clamp(margin, (viewport_h - submenu_h - margin).max(margin));

        let submenu_enabled = is_entry || has_hidden;
        let floating = self.shell.panel_is_floating();
        let icon_slot = || {
            div()
                .w(px(14.))
                .h(px(14.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
        };
        let separator = || {
            div()
                .h(px(1.))
                .my(px(3.))
                .mx_3()
                .bg(rgb(palette.border))
                .opacity(0.65)
        };

        let mut parent_menu = div()
            .id(SharedString::from("activity-context-menu"))
            .absolute()
            .top(px(menu_y))
            .left(px(menu_x))
            .w(px(menu_w))
            .rounded_md()
            .border_1()
            .border_color(rgb(palette.border))
            .bg(self.shell_surface_color(palette.surface))
            .shadow_lg()
            .py_1()
            .flex()
            .flex_col()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(|_, _, cx| cx.stop_propagation());

        for &row in activity_bar_menu_rows(is_entry) {
            parent_menu = match row {
                ActivityBarMenuRow::Separator => parent_menu.child(separator()),
                ActivityBarMenuRow::Submenu => {
                    let label_color = if submenu_enabled {
                        palette.text
                    } else {
                        palette.text_muted
                    };
                    parent_menu.child(
                        div()
                            .id(SharedString::from("activity-submenu-opener"))
                            .h(px(30.))
                            .mx_1()
                            .px_2()
                            .rounded_sm()
                            .flex()
                            .items_center()
                            .justify_between()
                            .text_xs()
                            .text_color(rgb(label_color))
                            .when(submenu_enabled, |this| {
                                this.cursor_pointer()
                                    .when(menu.move_submenu_open, |this| {
                                        this.bg(rgb(palette.hover))
                                    })
                                    .hover(|this| this.bg(rgb(palette.hover)))
                                    .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                                        if *hovered {
                                            this.open_activity_bar_move_submenu(cx);
                                        }
                                    }))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.open_activity_bar_move_submenu(cx);
                                    }))
                            })
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(icon_slot())
                                    .child(submenu_label.clone()),
                            )
                            .child(
                                svg()
                                    .size(px(12.))
                                    .path("icons/fe/forward.svg")
                                    .text_color(rgb(label_color)),
                            ),
                    )
                }
                ActivityBarMenuRow::Hide => {
                    let hide_id = entry_id.clone().unwrap_or_default();
                    parent_menu.child(
                        div()
                            .id(SharedString::from("activity-hide-entry"))
                            .h(px(30.))
                            .mx_1()
                            .px_2()
                            .rounded_sm()
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_xs()
                            .text_color(rgb(palette.text))
                            .cursor_pointer()
                            .hover(|this| this.bg(rgb(palette.hover)))
                            .child(
                                icon_slot().child(
                                    svg()
                                        .size(px(13.))
                                        .path("icons/eye.svg")
                                        .text_color(rgb(palette.text_dimmed)),
                                ),
                            )
                            .child(hide_label.clone())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.hide_activity_entry(hide_id.clone(), cx);
                            })),
                    )
                }
                ActivityBarMenuRow::Floating => parent_menu.child(
                    div()
                        .id(SharedString::from("activity-toggle-floating"))
                        .h(px(30.))
                        .mx_1()
                        .px_2()
                        .rounded_sm()
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_xs()
                        .text_color(rgb(palette.text))
                        .cursor_pointer()
                        .hover(|this| this.bg(rgb(palette.hover)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.set_panel_open_mode(
                                if floating {
                                    PanelOpenMode::Docked
                                } else {
                                    PanelOpenMode::Floating
                                },
                                cx,
                            );
                            this.close_activity_bar_context_menu(cx);
                        }))
                        .child(icon_slot().when(floating, |this| {
                            this.child(
                                svg()
                                    .size(px(13.))
                                    .path("icons/check.svg")
                                    .text_color(rgb(palette.link)),
                            )
                        }))
                        .child(t!("panel.floatingMode")),
                ),
                ActivityBarMenuRow::Labels => parent_menu.child(
                    div()
                        .id(SharedString::from("activity-toggle-labels"))
                        .h(px(30.))
                        .mx_1()
                        .px_2()
                        .rounded_sm()
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_xs()
                        .text_color(rgb(palette.text))
                        .cursor_pointer()
                        .hover(|this| this.bg(rgb(palette.hover)))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.toggle_activity_bar_labels(cx);
                            this.close_activity_bar_context_menu(cx);
                        }))
                        .child(icon_slot().when(show_labels, |this| {
                            this.child(
                                svg()
                                    .size(px(13.))
                                    .path("icons/check.svg")
                                    .text_color(rgb(palette.link)),
                            )
                        }))
                        .child(show_labels_label.clone()),
                ),
                ActivityBarMenuRow::Reset => parent_menu.child(
                    div()
                        .id(SharedString::from("activity-reset-layout"))
                        .h(px(30.))
                        .mx_1()
                        .px_2()
                        .rounded_sm()
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_xs()
                        .text_color(rgb(palette.danger))
                        .cursor_pointer()
                        .hover(|this| this.bg(rgb(palette.hover)))
                        .child(
                            icon_slot().child(
                                svg()
                                    .size(px(13.))
                                    .path("icons/menu/reset.svg")
                                    .text_color(rgb(palette.danger)),
                            ),
                        )
                        .child(reset_label.clone())
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.close_activity_bar_context_menu(cx);
                            this.confirm_reset_activity_bar_layout(window, cx);
                        })),
                ),
            };
        }

        div()
            .id(SharedString::from("activity-context-backdrop"))
            .absolute()
            .inset_0()
            .on_click(cx.listener(|this, _, _, cx| {
                this.close_activity_bar_context_menu(cx);
            }))
            .child(parent_menu)
            .when(menu.move_submenu_open && submenu_enabled, |this| {
                this.child(
                    div()
                        .id(SharedString::from("activity-move-submenu"))
                        .absolute()
                        .top(px(submenu_y))
                        .left(px(submenu_x))
                        .w(px(submenu_w))
                        .rounded_md()
                        .border_1()
                        .border_color(rgb(palette.border))
                        .bg(self.shell_surface_color(palette.surface))
                        .shadow_lg()
                        .py_1()
                        .flex()
                        .flex_col()
                        .child(submenu)
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(|_, _, cx| cx.stop_propagation()),
                )
            })
            .into_any_element()
    }

    pub(in crate::features) fn activity_bar(
        &mut self,
        side: ActivitySide,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let (top_zone, bottom_zone) = match side {
            ActivitySide::Left => (ActivityBarZone::LeftTop, ActivityBarZone::LeftBottom),
            ActivitySide::Right => (ActivityBarZone::RightTop, ActivityBarZone::RightBottom),
        };
        let top_entries = self.activity_entries_for_zone(top_zone);
        let bottom_entries = self.activity_entries_for_zone(bottom_zone);
        let top_len = top_entries.len();
        let bottom_len = bottom_entries.len();
        let show_labels = self.shell.activity_bar_layout().show_labels;
        let palette = self.theme_palette();

        // Tauri DropZone: gap-0.5 pt-1
        let mut top = div().flex().flex_col().items_center().gap(px(2.)).pt_1();
        for (index, entry) in top_entries.into_iter().enumerate() {
            top = top.child(self.activity_entry_button(
                entry,
                side,
                top_zone,
                index,
                show_labels,
                cx,
            ));
        }
        // End-of-zone drop target (append).
        top = top.child(self.activity_zone_end_drop_target(top_zone, top_len, cx));

        let mut bottom = div()
            .mt_auto()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(2.))
            .pb_1();
        for (index, entry) in bottom_entries.into_iter().enumerate() {
            bottom = bottom.child(self.activity_entry_button(
                entry,
                side,
                bottom_zone,
                index,
                show_labels,
                cx,
            ));
        }
        bottom = bottom.child(self.activity_zone_end_drop_target(bottom_zone, bottom_len, cx));

        div()
            .w(px(40.))
            .flex_none()
            .flex()
            .flex_col()
            .border_color(rgb(palette.border))
            .when(side == ActivitySide::Left, |this| this.border_r_1())
            .when(side == ActivitySide::Right, |this| this.border_l_1())
            .bg(self.shell_surface_color(palette.surface))
            .id(SharedString::from(match side {
                ActivitySide::Left => "activity-bar-left",
                ActivitySide::Right => "activity-bar-right",
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    let panel_side = match side {
                        ActivitySide::Left => PanelSide::Left,
                        ActivitySide::Right => PanelSide::Right,
                    };
                    this.open_activity_bar_side_context_menu(panel_side, event, cx);
                }),
            )
            .child(top)
            .child(bottom)
    }

    pub(in crate::features) fn activity_zone_end_drop_target(
        &self,
        zone: ActivityBarZone,
        end_index: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let zone_key = zone.persistence_key();
        let palette = self.theme_palette();
        let drop_line = rgb(palette.primary);
        let drop_wash = rgba((palette.primary << 8) | 0x16);
        div()
            .id(SharedString::from(format!("activity-zone-end-{zone_key}")))
            .w_full()
            .h(px(8.))
            .flex_none()
            // Keep the border in the normal layout so entering a drop target
            // only changes its color and never nudges the surrounding icons.
            .border_t_2()
            .border_color(rgba(0x00000000))
            .drag_over::<ActivityBarDragPayload>(move |this, payload, _, _| {
                if payload.entry_id.is_empty() {
                    this
                } else {
                    this.border_color(drop_line).bg(drop_wash)
                }
            })
            .on_drop(
                cx.listener(move |this, payload: &ActivityBarDragPayload, _, cx| {
                    if payload.entry_id.is_empty() {
                        return;
                    }
                    this.move_activity_entry(payload.entry_id.clone(), zone, Some(end_index), cx);
                }),
            )
    }

    pub(in crate::features) fn activity_entry_button(
        &self,
        entry: ActivityBarEntry,
        side: ActivitySide,
        zone: ActivityBarZone,
        index: usize,
        show_labels: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let selected = self.activity_entry_selected(entry);
        let icon_path = entry.icon_path();
        let tooltip = entry
            .i18n_key()
            .map(|key| t!(key))
            .unwrap_or_else(|| Cow::Borrowed(entry.label()))
            .to_string();
        let palette = self.theme_palette();
        let active_color = rgb(palette.primary);
        let icon_color = if selected {
            active_color
        } else {
            rgb(palette.text_muted)
        };
        let entry_id = entry.persistence_id().to_string();
        let context_entry_id = entry_id.clone();
        let indicator = if selected {
            active_color
        } else {
            rgba(0x00000000)
        };

        div()
            .id(SharedString::from(format!("activity-{entry_id}")))
            .debug_selector({
                let entry_id = entry_id.clone();
                move || format!("activity-{entry_id}")
            })
            .relative()
            .when(show_labels, |this| {
                this.w_full()
                    .min_h(px(48.))
                    .px_1()
                    .py_1()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(2.))
            })
            .when(!show_labels, |this| {
                this.w_full()
                    .h(px(36.))
                    .flex()
                    .items_center()
                    .justify_center()
            })
            .rounded_sm()
            .cursor_pointer()
            .text_sm()
            .font_weight(FontWeight(700.))
            .text_color(if selected {
                // Tauri ActivityBarButton uses primary color when active.
                active_color
            } else {
                rgb(palette.text_muted)
            })
            .bg(rgba(0x00000000))
            .child(
                div()
                    .absolute()
                    .top(px(4.))
                    .bottom(px(4.))
                    .w(px(2.))
                    .rounded_full()
                    .bg(indicator)
                    .when(side == ActivitySide::Left, |this| this.left_0())
                    .when(side == ActivitySide::Right, |this| this.right_0()),
            )
            .child(activity_icon(icon_path, icon_color.into(), 18.))
            .when(show_labels, |this| {
                this.child(
                    div()
                        .w_full()
                        .text_size(px(8.))
                        .font_weight(FontWeight(500.))
                        .text_center()
                        .whitespace_normal()
                        .text_color(if selected {
                            active_color
                        } else {
                            rgb(palette.text_muted)
                        })
                        .child(tooltip.clone()),
                )
            })
            // Every icon represents the insertion slot immediately before it.
            // A transparent border reserves the line's space outside a drag.
            .border_t_2()
            .border_color(rgba(0x00000000))
            .cursor_move()
            .on_drag(
                ActivityBarDragPayload {
                    entry_id: entry_id.clone(),
                    label: tooltip.clone(),
                    icon_path,
                },
                |payload, position, _, cx| {
                    cx.new(|_| ActivityBarDragPreview::new(payload.clone(), position))
                },
            )
            .drag_over::<ActivityBarDragPayload>({
                let drop_entry_id = entry_id.clone();
                move |this, payload, _, _| {
                    if payload.entry_id.is_empty() || payload.entry_id == drop_entry_id {
                        this
                    } else {
                        this.border_color(active_color)
                            .bg(rgba((palette.primary << 8) | 0x16))
                    }
                }
            })
            .on_drop({
                let drop_zone = zone;
                let drop_index = index;
                cx.listener(move |this, payload: &ActivityBarDragPayload, _, cx| {
                    if payload.entry_id.is_empty() {
                        return;
                    }
                    // Drop onto this button inserts before it (Tauri dropIndex == idx).
                    this.move_activity_entry(
                        payload.entry_id.clone(),
                        drop_zone,
                        Some(drop_index),
                        cx,
                    );
                })
            })
            .tooltip({
                let title = tooltip.clone();
                move |window, cx| NyaTooltip::new(title.clone()).build(window, cx)
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                this.activate_activity_entry(entry, window, cx);
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.open_activity_bar_context_menu(
                        context_entry_id.clone(),
                        zone,
                        index,
                        event,
                        cx,
                    );
                }),
            )
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use gpui::{
        AppContext as _, Entity, IntoElement, Modifiers, MouseButton, ParentElement, Render,
        Styled, TestAppContext, VisualTestContext, div,
    };
    use nyaterm_core::{AppRuntime, RuntimeMode};

    use crate::entities::{OverlayStore, StartupRestoreStore, UiStoreHandles};
    use crate::features::NyaTermApp;
    use crate::models::{ActivityBarContextTarget, ActivityBarZone, PanelSide};
    use crate::test_support::TestConfigDir;

    struct AppHost {
        app: Entity<NyaTermApp>,
    }

    impl Render for AppHost {
        fn render(
            &mut self,
            _window: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let has_menu = self
                .app
                .read(cx)
                .shell
                .activity_bar_context_menu()
                .is_some();
            let bar = self.app.update(cx, |app, cx| {
                app.activity_bar(crate::features::runtime_jobs::ActivitySide::Left, cx)
                    .into_any_element()
            });
            let overlay = has_menu.then(|| {
                self.app.update(cx, |app, cx| {
                    app.activity_bar_context_menu_overlay(cx).into_any_element()
                })
            });
            let mut root = div().size_full().relative().flex().child(bar);
            if let Some(overlay) = overlay {
                root = root.child(overlay);
            }
            root
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

    fn draw(app: &Entity<NyaTermApp>, cx: &mut VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| {
            app.update(cx, |_, cx| cx.notify());
            _ = window.draw(cx);
        });
        cx.run_until_parked();
    }

    #[test]
    fn entry_and_bar_menus_keep_distinct_tauri_grouping() {
        assert_eq!(
            super::activity_bar_menu_rows(true),
            &[
                super::ActivityBarMenuRow::Submenu,
                super::ActivityBarMenuRow::Separator,
                super::ActivityBarMenuRow::Hide,
                super::ActivityBarMenuRow::Separator,
                super::ActivityBarMenuRow::Labels,
            ]
        );
        assert_eq!(
            super::activity_bar_menu_rows(false),
            &[
                super::ActivityBarMenuRow::Floating,
                super::ActivityBarMenuRow::Separator,
                super::ActivityBarMenuRow::Submenu,
                super::ActivityBarMenuRow::Separator,
                super::ActivityBarMenuRow::Labels,
                super::ActivityBarMenuRow::Separator,
                super::ActivityBarMenuRow::Reset,
            ]
        );
        assert_eq!(super::activity_bar_menu_height(true), 114.);
        assert_eq!(super::activity_bar_menu_height(false), 151.);
        assert_eq!(super::activity_bar_submenu_height(4), 130.);
    }

    #[test]
    fn right_click_keeps_entry_target_and_blank_space_uses_bar_target() {
        let test_dir = TestConfigDir::new("nyaterm-activity-bar-view");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = test_app(&mut cx, test_dir.path());
        cx.update_entity(&app, |app, cx| app.sync_component_theme(cx));
        let host_app = app.clone();
        let (_, cx) = cx.add_window_view(move |_, _| AppHost { app: host_app });
        let cx: &mut VisualTestContext = cx;
        draw(&app, cx);

        let entry_center = gpui::point(gpui::px(20.), gpui::px(22.));
        cx.simulate_mouse_down(entry_center, MouseButton::Right, Modifiers::default());
        draw(&app, cx);
        cx.update(|_, cx| {
            let menu = app
                .read(cx)
                .shell
                .activity_bar_context_menu()
                .expect("entry context menu should be open");
            assert!(matches!(
                &menu.target,
                ActivityBarContextTarget::Entry { entry_id, .. }
                    if entry_id == "fileExplorer"
            ));
        });

        cx.update(|_, cx| {
            app.update(cx, |app, cx| app.close_activity_bar_context_menu(cx));
        });
        draw(&app, cx);
        let bar_blank = gpui::point(gpui::px(20.), gpui::px(300.));
        cx.simulate_mouse_down(bar_blank, MouseButton::Right, Modifiers::default());
        draw(&app, cx);
        cx.update(|_, cx| {
            let menu = app
                .read(cx)
                .shell
                .activity_bar_context_menu()
                .expect("bar context menu should be open");
            assert_eq!(
                menu.target,
                ActivityBarContextTarget::Bar {
                    side: PanelSide::Left
                }
            );
        });
    }

    #[test]
    fn bar_menu_hidden_items_opener_tracks_current_side_recovery() {
        let test_dir = TestConfigDir::new("nyaterm-activity-bar-view");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = test_app(&mut cx, test_dir.path());
        cx.update_entity(&app, |app, cx| app.sync_component_theme(cx));
        let host_app = app.clone();
        let (_, cx) = cx.add_window_view(move |_, _| AppHost { app: host_app });
        let cx: &mut VisualTestContext = cx;
        draw(&app, cx);

        let bar_blank = gpui::point(gpui::px(20.), gpui::px(300.));
        cx.simulate_mouse_down(bar_blank, MouseButton::Right, Modifiers::default());
        draw(&app, cx);
        // Floating occupies the first 30px row, followed by a 7px
        // separator. Hidden Items is the next row; if that row were omitted,
        // this click would hit the following action and dismiss the menu.
        let opener_center = gpui::point(gpui::px(100.), gpui::px(356.));
        cx.simulate_click(opener_center, Modifiers::default());
        draw(&app, cx);

        cx.update(|_, cx| {
            let menu = app
                .read(cx)
                .shell
                .activity_bar_context_menu()
                .expect("disabled opener should not dismiss its menu");
            assert!(!menu.move_submenu_open);
        });

        cx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.hide_activity_entry("fileExplorer".to_string(), cx)
            });
        });
        draw(&app, cx);
        cx.simulate_mouse_down(bar_blank, MouseButton::Right, Modifiers::default());
        draw(&app, cx);
        cx.simulate_click(opener_center, Modifiers::default());
        draw(&app, cx);
        cx.update(|_, cx| {
            let menu = app
                .read(cx)
                .shell
                .activity_bar_context_menu()
                .expect("enabled opener should keep its parent menu open");
            assert!(menu.move_submenu_open);
        });
    }

    #[test]
    fn hidden_recovery_is_side_scoped_and_includes_monitor_entries() {
        let test_dir = TestConfigDir::new("nyaterm-activity-bar-view");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = test_app(&mut cx, test_dir.path());
        cx.update_entity(&app, |app, cx| {
            app.hide_activity_entry("fileExplorer".to_string(), cx);
            app.hide_activity_entry("aiAssistant".to_string(), cx);
            app.hide_activity_entry("gpuMonitor".to_string(), cx);

            let left = app.hidden_activity_entry_ids_for_target(&ActivityBarContextTarget::Bar {
                side: PanelSide::Left,
            });
            let right = app.hidden_activity_entry_ids_for_target(&ActivityBarContextTarget::Bar {
                side: PanelSide::Right,
            });

            assert_eq!(left, vec!["fileExplorer"]);
            assert!(right.iter().any(|id| id == "aiAssistant"));
            assert!(!right.iter().any(|id| id == "fileExplorer"));
            assert!(right.iter().any(|id| id == "gpuMonitor"));
        });
    }

    #[test]
    fn activity_entry_released_outside_preserves_every_zone() {
        let test_dir = TestConfigDir::new("nyaterm-activity-external-drop");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = test_app(&mut cx, test_dir.path());
        app.update(&mut cx, |app, cx| app.sync_component_theme(cx));
        let host_app = app.clone();
        let (_, cx) = cx.add_window_view(move |_, _| AppHost { app: host_app });
        draw(&app, cx);
        let zones = [
            ActivityBarZone::LeftTop,
            ActivityBarZone::LeftBottom,
            ActivityBarZone::RightTop,
            ActivityBarZone::RightBottom,
        ];
        let before = app.read_with(cx, |app, _| {
            zones.map(|zone| app.activity_entries_for_zone(zone))
        });
        let source = gpui::point(gpui::px(20.), gpui::px(22.));
        let outside = gpui::point(gpui::px(-30.), gpui::px(22.));
        cx.simulate_mouse_down(source, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_move(outside, MouseButton::Left, Modifiers::default());
        cx.read(|cx| assert!(cx.has_active_drag()));
        cx.simulate_mouse_up(outside, MouseButton::Left, Modifiers::default());
        cx.read(|cx| assert!(!cx.has_active_drag()));
        app.read_with(cx, |app, _| {
            assert_eq!(
                zones.map(|zone| app.activity_entries_for_zone(zone)),
                before
            );
        });
    }
}
