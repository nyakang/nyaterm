use crate::details::ConnectionDetailRow;
use gpui::{
    ClipboardItem, IntoElement, RenderOnce, SharedString, Window, div, prelude::*, px, rgb,
};
use nyaterm_ui::NyaIconButton;
use nyaterm_ui::notification::{NyaNotificationKind, NyaNotificationWindowExt as _};
use rust_i18n::t;
use std::sync::Arc;
/// Interactive label/value card shown after hovering a saved connection.
///
/// `NyaHoverCard` keeps this content open while the pointer crosses from the
/// connection name, so copy buttons remain reachable just like Tauri's
/// hoverable TooltipContent.
#[derive(IntoElement)]
pub struct ConnectionDetailsHoverCard {
    connection_id: String,
    rows: Arc<[ConnectionDetailRow]>,
    share_text: String,
    palette: nyaterm_ui::ThemePalette,
}

impl ConnectionDetailsHoverCard {
    pub fn new(
        connection_id: String,
        rows: Arc<[ConnectionDetailRow]>,
        share_text: String,
        palette: nyaterm_ui::ThemePalette,
    ) -> Self {
        Self {
            connection_id,
            rows,
            share_text,
            palette,
        }
    }
}

impl RenderOnce for ConnectionDetailsHoverCard {
    fn render(self, _: &mut Window, _: &mut gpui::App) -> impl IntoElement {
        let mut grid = div().flex().flex_col().gap(px(6.));
        for (index, row) in self.rows.iter().enumerate() {
            // connection_detail_rows always places the description last.
            let is_description = index + 1 == self.rows.len();
            let value = div()
                .min_w_0()
                .flex_1()
                .when(is_description, |this| {
                    let selector = format!("connection-detail-description-{}", self.connection_id);
                    this.debug_selector(move || selector.clone())
                        .whitespace_normal()
                        .line_height(px(16.))
                        .line_clamp(3)
                        .text_ellipsis()
                })
                .text_size(px(11.))
                .text_color(rgb(self.palette.text))
                .when(!is_description && row.multiline, |this| {
                    this.whitespace_normal()
                })
                .when(!is_description && !row.multiline, |this| {
                    this.whitespace_nowrap().line_clamp(1).text_ellipsis()
                })
                .child(row.value.clone());
            let copy = row.copy_value.clone().map(|copy_value| {
                let copy_id = format!("connection-detail-copy-{}-{index}", self.connection_id);
                let copy_selector = copy_id.clone();
                div()
                    .id(SharedString::from(copy_id.clone()))
                    .debug_selector(move || copy_selector.clone())
                    .size(px(20.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        NyaIconButton::new(format!("{copy_id}-button"), "icons/copy.svg")
                            .icon_size(px(12.))
                            .tooltip(t!("savedConnections.copy").to_string())
                            .on_click(move |_, window, cx| {
                                cx.stop_propagation();
                                cx.write_to_clipboard(ClipboardItem::new_string(
                                    copy_value.clone(),
                                ));
                                window.notify_operation(
                                    "copy-connection-detail",
                                    NyaNotificationKind::Success,
                                    t!("common.copied").to_string(),
                                    cx,
                                );
                            }),
                    )
            });
            let action = if is_description {
                let share_id = format!("connection-detail-share-{}", self.connection_id);
                let share_selector = share_id.clone();
                let share_text = self.share_text.clone();
                div()
                    .id(SharedString::from(share_id.clone()))
                    .debug_selector(move || share_selector.clone())
                    .self_center()
                    .size(px(20.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        NyaIconButton::new(format!("{share_id}-button"), "icons/share.svg")
                            .icon_size(px(12.))
                            .size(px(20.))
                            .tooltip(t!("savedConnections.share").to_string())
                            .on_click(move |_, window, cx| {
                                cx.stop_propagation();
                                cx.write_to_clipboard(ClipboardItem::new_string(
                                    share_text.clone(),
                                ));
                                window.notify_operation(
                                    "share-connection-detail",
                                    NyaNotificationKind::Success,
                                    t!("common.copied").to_string(),
                                    cx,
                                );
                            }),
                    )
                    .into_any_element()
            } else {
                div()
                    .size(px(20.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .children(copy)
                    .into_any_element()
            };
            let label = div()
                .text_size(px(11.))
                .text_color(rgb(self.palette.text_dimmed))
                .child(row.label.clone());
            grid = grid.child(
                div()
                    .min_w_0()
                    .flex()
                    .items_start()
                    .gap_2()
                    .child(label.w(px(64.)).flex_none())
                    .child(value)
                    .child(action),
            );
        }

        let content_selector = format!(
            "connection-details-hover-card-content-{}",
            self.connection_id
        );
        div()
            .id(SharedString::from(content_selector.clone()))
            .debug_selector(move || content_selector.clone())
            .w(px(220.))
            .rounded_md()
            .border_1()
            .border_color(rgb(self.palette.border))
            .bg(rgb(self.palette.surface_elevated))
            .shadow_lg()
            .px_2()
            .py(px(6.))
            .child(grid)
    }
}

#[cfg(test)]
mod hover_card_tests {
    use std::{cell::Cell, rc::Rc, sync::Arc, time::Duration};

    use gpui::{
        AppContext as _, ClipboardItem, Context, InteractiveElement as _, KeyDownEvent, KeyUpEvent,
        Keystroke, Modifiers, ParentElement as _, Render, StatefulInteractiveElement as _,
        Styled as _, TestAppContext, Window, div, prelude::FluentBuilder as _, px,
    };
    use nyaterm_ui::{NyaHoverCard, NyaPopoverAlign, NyaPopoverPlacement};

    use nyaterm_ui::{apply_component_theme, theme_palette};

    use super::ConnectionDetailsHoverCard;
    use crate::details::ConnectionDetailRow;

    const SHARE_TEXT: &str = "Name: Production\nType: SSH\nHost: example.com\nDescription: Production server with a long description that wraps onto several lines\nSecond line\nThird line\nFourth line\nFifth line";

    struct HoverCardHarness {
        parent_clicks: Rc<Cell<usize>>,
        at_right_edge: bool,
    }

    impl Render for HoverCardHarness {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
            let rows: Arc<[ConnectionDetailRow]> = [
                ConnectionDetailRow {
                    label: "Host".to_string(),
                    value: "example.com".to_string(),
                    copy_value: Some("example.com".to_string()),
                    multiline: false,
                },
                ConnectionDetailRow {
                    label: "Description".to_string(),
                    value:
                        "Production server with a long description that wraps onto several lines\nSecond line\nThird line\nFourth line\nFifth line"
                            .to_string(),
                    copy_value: None,
                    multiline: false,
                },
            ]
            .into();
            let parent_clicks = self.parent_clicks.clone();
            // Exercise both preferred placement and flipping at the right edge.
            div()
                .id("connection-share-parent")
                .tab_group()
                .size_full()
                .flex()
                .items_center()
                .when(!self.at_right_edge, |this| this.justify_start())
                .when(self.at_right_edge, |this| this.justify_end())
                .on_click(move |_, _, _| parent_clicks.set(parent_clicks.get() + 1))
                .child(
                    NyaHoverCard::new(
                        "connection-details-hover-card-test",
                        div()
                            .debug_selector(|| "connection-details-hover-card-trigger-test".into())
                            .size(px(40.)),
                        ConnectionDetailsHoverCard::new(
                            "test".to_string(),
                            rows,
                            SHARE_TEXT.to_string(),
                            theme_palette("github-dark"),
                        ),
                    )
                    .placement(NyaPopoverPlacement::Right)
                    .align(NyaPopoverAlign::Center)
                    .offset(px(6.))
                    .appearance(false)
                    .open_delay(Duration::from_millis(10))
                    .close_delay(Duration::from_millis(120)),
                )
        }
    }

    #[gpui::test]
    fn connection_hover_card_stays_open_and_copy_and_share_buttons_work(cx: &mut TestAppContext) {
        cx.update(|cx| {
            apply_component_theme(
                theme_palette("github-dark"),
                gpui::font("Inter"),
                gpui::px(16.),
                cx,
            )
        });
        let parent_clicks = Rc::new(Cell::new(0));
        let (_, cx) = cx.add_window_view(|window, cx| {
            window.activate_window();
            let view = cx.new(|_| HoverCardHarness {
                parent_clicks: parent_clicks.clone(),
                at_right_edge: false,
            });
            nyaterm_ui::nya_root(view, window, cx)
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));

        let trigger = cx
            .debug_bounds("connection-details-hover-card-trigger-test")
            .expect("hover-card trigger");
        cx.simulate_mouse_move(trigger.center(), None, Modifiers::default());
        cx.executor().advance_clock(Duration::from_millis(10));
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            window.draw(cx).clear(cx);
        });

        let content_selector = "connection-details-hover-card-content-test";
        let content = cx
            .debug_bounds(content_selector)
            .expect("opened hover-card content");
        assert_eq!(content.size.width, px(220.));
        let description = cx
            .debug_bounds("connection-detail-description-test")
            .expect("description body");
        assert!(description.size.height > px(22.), "description must wrap");
        assert!(
            description.size.height <= px(48.),
            "description is capped at three lines: {:?}",
            description.size.height
        );
        let share = cx
            .debug_bounds("connection-detail-share-test")
            .expect("share button beside description");
        assert_eq!(description.left(), content.left() + px(9. + 64. + 8.));
        assert_eq!(description.right() + px(8.), share.left());
        assert_eq!(share.right(), content.right() - px(9.));
        assert_eq!(share.center().y, description.center().y);
        // Beside the row, not under it: a card below would cover the
        // neighbouring connections in the list.
        assert_eq!(content.left(), trigger.right() + px(6.));
        assert_eq!(content.center().y, trigger.center().y);

        cx.simulate_mouse_move(content.center(), None, Modifiers::default());
        cx.executor().advance_clock(Duration::from_millis(120));
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(
            cx.debug_bounds(content_selector).is_some(),
            "moving from the trigger into content must cancel delayed close"
        );

        cx.update(|window, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(String::new()));
            window.blur(cx);
            window.focus_next(cx);
            window.focus_next(cx);
            assert!(window.focused(cx).is_some());
            window.draw(cx).clear(cx);
        });
        let keystroke = Keystroke::parse("enter").expect("Enter keystroke");
        cx.simulate_event(KeyDownEvent {
            keystroke: keystroke.clone(),
            is_held: false,
            prefer_character_input: false,
        });
        cx.simulate_event(KeyUpEvent { keystroke });
        let keyboard_clipboard = cx.update(|_, cx| {
            cx.read_from_clipboard()
                .and_then(|item| item.text())
                .unwrap_or_default()
        });
        assert_eq!(keyboard_clipboard, SHARE_TEXT, "Enter must activate share");
        assert_eq!(parent_clicks.get(), 0);

        let copy = cx
            .debug_bounds("connection-detail-copy-test-0")
            .expect("copy button target");
        cx.simulate_click(copy.center(), Modifiers::default());
        let clipboard = cx.update(|_, cx| {
            cx.read_from_clipboard()
                .and_then(|item| item.text())
                .unwrap_or_default()
        });
        assert_eq!(clipboard, "example.com");

        let share = cx
            .debug_bounds("connection-detail-share-test")
            .expect("share button target");
        assert!(share.left() >= content.left() && share.right() <= content.right());
        assert!(share.top() >= content.top() && share.bottom() <= content.bottom());
        cx.simulate_click(share.center(), Modifiers::default());
        let clipboard = cx.update(|_, cx| {
            cx.read_from_clipboard()
                .and_then(|item| item.text())
                .unwrap_or_default()
        });
        assert_eq!(clipboard, SHARE_TEXT);
        assert_eq!(
            parent_clicks.get(),
            0,
            "copy actions must not activate the row"
        );
    }

    #[gpui::test]
    fn connection_hover_card_flips_at_right_window_edge(cx: &mut TestAppContext) {
        cx.update(|cx| {
            apply_component_theme(
                theme_palette("github-dark"),
                gpui::font("Inter"),
                px(16.),
                cx,
            )
        });
        let (_, cx) = cx.add_window_view(|window, cx| {
            window.activate_window();
            let view = cx.new(|_| HoverCardHarness {
                parent_clicks: Rc::new(Cell::new(0)),
                at_right_edge: true,
            });
            nyaterm_ui::nya_root(view, window, cx)
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let trigger = cx
            .debug_bounds("connection-details-hover-card-trigger-test")
            .expect("trigger");
        cx.simulate_mouse_move(trigger.center(), None, Modifiers::default());
        cx.executor().advance_clock(Duration::from_millis(10));
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            window.draw(cx).clear(cx);
        });
        let content = cx
            .debug_bounds("connection-details-hover-card-content-test")
            .expect("content");
        assert_eq!(content.size.width, px(220.));
        assert_eq!(content.right(), trigger.left() - px(6.));
        assert!(content.left() >= px(0.));
        assert_eq!(content.center().y, trigger.center().y);
    }
}
