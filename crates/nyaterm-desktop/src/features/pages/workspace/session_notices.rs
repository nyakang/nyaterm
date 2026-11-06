use crate::features::NyaTermApp;
use crate::features::session::notice_runtime::session_notice_text;
use crate::features::session::notices::{SessionNoticeEntry, SessionNoticeKind};
use gpui::{AnyElement, Context, IntoElement, Window, div, prelude::*, px, rgb};
use nyaterm_ui::{NyaButton, NyaButtonVariant, NyaScrollable, NyaSelectableText};
use rust_i18n::t;

impl NyaTermApp {
    pub(super) fn with_session_notice_bar(
        &mut self,
        session_id: &str,
        content: AnyElement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let notices = self.session.notices.get(session_id);
        let entries = notices
            .map(|state| state.visible().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        let Some(latest) = entries.last() else {
            return content;
        };
        let expanded = notices.is_some_and(|state| state.expanded);
        let palette = self.theme_palette();
        let id = session_id.to_string();
        let (title, detail) = session_notice_text(&latest.kind);
        let summary = if detail.is_empty() {
            title
        } else {
            format!("{title}: {}", detail.lines().next().unwrap_or_default())
        };
        let color = match latest.kind {
            SessionNoticeKind::Error(_) | SessionNoticeKind::WriteFailed { .. } => palette.danger,
            _ => palette.warning,
        };
        let mut header = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .min_w_0()
            .child(
                div()
                    .flex_1()
                    .flex_basis(px(120.))
                    .min_w_0()
                    .text_color(rgb(color))
                    .text_ellipsis()
                    .child(summary),
            );
        if self.session.is_disconnected(session_id) {
            let reconnect_id = id.clone();
            header = header.child(
                NyaButton::new(format!("notice-reconnect-{id}"), t!("tabCtx.reconnect"))
                    .small()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.reconnect_session(reconnect_id.clone(), window, cx);
                    })),
            );
        }
        let details_id = id.clone();
        header = header.child(
            NyaButton::new(
                format!("notice-details-{id}"),
                t!("sessionNotice.details", count = entries.len()),
            )
            .small()
            .variant(NyaButtonVariant::Ghost)
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                this.session.notices.toggle_expanded(&details_id);
                this.restore_notice_terminal_focus(&details_id, window, cx);
                cx.notify();
            })),
        );
        header = header.child(
            div()
                .debug_selector(|| format!("notice-dismiss-{id}"))
                .child(self.session_notice_close_button(session_id, latest, "summary", cx)),
        );
        let mut bar = div()
            .debug_selector(|| format!("notice-bar-{id}"))
            .flex_none()
            .min_w_0()
            .flex()
            .flex_col()
            .gap_1()
            .px_3()
            .py_1()
            .bg(rgb(palette.surface))
            .border_b_1()
            .border_color(rgb(palette.border))
            .text_size(px(12.))
            .text_color(rgb(palette.text))
            .child(header);
        if expanded {
            let mut details = div().min_w_0().flex().flex_col().gap_2().max_h(px(180.));
            for entry in &entries {
                let category = entry.kind.category();
                let (title, detail) = session_notice_text(&entry.kind);
                details = details.child(
                    div()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(div().flex_1().min_w_0().whitespace_normal().child(title))
                                .child(
                                    div()
                                        .text_color(rgb(palette.text_dimmed))
                                        .child(t!("sessionNotice.repeated", count = entry.count)),
                                )
                                .child(
                                    self.session_notice_close_button(
                                        session_id, entry, "detail", cx,
                                    ),
                                ),
                        )
                        .child(NyaSelectableText::new(
                            format!("notice-text-{id}-{category}"),
                            detail,
                        )),
                );
            }
            bar = bar.child(details.overflow_scrollbar());
        }
        div()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex()
            .flex_col()
            .child(bar)
            .child(div().flex_1().min_h_0().min_w_0().child(content))
            .into_any_element()
    }

    fn session_notice_close_button(
        &self,
        session_id: &str,
        entry: &SessionNoticeEntry,
        slot: &str,
        cx: &mut Context<Self>,
    ) -> NyaButton {
        let id = session_id.to_string();
        let category = entry.kind.category();
        NyaButton::new(format!("notice-close-{slot}-{id}-{category}"), "")
            .icon("icons/close.svg")
            .tooltip(t!("common.close"))
            .small()
            .variant(NyaButtonVariant::Ghost)
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                this.session.notices.dismiss(&id, category);
                this.restore_notice_terminal_focus(&id, window, cx);
                cx.notify();
            }))
    }

    fn restore_notice_terminal_focus(
        &self,
        session_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.remote_desktop.is_session(session_id) && !self.session_is_sftp_only(session_id) {
            window.focus(self.terminal.input_focus(), cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::features::NyaTermApp;
    use crate::features::session::notices::SessionNoticeKind;
    use crate::features::test_support::app_with_visible_local_session;
    use gpui::{
        AppContext as _, Context, Entity, IntoElement, MouseButton, Render, TestAppContext, Window,
        div, prelude::*,
    };
    use nyaterm_core::test_support::TestTempDir;

    struct NoticeFixture(Entity<NyaTermApp>);

    impl Render for NoticeFixture {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let content = self.0.update(cx, |app, cx| {
                app.workspace_tab_session_content("s1".into(), cx)
            });
            div().size_full().flex().child(content)
        }
    }

    #[test]
    fn notice_dismissal_preserves_terminal_output_and_restores_input_focus() {
        let root = TestTempDir::new("nyaterm-notice-ui-focus");
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        cx.update(nyaterm_ui::init_components);
        let app = app_with_visible_local_session(&mut cx, root.path(), "s1");
        app.update(&mut cx, |app, _| {
            app.publish_session_notice("s1", SessionNoticeKind::Error("fixture detail".into()));
        });
        let host_app = app.clone();
        let (_, visual) = cx.add_window_view(move |window, cx| {
            let fixture = cx.new(|_| NoticeFixture(host_app));
            nyaterm_ui::nya_root(fixture, window, cx)
        });
        visual.update(|window, cx| {
            let _ = window.draw(cx);
        });
        let close = visual.debug_bounds("notice-dismiss-s1").unwrap();
        assert!(visual.debug_bounds("notice-bar-s1").is_some());
        visual.simulate_mouse_down(close.center(), MouseButton::Left, Default::default());
        visual.simulate_mouse_up(close.center(), MouseButton::Left, Default::default());
        visual.update(|window, cx| {
            let app = app.read(cx);
            assert_eq!(app.session.notices.get("s1").unwrap().visible().count(), 0);
            assert!(
                !app.terminal
                    .session_output("s1")
                    .unwrap()
                    .contains("fixture detail")
            );
            assert!(app.terminal.input_focus().is_focused(window));
        });
    }
    #[test]
    fn hidden_host_notices_stay_above_the_projected_pane_tree_with_bounded_details() {
        let root = TestTempDir::new("nyaterm-host-notice-layout");
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        cx.update(nyaterm_ui::init_components);
        let app = app_with_visible_local_session(&mut cx, root.path(), "s1");
        app.update(&mut cx, |app, _| {
            let metadata = app.session.metadata("s1").unwrap().clone();
            app.session.register_session_metadata("pane", metadata);
            app.shell.insert_workspace_pane_root(
                "s1".into(),
                crate::models::WorkspacePaneNode::leaf("pane"),
            );
            app.publish_session_notice(
                "s1",
                SessionNoticeKind::Transport(
                    nyaterm_transport::session_notice::SessionNotice::TmuxCommandFailed {
                        message: "long diagnostic\n".repeat(100),
                    },
                ),
            );
            app.session.notices.toggle_expanded("s1");
        });
        let (_, visual) = cx.add_window_view(move |window, cx| {
            let fixture = cx.new(|_| NoticeFixture(app));
            nyaterm_ui::nya_root(fixture, window, cx)
        });
        visual.update(|window, cx| {
            let _ = window.draw(cx);
        });
        let bar = visual.debug_bounds("notice-bar-s1").unwrap();
        assert!(bar.size.height <= gpui::px(240.));
        assert!(visual.debug_bounds("notice-bar-pane").is_none());
    }
}
