use gpui::{
    App, ClickEvent, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    Window, px,
};
use gpui_kit::component::{
    Disableable as _, WindowExt as _,
    button::{Button, ButtonVariant, ButtonVariants as _},
    dialog::{Dialog, DialogAction, DialogButtonProps, DialogClose, DialogFooter},
};

pub struct NyaDialog {
    inner: Dialog,
}

impl NyaDialog {
    fn from_component(inner: Dialog) -> Self {
        Self { inner }
    }

    pub fn title(mut self, title: impl IntoElement) -> Self {
        self.inner = self.inner.title(title);
        self
    }

    pub fn content(mut self, content: impl IntoElement) -> Self {
        self.inner = self.inner.child(content);
        self
    }

    pub fn width(mut self, width: f32) -> Self {
        self.inner = self.inner.width(px(width));
        self
    }

    pub fn max_width(mut self, max_width: f32) -> Self {
        self.inner = self.inner.max_w(px(max_width));
        self
    }

    pub fn overlay_closable(mut self, overlay_closable: bool) -> Self {
        self.inner = self.inner.overlay_closable(overlay_closable);
        self
    }

    pub fn close_button(mut self, close_button: bool) -> Self {
        self.inner = self.inner.close_button(close_button);
        self
    }

    pub fn keyboard(mut self, keyboard: bool) -> Self {
        self.inner = self.inner.keyboard(keyboard);
        self
    }

    pub fn confirm(mut self, footer: NyaDialogFooter) -> Self {
        self.inner = self
            .inner
            .button_props(footer.button_props())
            .footer(footer.into_footer());
        self
    }

    /// Uses the standard dialog actions while allowing a reactive footer
    /// element to render their current disabled/loading state.
    pub fn confirm_with_footer(
        mut self,
        footer: NyaDialogFooter,
        footer_element: impl IntoElement,
    ) -> Self {
        self.inner = self
            .inner
            .button_props(footer.button_props())
            .footer(footer_element);
        self
    }

    pub fn alert(mut self, action_label: impl Into<SharedString>) -> Self {
        let action_label = action_label.into();
        self.inner = self
            .inner
            .button_props(DialogButtonProps::default().ok_text(action_label.clone()))
            .footer(nya_dialog_action_footer(
                action_label,
                ButtonVariant::Primary,
            ));
        self
    }

    pub fn on_ok(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) -> bool + 'static,
    ) -> Self {
        self.inner = self.inner.on_ok(handler);
        self
    }

    pub fn on_cancel(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) -> bool + 'static,
    ) -> Self {
        self.inner = self.inner.on_cancel(handler);
        self
    }

    pub fn on_close(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.inner = self.inner.on_close(handler);
        self
    }

    fn into_component(self) -> Dialog {
        self.inner
    }
}

pub struct NyaConfirmDialog {
    dialog: NyaDialog,
}

impl NyaConfirmDialog {
    pub fn new(dialog: NyaDialog, footer: NyaDialogFooter) -> Self {
        Self {
            dialog: dialog.confirm(footer),
        }
    }

    pub fn content(mut self, content: impl IntoElement) -> Self {
        self.dialog = self.dialog.content(content);
        self
    }

    pub fn on_confirm(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) -> bool + 'static,
    ) -> Self {
        self.dialog = self.dialog.on_ok(handler);
        self
    }

    pub fn on_cancel(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) -> bool + 'static,
    ) -> Self {
        self.dialog = self.dialog.on_cancel(handler);
        self
    }

    pub fn into_dialog(self) -> NyaDialog {
        self.dialog
    }
}

pub struct NyaDialogFooter {
    cancel_label: SharedString,
    action_label: SharedString,
    danger: bool,
    disabled: bool,
    loading: bool,
}

impl NyaDialogFooter {
    pub fn new(
        cancel_label: impl Into<SharedString>,
        action_label: impl Into<SharedString>,
    ) -> Self {
        Self {
            cancel_label: cancel_label.into(),
            action_label: action_label.into(),
            danger: false,
            disabled: false,
            loading: false,
        }
    }

    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn loading(mut self, loading: bool) -> Self {
        self.loading = loading;
        self
    }

    pub fn into_element(self) -> impl IntoElement {
        self.into_footer()
    }

    fn button_props(&self) -> DialogButtonProps {
        DialogButtonProps::default()
            .cancel_text(self.cancel_label.clone())
            .ok_text(self.action_label.clone())
            .show_cancel(true)
            .ok_variant(if self.danger {
                ButtonVariant::Danger
            } else {
                ButtonVariant::Primary
            })
    }

    fn into_footer(self) -> DialogFooter {
        let action_variant = if self.danger {
            ButtonVariant::Danger
        } else {
            ButtonVariant::Primary
        };

        let cancel = Button::new("nya-dialog-cancel")
            .outline()
            .label(self.cancel_label)
            .disabled(self.disabled)
            .debug_selector(|| "nya-dialog-cancel-button".to_string());
        let action = Button::new("nya-dialog-action")
            .label(self.action_label)
            .with_variant(action_variant)
            .loading(self.loading)
            .disabled(self.disabled)
            .debug_selector(|| "nya-dialog-action-button".to_string());

        DialogFooter::new()
            .child(if self.disabled {
                cancel.into_any_element()
            } else {
                DialogClose::new().child(cancel).into_any_element()
            })
            .child(if self.disabled {
                action.into_any_element()
            } else {
                DialogAction::new().child(action).into_any_element()
            })
    }
}

fn nya_dialog_action_footer(
    action_label: SharedString,
    action_variant: ButtonVariant,
) -> DialogFooter {
    DialogFooter::new().child(nya_dialog_action(action_label, action_variant))
}

fn nya_dialog_action(action_label: SharedString, action_variant: ButtonVariant) -> DialogAction {
    DialogAction::new().child(
        Button::new("nya-dialog-action")
            .label(action_label)
            .with_variant(action_variant)
            .debug_selector(|| "nya-dialog-action-button".to_string()),
    )
}

pub trait NyaDialogWindowExt {
    fn open_nya_dialog<F>(&mut self, cx: &mut App, build: F)
    where
        F: Fn(NyaDialog, &mut Window, &mut App) -> NyaDialog + 'static;

    fn has_active_nya_dialog(&mut self, cx: &mut App) -> bool;
    fn close_nya_dialog(&mut self, cx: &mut App);
    fn close_all_nya_dialogs(&mut self, cx: &mut App);
}

impl NyaDialogWindowExt for Window {
    fn open_nya_dialog<F>(&mut self, cx: &mut App, build: F)
    where
        F: Fn(NyaDialog, &mut Window, &mut App) -> NyaDialog + 'static,
    {
        self.open_dialog(cx, move |dialog, window, cx| {
            build(NyaDialog::from_component(dialog), window, cx).into_component()
        });
    }

    fn has_active_nya_dialog(&mut self, cx: &mut App) -> bool {
        // `gpui-kit` reads its `Root` with an `expect`, so asking a window
        // that was not built with `nya_root` panics rather than answering. A
        // window with no component root has no dialog, which is the answer every
        // caller wants.
        if self.root::<crate::root::NyaRoot>().flatten().is_none() {
            return false;
        }
        self.has_active_dialog(cx)
    }

    fn close_nya_dialog(&mut self, cx: &mut App) {
        self.close_dialog(cx);
    }

    fn close_all_nya_dialogs(&mut self, cx: &mut App) {
        self.close_all_dialogs(cx);
    }
}
