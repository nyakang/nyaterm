use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use gpui::{AppContext as _, Context, IntoElement as _, Render, TestAppContext, Window, div};
use nyaterm_ui::{NyaDialogWindowExt as _, nya_root};

use super::FormSaveCompletion;
use crate::features::test_support::app_with_visible_local_session;
use crate::test_support::{TestAppCleanup, TestConfigDir};

struct Host;
impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
        div()
    }
}

#[test]
fn async_form_waits_for_save_rejects_duplicates_and_closes_on_success() {
    let root = TestConfigDir::new("nyaterm-async-form");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "async-form");
    app.update(&mut cx, |app, cx| app.sync_component_theme(cx));
    let host = cx.new(|_| Host);
    let (_, cx) = cx.add_window_view(move |window, cx| nya_root(host, window, cx));
    let completion = Rc::new(RefCell::new(None::<FormSaveCompletion>));
    let calls = Rc::new(Cell::new(0));
    let valid = Rc::new(Cell::new(false));
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            let completion = completion.clone();
            let calls = calls.clone();
            let valid = valid.clone();
            app.open_async_form_dialog(
                (
                    "Save".into(),
                    320.,
                    "Save".into(),
                    |_, _, _| div().into_any_element(),
                    move |_, done, _| {
                        calls.set(calls.get() + 1);
                        if !valid.get() {
                            return false;
                        }
                        *completion.borrow_mut() = Some(done);
                        true
                    },
                    |_, _| {},
                ),
                window,
                cx,
            );
        });
    });
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    assert_eq!(calls.get(), 1);
    assert!(completion.borrow().is_none());
    valid.set(true);
    cx.simulate_keystrokes("enter");
    cx.simulate_keystrokes("enter");
    cx.simulate_keystrokes("escape");
    assert_eq!(calls.get(), 2, "pending saves must not be submitted twice");
    cx.update(|window, cx| assert!(window.has_active_nya_dialog(cx)));
    let failed = completion.borrow_mut().take().unwrap();
    app.update(cx, |_, cx| failed(false, cx));
    cx.run_until_parked();
    cx.update(|window, cx| assert!(window.has_active_nya_dialog(cx)));
    cx.simulate_keystrokes("enter");
    assert_eq!(calls.get(), 3, "failed saves must allow retry");
    let saved = completion.borrow_mut().take().unwrap();
    app.update(cx, |_, cx| saved(true, cx));
    cx.run_until_parked();
    cx.update(|window, cx| assert!(!window.has_active_nya_dialog(cx)));
}
