use gpui::{
    Context, FontWeight, IntoElement, MouseButton, SharedString, div, prelude::*, px, rgb, rgba,
    svg,
};
use nyaterm_ui::tab_close::NyaTabCloseButton;
use nyaterm_ui::{NyaContextMenu, NyaMenuItem, NyaTooltip};
use rust_i18n::t;

use crate::features::NyaTermApp;
use crate::features::shell::session_tabs::SessionStripTabId;
use crate::features::shell::{SessionTabDragPreview, WorkspaceTabDragPayload};
use crate::models::WorkspaceTabId;

impl NyaTermApp {
    pub(in crate::features) fn settings_strip_tab(
        &mut self,
        index: usize,
        number: usize,
        active: Option<&str>,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let palette = self.theme_palette();
        let key = WorkspaceTabId::settings_key();
        let selected = active == Some(key.as_str());
        let tab_group = SharedString::from("settings-tab-group");
        let hover_bg = self.shell_surface_color(palette.hover);
        let click_key = key.clone();
        let drop_key = key.clone();
        let title = t!("settings.title").to_string();
        let dirty = self.settings_draft_dirty() || self.settings.theme_designer().dirty();
        let payload = WorkspaceTabDragPayload {
            source_workspace_id: self.workspace_id,
            root_tab_id: key.clone(),
            source_revision: self.workspace_revision(),
            tab_id: SessionStripTabId::Workspace(WorkspaceTabId::Settings),
            order_index: index,
            source_group_id: self.shell.terminal_group_for_tab(&key),
            display_name: title.clone(),
            kind_icon: "icons/settings.svg",
            preview_background: palette.surface,
            preview_border: palette.border,
            preview_text: palette.text,
            preview_text_muted: palette.text_muted,
            preview_accent: palette.primary,
        };
        let app = cx.weak_entity();
        let drag_app = app.clone();
        let close_label = if dirty {
            format!(
                "{} · {}",
                t!("common.close"),
                t!("settings.unappliedChanges")
            )
        } else {
            t!("common.close").to_string()
        };
        let tooltip = if dirty {
            format!("{title}\n{}", t!("settings.unappliedChanges"))
        } else {
            title.clone()
        };
        let tab = div()
            .id(SharedString::from("settings-tab"))
            .debug_selector(|| "settings-tab".into())
            .group(tab_group.clone())
            .h(px(36.))
            .min_w(px(118.))
            .max_w(px(236.))
            .flex_none()
            .pl_3()
            .pr_2()
            .flex()
            .items_center()
            .gap_2()
            .relative()
            .border_r_1()
            .border_color(rgb(palette.border))
            .when(!selected, |this| this.border_b_1())
            .bg(if selected {
                self.shell_surface_color(palette.bg)
            } else {
                rgba(0x00000000)
            })
            .cursor_pointer()
            .hover(move |this| this.bg(hover_bg))
            .tooltip(move |window, cx| NyaTooltip::new(tooltip.clone()).build(window, cx))
            .on_click(cx.listener(move |app, _, _, cx| app.activate_workspace_tab(&click_key, cx)))
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(move |app, _, window, cx| {
                    cx.stop_propagation();
                    app.request_settings_tab_close(window, cx);
                }),
            )
            .on_drag(payload, move |payload, position, window, cx| {
                let _ = drag_app.update(cx, |app, cx| {
                    app.begin_session_tab_drag(payload, position, window, cx)
                });
                cx.new(|_| SessionTabDragPreview::new(payload.clone(), position))
            })
            .on_drop(
                cx.listener(move |app, payload: &WorkspaceTabDragPayload, _, cx| {
                    let group = app.shell.terminal_group_for_tab(&drop_key);
                    if payload.tab_id.is_start()
                        && (payload.source_workspace_id != app.workspace_id
                            || payload.source_group_id != group)
                    {
                        app.clear_session_tab_drag(cx);
                        return;
                    }
                    app.accept_session_tab_drop(payload, cx);
                    if app.commit_session_tab_preview(payload, &group, cx) {
                        return;
                    }
                    if payload.source_workspace_id == app.workspace_id {
                        app.reorder_session_relative(
                            payload.tab_id.layout_key(),
                            drop_key.clone(),
                            false,
                            cx,
                        );
                    }
                }),
            )
            .when(selected, |this| {
                this.child(
                    div()
                        .debug_selector(|| "settings-tab-active".into())
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .h(px(2.))
                        .bg(rgb(palette.primary)),
                )
            })
            .child(
                div()
                    .size(px(14.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        svg()
                            .path("icons/settings.svg")
                            .size(px(14.))
                            .text_color(rgb(palette.text_muted)),
                    ),
            )
            .child(
                div()
                    .min_w(px(12.))
                    .flex_none()
                    .text_size(px(12.))
                    .font_weight(FontWeight(700.))
                    .text_color(rgb(palette.text_dimmed))
                    .child(number.to_string()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .max_w(px(160.))
                    .text_size(px(12.))
                    .font_weight(if selected {
                        FontWeight(700.)
                    } else {
                        FontWeight(500.)
                    })
                    .text_color(rgb(if selected {
                        palette.text
                    } else {
                        palette.text_muted
                    }))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(title),
            )
            .when(dirty, |this| {
                this.child(div().size(px(7.)).rounded_full().bg(rgb(palette.warning)))
            })
            .child(
                NyaTabCloseButton::new("settings-tab-close", tab_group, palette, close_label)
                    .active(selected)
                    .dirty(dirty)
                    .on_click(cx.listener(move |app, _, window, cx| {
                        app.request_settings_tab_close(window, cx);
                    })),
            );
        NyaContextMenu::new_dynamic(tab, move |_, _| {
            let Some(app) = app.upgrade() else {
                return Vec::new();
            };
            let app = app.downgrade();
            vec![
                NyaMenuItem::action(t!("common.close").to_string()).on_click(
                    move |_, window, cx| {
                        let app = app.clone();
                        // Open the discard confirmation after the menu has dismissed.
                        window.defer(cx, move |window, cx| {
                            let _ = app
                                .update(cx, |app, cx| app.request_settings_tab_close(window, cx));
                        });
                    },
                ),
            ]
        })
        .into_any_element()
    }
}
