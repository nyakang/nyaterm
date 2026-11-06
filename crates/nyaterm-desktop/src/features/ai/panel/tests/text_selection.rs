use std::path::Path;
use std::sync::Arc;

use gpui::{
    AppContext as _, Entity, IntoElement, MouseButton, ParentElement as _, Render, Styled as _,
    TestAppContext, VisualTestContext, div, point, px,
};
use nyaterm_core::{AiMessage, AiMessageRole};
use nyaterm_ui::text::NyaTextState;

use super::{compact_app, compact_draw, edit_panel_snapshot, transcript_message};
use crate::features::NyaTermApp;
use crate::features::ai::panel::texts::TextOwner;
use crate::test_support::TestConfigDir;

struct OffsetAiHost {
    panel: Entity<crate::features::ai::panel::AiPanel>,
}

impl Render for OffsetAiHost {
    fn render(&mut self, _: &mut gpui::Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
        div()
            .w(px(900.))
            .h(px(700.))
            .flex()
            .child(div().w(px(500.)).flex_none())
            .child(div().w(px(320.)).h_full().child(self.panel.clone()))
    }
}

fn hosted<'a>(
    cx: &'a mut TestAppContext,
    root: &Path,
) -> (Entity<NyaTermApp>, &'a mut VisualTestContext) {
    cx.update(nyaterm_ui::init_components);
    let app = compact_app(cx, root);
    let panel = app.read_with(cx, |app, _| app.ai_panel.clone());
    let (_, vcx) = cx.add_window_view(move |window, cx| {
        let host = cx.new(|_| OffsetAiHost { panel });
        nyaterm_ui::nya_root(host, window, cx)
    });
    let vcx: &mut VisualTestContext = vcx;
    compact_draw(&app, vcx);
    (app, vcx)
}

fn load_message(app: &Entity<NyaTermApp>, vcx: &mut VisualTestContext, text: &str) {
    vcx.update(|_, cx| {
        app.update(cx, |app, cx| {
            let session = app.ai.chat_session_id().to_string();
            let job = app.ai.begin_history_operation("fixture").unwrap();
            let mut message: AiMessage = (*transcript_message(0)).clone();
            message.session_id = session.clone();
            message.content = text.into();
            assert!(app.ai.finish_history_message_load(
                job,
                &session,
                session.clone(),
                Ok(vec![message]),
                "fixture".into()
            ));
            app.flush_ai_panel_snapshot(cx);
        })
    });
    compact_draw(app, vcx);
}

fn message_state(app: &Entity<NyaTermApp>, vcx: &mut VisualTestContext) -> NyaTextState {
    vcx.update(|_, cx| {
        app.read(cx)
            .ai_panel
            .read(cx)
            .text_state(TextOwner::Message("message-0".into()), "answer")
            .unwrap()
    })
}

fn open_menu(app: &Entity<NyaTermApp>, vcx: &mut VisualTestContext) {
    let text = vcx
        .debug_bounds("ai-text-Message(\"message-0\")-answer")
        .unwrap();
    assert!(text.left() >= px(500.));
    let position = point(text.right() - px(1.), text.top() + px(8.));
    vcx.simulate_mouse_down(position, MouseButton::Right, Default::default());
    vcx.simulate_mouse_up(position, MouseButton::Right, Default::default());
    compact_draw(app, vcx);
    vcx.update(|window, cx| assert!(nyaterm_ui::nya_popup_is_active(window, cx)));
}

#[gpui::test]
fn menu_disables_actions_without_selection_and_copy_uses_opening_snapshot(cx: &mut TestAppContext) {
    let root = TestConfigDir::new("nyaterm-ai-text-menu");
    let _cleanup = crate::test_support::TestAppCleanup::new(cx);
    let (app, vcx) = hosted(cx, root.path());
    let source = "\n  中文 😀\n\tsecond line  \n";
    load_message(&app, vcx, source);
    vcx.update(|_, cx| cx.write_to_clipboard(gpui::ClipboardItem::new_string("unchanged".into())));
    open_menu(&app, vcx);
    vcx.simulate_keystrokes("down down enter");
    compact_draw(&app, vcx);
    vcx.update(|_, cx| {
        assert_eq!(
            cx.read_from_clipboard().unwrap().text().unwrap(),
            "unchanged"
        );
        assert_eq!(app.read(cx).ai.chat_quote(), None);
    });
    vcx.simulate_keystrokes("escape");
    compact_draw(&app, vcx);
    vcx.update(|window, cx| assert!(!nyaterm_ui::nya_popup_is_active(window, cx)));
    let state = message_state(&app, vcx);
    vcx.update(|window, cx| {
        state.select_all(cx);
        state.focus(window, cx);
    });
    compact_draw(&app, vcx);
    open_menu(&app, vcx);
    vcx.update(|_, cx| state.set_text("replacement", cx));
    compact_draw(&app, vcx);
    vcx.simulate_keystrokes("down down enter");
    compact_draw(&app, vcx);
    vcx.update(|_, cx| assert_eq!(cx.read_from_clipboard().unwrap().text().unwrap(), source));
    // A fresh copy retains its live selection after dismissing the popup.
    vcx.update(|_, cx| state.set_text(source, cx));
    compact_draw(&app, vcx);
    vcx.update(|window, cx| {
        state.select_all(cx);
        state.focus(window, cx);
    });
    compact_draw(&app, vcx);
    open_menu(&app, vcx);
    vcx.simulate_keystrokes("down down enter");
    compact_draw(&app, vcx);
    vcx.update(|_, cx| assert_eq!(state.selected_text(cx), source));
}

#[gpui::test]
fn quote_trims_snapshot_and_leaves_focus_on_ai_prompt(cx: &mut TestAppContext) {
    let root = TestConfigDir::new("nyaterm-ai-text-quote");
    let _cleanup = crate::test_support::TestAppCleanup::new(cx);
    let (app, vcx) = hosted(cx, root.path());
    load_message(&app, vcx, "  first\n  second  \n");
    let state = message_state(&app, vcx);
    vcx.update(|window, cx| {
        state.select_all(cx);
        state.focus(window, cx);
    });
    compact_draw(&app, vcx);
    open_menu(&app, vcx);
    vcx.simulate_keystrokes("down enter");
    compact_draw(&app, vcx);
    vcx.update(|window, cx| {
        assert_eq!(app.read(cx).ai.chat_quote(), Some("first\n  second"));
        let input = app.read(cx).existing_text_input("ai.chat.prompt").unwrap();
        assert_eq!(
            window.focused(cx),
            Some(input.read(cx).component_focus_handle(cx))
        );
        assert!(!nyaterm_ui::nya_popup_is_active(window, cx));
    });
}

#[gpui::test]
fn streaming_reuses_text_state_and_switching_conversation_releases_selection(
    cx: &mut TestAppContext,
) {
    let root = TestConfigDir::new("nyaterm-ai-text-state");
    let _cleanup = crate::test_support::TestAppCleanup::new(cx);
    let (app, vcx) = hosted(cx, root.path());
    load_message(&app, vcx, "first");
    let state = message_state(&app, vcx);
    let bounds = vcx
        .debug_bounds("ai-text-Message(\"message-0\")-answer")
        .unwrap();
    let start = point(bounds.left(), bounds.top() + px(6.));
    let end = point(bounds.right() - px(1.), bounds.top() + px(6.));
    vcx.simulate_mouse_down(start, MouseButton::Left, Default::default());
    vcx.simulate_mouse_move(end, Some(MouseButton::Left), Default::default());
    vcx.simulate_mouse_up(end, MouseButton::Left, Default::default());
    compact_draw(&app, vcx);
    edit_panel_snapshot(&app, vcx, |snapshot| {
        let mut message = (*snapshot.messages[0]).clone();
        message.content.push_str("\nsecond");
        snapshot.messages = vec![Arc::new(message)].into();
    });
    vcx.update(|window, cx| {
        assert_eq!(state.selected_text(cx), "first");
        let scope = app.read(cx).ai_panel.read(cx).texts.scope.clone();
        assert_eq!(scope.selected_text(window, cx), "first");
    });
    vcx.simulate_keystrokes("secondary-c");
    vcx.update(|_, cx| {
        assert_eq!(cx.read_from_clipboard().unwrap().text().unwrap(), "first");
    });
    edit_panel_snapshot(&app, vcx, |snapshot| {
        let mut message = (*snapshot.messages[0]).clone();
        message.content = "replacement".into();
        snapshot.messages = vec![Arc::new(message)].into();
    });
    vcx.update(|_, cx| assert_eq!(state.selected_text(cx), ""));
    let current = message_state(&app, vcx);
    vcx.update(|_, cx| current.select_all(cx));
    edit_panel_snapshot(&app, vcx, |snapshot| {
        snapshot.current_ai_session_id = "another-session".into()
    });
    vcx.update(|window, cx| {
        assert_eq!(current.selected_text(cx), "");
        let scope = app.read(cx).ai_panel.read(cx).texts.scope.clone();
        assert_eq!(scope.selected_text(window, cx), "");
    });
    edit_panel_snapshot(&app, vcx, |snapshot| snapshot.messages = Vec::new().into());
    vcx.update(|_, cx| {
        assert!(
            app.read(cx)
                .ai_panel
                .read(cx)
                .text_state(TextOwner::Message("message-0".into()), "answer")
                .is_none()
        )
    });
}

#[gpui::test]
fn collapsing_reasoning_clears_only_its_selection(cx: &mut TestAppContext) {
    let root = TestConfigDir::new("nyaterm-ai-text-collapse");
    let _cleanup = crate::test_support::TestAppCleanup::new(cx);
    let (app, vcx) = hosted(cx, root.path());
    edit_panel_snapshot(&app, vcx, |snapshot| {
        let mut message = (*transcript_message(0)).clone();
        message.role = AiMessageRole::Assistant;
        message.content = "answer".into();
        message.reasoning_content = Some("reasoning".into());
        snapshot.messages = vec![Arc::new(message)].into();
        snapshot.expanded_message_thoughts = vec!["message-0".into()].into();
    });
    let thought = vcx.update(|_, cx| {
        app.read(cx)
            .ai_panel
            .read(cx)
            .text_state(TextOwner::Message("message-0".into()), "thought")
            .unwrap()
    });
    vcx.update(|_, cx| thought.select_all(cx));
    edit_panel_snapshot(&app, vcx, |snapshot| {
        snapshot.expanded_message_thoughts = Default::default()
    });
    vcx.update(|_, cx| assert_eq!(thought.selected_text(cx), ""));
}
