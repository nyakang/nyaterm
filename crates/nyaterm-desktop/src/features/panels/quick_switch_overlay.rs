use std::sync::Arc;

use rust_i18n::t;

use gpui::{
    Context, FontWeight, IntoElement, SharedString, StatefulInteractiveElement as _, div,
    prelude::*, px, rgb, rgba,
};
use nyaterm_core::truncate_preview;
use nyaterm_ui::{
    NyaCommand, NyaCommandItem, command_row_highlight_index, command_row_selection_wash,
};

use crate::features::NyaTermApp;
use crate::models::QuickSwitchItem;
use crate::widgets::status_pill;

#[derive(Clone, Copy)]
enum QuickSwitchBadge {
    None,
    Active,
    Saved,
}

impl NyaTermApp {
    pub(in crate::features) fn quick_switch_overlay(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let (viewport_w, viewport_h) = self.shell.viewport_size();
        let command_state = self
            .quick_switch_command_state(cx)
            .expect("an open quick switch must own command state");
        let query = command_state.read(cx).query(cx);
        let items = self.filtered_quick_switch_items(&query);
        let catalog_empty = items.is_empty() && self.quick_switch_items().is_empty();
        let items: Arc<[QuickSwitchItem]> = items.into();
        let saved_badge_bg = self.shell_surface_color(palette.hover);
        // Enter opens this row. The command list's own highlight is the theme
        // accent, which is only a hover step away from the dialog surface.
        let highlight =
            command_row_highlight_index(command_state.read(cx).selected_index(cx), items.len());
        let command_items = items
            .iter()
            .enumerate()
            .map(|(row, item)| {
                let selected = highlight == Some(row);
                let title = item.title().to_string();
                let subtitle = item.subtitle().to_string();
                let search_text = item.search_text();
                let badge = match item {
                    QuickSwitchItem::Session { active: true, .. }
                    | QuickSwitchItem::Pending { active: true, .. } => QuickSwitchBadge::Active,
                    QuickSwitchItem::Connection { .. } => QuickSwitchBadge::Saved,
                    QuickSwitchItem::Session { .. } | QuickSwitchItem::Pending { .. } => {
                        QuickSwitchBadge::None
                    }
                };
                let row_title = title.clone();
                NyaCommandItem::new()
                    .label(title)
                    .keywords([search_text])
                    .child(move |window, _| {
                        let badge = match badge {
                            QuickSwitchBadge::None => div().into_any_element(),
                            QuickSwitchBadge::Active => status_pill(
                                t!("sessionQuickSwitcher.active"),
                                rgb(palette.primary),
                                rgba((palette.primary << 8) | 0x1a),
                            )
                            .into_any_element(),
                            QuickSwitchBadge::Saved => status_pill(
                                t!("sessionQuickSwitcher.saved"),
                                rgb(palette.text_muted),
                                saved_badge_bg,
                            )
                            .into_any_element(),
                        };
                        div()
                            .relative()
                            .w_full()
                            .min_w_0()
                            .when(selected, |this| {
                                this.child(command_row_selection_wash(palette.primary, window))
                            })
                            .child(
                                div()
                                    .min_h(px(36.))
                                    .w_full()
                                    .min_w_0()
                                    .flex()
                                    .items_center()
                                    .gap_3()
                                    .child(
                                        div()
                                            .min_w_0()
                                            .flex_1()
                                            .flex()
                                            .flex_col()
                                            .gap_1()
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .font_weight(FontWeight(500.))
                                                    .text_color(rgb(palette.text))
                                                    .overflow_hidden()
                                                    .child(truncate_preview(&row_title, 54)),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(rgb(palette.text_muted))
                                                    .overflow_hidden()
                                                    .child(truncate_preview(&subtitle, 78)),
                                            ),
                                    )
                                    .child(badge),
                            )
                    })
            })
            .collect::<Vec<_>>();
        let list_max_height = (self.shell.viewport_size().1 * 0.55).clamp(160., 384.);

        let query_owner = cx.weak_entity();
        let select_owner = cx.weak_entity();
        let confirm_owner = cx.weak_entity();
        let confirm_items = Arc::clone(&items);
        let cancel_owner = cx.weak_entity();
        let footer_owner = cx.weak_entity();
        let empty_message = if catalog_empty {
            t!("sessionQuickSwitcher.noSessions")
        } else {
            t!("sessionQuickSwitcher.noMatches")
        };

        let command = NyaCommand::new(&command_state)
            .items(command_items)
            .filterable(false)
            .bordered(false)
            .placeholder(t!("sessionQuickSwitcher.searchPlaceholder"))
            .max_h(px(list_max_height))
            .empty(move |_, _| {
                div()
                    .h(px(120.))
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_xs()
                    .text_color(rgb(palette.text_muted))
                    .child(empty_message.clone())
            })
            .on_query(move |_, _, cx| {
                let _ = query_owner.update(cx, |this, cx| {
                    this.mark_user_activity();
                    cx.notify();
                });
            })
            .on_select(move |_, _, cx| {
                // The wash is captured when this overlay renders. Arrow keys and
                // hover move the command selection without otherwise notifying
                // the shell, so rebuild once that selection has changed.
                let _ = select_owner.update(cx, |this, cx| {
                    this.mark_user_activity();
                    cx.notify();
                });
            })
            .on_confirm(move |index, window, cx| {
                if index.section != 0 {
                    return;
                }
                let Some(item) = confirm_items.get(index.row).cloned() else {
                    return;
                };
                let _ = confirm_owner.update(cx, |this, cx| {
                    this.select_quick_switch_item(item, window, cx);
                });
            })
            .on_cancel(move |window, cx| {
                let _ = cancel_owner.update(cx, |this, cx| {
                    this.dismiss_quick_switch(window, cx);
                });
            })
            .consume_cancel(true)
            .footer(move |_, _| {
                let footer_owner = footer_owner.clone();
                div()
                    .h(px(40.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .px_3()
                    .border_t_1()
                    .border_color(rgb(palette.border))
                    .bg(rgba(0x00000000))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(palette.text_muted))
                            .child(format!(
                                "Enter {} / Esc {}",
                                t!("sessionQuickSwitcher.open"),
                                t!("sessionQuickSwitcher.close")
                            )),
                    )
                    .child(
                        div()
                            .id("quick-switch-new-ssh")
                            .h(px(28.))
                            .px_3()
                            .flex()
                            .items_center()
                            .rounded_sm()
                            .bg(rgb(palette.primary))
                            .text_color(rgb(palette.on_primary))
                            .text_xs()
                            .cursor_pointer()
                            .hover(|this| this.bg(rgb(palette.primary_hover)))
                            .child(t!("sessionQuickSwitcher.newSsh"))
                            .on_click(move |_, window, cx| {
                                let _ = footer_owner.update(cx, |this, cx| {
                                    this.close_quick_switch(cx);
                                    this.open_connection_editor(None, None, true, window, cx);
                                });
                            }),
                    )
            });

        div()
            .id(SharedString::from("quick-switch-overlay"))
            .absolute()
            .top_0()
            .bottom_0()
            .left_0()
            .right_0()
            .bg(rgba(0x00000080))
            .flex()
            .items_start()
            .justify_center()
            .pt(px(viewport_h * 0.18))
            .on_click(cx.listener(|this, _, window, cx| {
                this.dismiss_quick_switch(window, cx);
            }))
            .child(
                div()
                    .id(SharedString::from("quick-switch-dialog"))
                    .w(px((viewport_w - 32.).clamp(1., 640.)))
                    .max_w_full()
                    .mx_4()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(palette.border))
                    .bg(self.shell_surface_color(palette.bg))
                    .shadow_lg()
                    .overflow_hidden()
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .child(command),
            )
    }
}
