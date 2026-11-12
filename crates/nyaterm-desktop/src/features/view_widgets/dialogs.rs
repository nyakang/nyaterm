use rust_i18n::t;

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use gpui::{
    AnyElement, AppContext as _, Context, Entity, ParentElement as _, Render, Subscription, Window,
    div,
};
use nyaterm_ui::{
    NyaConfirmDialog, NyaDialog, NyaDialogFooter, NyaDialogHandle, NyaDialogWindowExt,
};

use crate::features::NyaTermApp;

pub(in crate::features) type FormSaveCompletion = Box<dyn FnOnce(bool, &mut Context<NyaTermApp>)>;

type FormRenderer =
    dyn Fn(&mut NyaTermApp, &mut Window, &mut Context<NyaTermApp>) -> AnyElement + 'static;
type FormSubmitHandler =
    dyn Fn(&mut NyaTermApp, &mut Window, &mut Context<NyaTermApp>) -> bool + 'static;
type AppDialogHandler = dyn Fn(&mut NyaTermApp, &mut Context<NyaTermApp>) + 'static;
type FormBusyPredicate = dyn Fn(&NyaTermApp) -> bool + 'static;
type ConfirmHandler =
    dyn Fn(&mut NyaTermApp, &mut Window, &mut Context<NyaTermApp>) -> bool + 'static;

struct AppDialogContent {
    app: Entity<NyaTermApp>,
    renderer: Rc<FormRenderer>,
    _app_subscription: Subscription,
    _release_subscription: Option<Subscription>,
}

impl AppDialogContent {
    fn new(app: Entity<NyaTermApp>, renderer: Rc<FormRenderer>, cx: &mut Context<Self>) -> Self {
        let app_subscription = cx.observe(&app, |_, _, cx| cx.notify());
        Self {
            app,
            renderer,
            _app_subscription: app_subscription,
            _release_subscription: None,
        }
    }
}

impl Render for AppDialogContent {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        self.app
            .update(cx, |app, cx| (self.renderer)(app, window, cx))
    }
}

struct AppDialogFooter {
    app: Entity<NyaTermApp>,
    cancel_label: String,
    action_label: String,
    busy: Rc<FormBusyPredicate>,
    cancel_while_busy: bool,
    _app_subscription: Subscription,
}

impl AppDialogFooter {
    fn new(
        app: Entity<NyaTermApp>,
        cancel_label: String,
        action_label: String,
        busy: Rc<FormBusyPredicate>,
        cx: &mut Context<Self>,
    ) -> Self {
        let app_subscription = cx.observe(&app, |_, _, cx| cx.notify());
        Self {
            app,
            cancel_label,
            action_label,
            busy,
            cancel_while_busy: false,
            _app_subscription: app_subscription,
        }
    }
}

impl Render for AppDialogFooter {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        let busy = (self.busy)(self.app.read(cx));
        NyaDialogFooter::new(self.cancel_label.clone(), self.action_label.clone())
            .disabled(busy && !self.cancel_while_busy)
            .action_disabled(busy)
            .loading(busy)
            .into_element()
    }
}

impl NyaTermApp {
    pub(in crate::features) fn open_content_dialog(
        &mut self,
        title: String,
        width: f32,
        render: impl Fn(&mut NyaTermApp, &mut Window, &mut Context<NyaTermApp>) -> AnyElement + 'static,
        on_close: impl Fn(&mut NyaTermApp, &mut Context<NyaTermApp>) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let app = cx.entity();
        let content = cx.new(|cx| AppDialogContent::new(app.clone(), Rc::new(render), cx));
        let on_close = Rc::new(on_close);

        window.open_nya_dialog(cx, move |dialog, _, _| {
            let close_app = app.clone();
            let on_close = on_close.clone();
            let dialog = if title.is_empty() {
                dialog
            } else {
                dialog.title(title.clone())
            };
            dialog
                .width(width)
                .content(content.clone())
                .on_close(move |_, _, cx| {
                    close_app.update(cx, |app, cx| on_close(app, cx));
                })
        });
    }

    pub(in crate::features) fn open_form_dialog<R, S, C>(
        &mut self,
        spec: (String, f32, String, R, S, C),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) where
        R: Fn(&mut NyaTermApp, &mut Window, &mut Context<NyaTermApp>) -> AnyElement + 'static,
        S: Fn(&mut NyaTermApp, &mut Window, &mut Context<NyaTermApp>) -> bool + 'static,
        C: Fn(&mut NyaTermApp, &mut Context<NyaTermApp>) + 'static,
    {
        self.open_tracked_form_dialog(spec, window, cx);
    }

    pub(in crate::features) fn open_tracked_form_dialog<R, S, C>(
        &mut self,
        spec: (String, f32, String, R, S, C),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> NyaDialogHandle
    where
        R: Fn(&mut NyaTermApp, &mut Window, &mut Context<NyaTermApp>) -> AnyElement + 'static,
        S: Fn(&mut NyaTermApp, &mut Window, &mut Context<NyaTermApp>) -> bool + 'static,
        C: Fn(&mut NyaTermApp, &mut Context<NyaTermApp>) + 'static,
    {
        let (title, width, action_label, render, on_submit, on_cancel) = spec;
        let cancel_label = t!("common.cancel").to_string();
        let app = cx.entity();
        let content = cx
            .new(|cx| AppDialogContent::new(app.clone(), Rc::new(render) as Rc<FormRenderer>, cx));
        let on_submit = Rc::new(on_submit) as Rc<FormSubmitHandler>;
        let on_cancel = Rc::new(on_cancel) as Rc<AppDialogHandler>;

        window.open_tracked_nya_dialog(cx, move |dialog, _, _| {
            let submit_app = app.clone();
            let cancel_app = app.clone();
            let on_submit = on_submit.clone();
            let on_cancel = on_cancel.clone();
            NyaConfirmDialog::new(
                dialog.title(title.clone()).width(width),
                NyaDialogFooter::new(cancel_label.clone(), action_label.clone()),
            )
            .content(content.clone())
            .on_confirm(move |_, window, cx| {
                submit_app.update(cx, |app, cx| on_submit(app, window, cx))
            })
            .on_cancel(move |_, _, cx| {
                cancel_app.update(cx, |app, cx| on_cancel(app, cx));
                true
            })
            .into_dialog()
        })
    }

    /// Keep the form open until persistence completes, then close its own dialog.
    pub(in crate::features) fn open_async_form_dialog<R, S, C>(
        &mut self,
        spec: (String, f32, String, R, S, C),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) where
        R: Fn(&mut NyaTermApp, &mut Window, &mut Context<NyaTermApp>) -> AnyElement + 'static,
        S: Fn(&mut NyaTermApp, FormSaveCompletion, &mut Context<NyaTermApp>) -> bool + 'static,
        C: Fn(&mut NyaTermApp, &mut Context<NyaTermApp>) + 'static,
    {
        let (title, width, action_label, render, save, cancel) = spec;
        let pending = Rc::new(Cell::new(false));
        let busy = pending.clone();
        let identity = Rc::new(RefCell::new(None::<NyaDialogHandle>));
        let submit_identity = identity.clone();
        let handle = self.open_guarded_form_dialog_impl(
            (
                title,
                width,
                action_label,
                render,
                move |app, window, cx| {
                    let pending_completion = pending.clone();
                    let identity = submit_identity.borrow().clone();
                    let origin = window.window_handle();
                    let completion: FormSaveCompletion = Box::new(move |saved, cx| {
                        pending_completion.set(false);
                        cx.notify();
                        if saved {
                            // Store completions run without a window; defer closing until
                            // the app update has released its borrow.
                            cx.defer(move |cx| {
                                let _ = origin.update(cx, |_, window, cx| {
                                    if identity.as_ref().is_some_and(|id| id.is_top(window, cx)) {
                                        window.close_nya_dialog(cx);
                                    }
                                });
                            });
                        }
                    });
                    pending.set(save(app, completion, cx));
                    cx.notify();
                    false
                },
                cancel,
                move |_| busy.get(),
            ),
            false,
            window,
            cx,
        );
        *identity.borrow_mut() = Some(handle);
    }

    pub(in crate::features) fn open_guarded_form_dialog<R, S, C, B>(
        &mut self,
        spec: (String, f32, String, R, S, C, B),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) where
        R: Fn(&mut NyaTermApp, &mut Window, &mut Context<NyaTermApp>) -> AnyElement + 'static,
        S: Fn(&mut NyaTermApp, &mut Window, &mut Context<NyaTermApp>) -> bool + 'static,
        C: Fn(&mut NyaTermApp, &mut Context<NyaTermApp>) + 'static,
        B: Fn(&NyaTermApp) -> bool + 'static,
    {
        self.open_guarded_form_dialog_impl(spec, false, window, cx);
    }

    /// A modal asynchronous form whose request can be invalidated by cancelling.
    pub(in crate::features) fn open_cancellable_form_dialog<R, S, C, B>(
        &mut self,
        spec: (String, f32, String, R, S, C, B),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> NyaDialogHandle
    where
        R: Fn(&mut NyaTermApp, &mut Window, &mut Context<NyaTermApp>) -> AnyElement + 'static,
        S: Fn(&mut NyaTermApp, &mut Window, &mut Context<NyaTermApp>) -> bool + 'static,
        C: Fn(&mut NyaTermApp, &mut Context<NyaTermApp>) + 'static,
        B: Fn(&NyaTermApp) -> bool + 'static,
    {
        self.open_guarded_form_dialog_impl(spec, true, window, cx)
    }

    fn open_guarded_form_dialog_impl<R, S, C, B>(
        &mut self,
        spec: (String, f32, String, R, S, C, B),
        cancel_while_busy: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> NyaDialogHandle
    where
        R: Fn(&mut NyaTermApp, &mut Window, &mut Context<NyaTermApp>) -> AnyElement + 'static,
        S: Fn(&mut NyaTermApp, &mut Window, &mut Context<NyaTermApp>) -> bool + 'static,
        C: Fn(&mut NyaTermApp, &mut Context<NyaTermApp>) + 'static,
        B: Fn(&NyaTermApp) -> bool + 'static,
    {
        let (title, width, action_label, render, on_submit, on_cancel, busy) = spec;
        let cancel_label = t!("common.cancel").to_string();
        let app = cx.entity();
        let content = cx
            .new(|cx| AppDialogContent::new(app.clone(), Rc::new(render) as Rc<FormRenderer>, cx));
        let busy = Rc::new(busy) as Rc<FormBusyPredicate>;
        let footer = cx.new(|cx| {
            let mut footer = AppDialogFooter::new(
                app.clone(),
                cancel_label.clone(),
                action_label.clone(),
                busy.clone(),
                cx,
            );
            footer.cancel_while_busy = cancel_while_busy;
            footer
        });
        let on_submit = Rc::new(on_submit) as Rc<FormSubmitHandler>;
        let on_cancel = Rc::new(on_cancel) as Rc<AppDialogHandler>;

        if cancel_while_busy {
            let weak_app = app.downgrade();
            let on_cancel = on_cancel.clone();
            content.update(cx, |content, cx| {
                content._release_subscription = Some(cx.on_release(move |_, cx| {
                    let _ = weak_app.update(cx, |app, cx| on_cancel(app, cx));
                }));
            });
        }

        window.open_tracked_nya_dialog(cx, move |dialog, window, _| {
            let submit_app = app.clone();
            let cancel_app = app.clone();
            let submit_busy = busy.clone();
            let cancel_busy = busy.clone();
            let on_submit = on_submit.clone();
            let on_cancel = on_cancel.clone();
            let button_props = NyaDialogFooter::new(cancel_label.clone(), action_label.clone());
            let dialog_width = if cancel_while_busy {
                width
                    .min(f32::from(window.viewport_size().width) - 24.)
                    .max(1.)
            } else {
                width
            };
            NyaDialog::confirm_with_footer(
                dialog
                    .title(title.clone())
                    .width(dialog_width)
                    .overlay_closable(!cancel_while_busy)
                    .close_button(false),
                button_props,
                footer.clone(),
            )
            .content(content.clone())
            .on_ok(move |_, window, cx| {
                submit_app.update(cx, |app, cx| {
                    if submit_busy(app) {
                        false
                    } else {
                        on_submit(app, window, cx)
                    }
                })
            })
            .on_cancel(move |_, _, cx| {
                cancel_app.update(cx, |app, cx| {
                    if cancel_busy(app) && !cancel_while_busy {
                        false
                    } else {
                        on_cancel(app, cx);
                        true
                    }
                })
            })
        })
    }

    pub(in crate::features) fn open_confirm_dialog<F>(
        &mut self,
        spec: (String, String, String, bool, F),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) where
        F: Fn(&mut NyaTermApp, &mut Window, &mut Context<NyaTermApp>) -> bool + 'static,
    {
        let (title, message, action_label, danger, on_confirm) = spec;
        self.open_confirm_dialog_with_cancel(
            (title, message, action_label, danger, on_confirm, |_, _| {}),
            window,
            cx,
        );
    }

    pub(in crate::features) fn open_confirm_dialog_with_cancel<F, C>(
        &mut self,
        spec: (String, String, String, bool, F, C),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) where
        F: Fn(&mut NyaTermApp, &mut Window, &mut Context<NyaTermApp>) -> bool + 'static,
        C: Fn(&mut NyaTermApp, &mut Context<NyaTermApp>) + 'static,
    {
        let (title, message, action_label, danger, on_confirm, on_cancel) = spec;
        let cancel_label = t!("common.cancel").to_string();
        let app = cx.weak_entity();
        let on_confirm = Rc::new(on_confirm) as Rc<ConfirmHandler>;
        let on_cancel = Rc::new(on_cancel) as Rc<AppDialogHandler>;
        window.open_nya_dialog(cx, move |dialog, _, _| {
            let confirm_app = app.clone();
            let cancel_app = app.clone();
            let on_confirm = on_confirm.clone();
            let on_cancel = on_cancel.clone();
            let footer = NyaDialogFooter::new(cancel_label.clone(), action_label.clone());
            let footer = if danger { footer.danger() } else { footer };
            NyaConfirmDialog::new(dialog.title(title.clone()).width(384.), footer)
                .content(div().child(message.clone()))
                .on_confirm(move |_, window, cx| {
                    confirm_app
                        .update(cx, |app, cx| on_confirm(app, window, cx))
                        .unwrap_or(false)
                })
                .on_cancel(move |_, _, cx| {
                    let _ = cancel_app.update(cx, |app, cx| on_cancel(app, cx));
                    true
                })
                .into_dialog()
        });
    }
}

#[cfg(test)]
mod tests;
