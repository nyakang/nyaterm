use gpui::{Context, IntoElement, Window};
use nyaterm_ui::NyaDialogWindowExt as _;

use super::UpdateCheckKind;
use crate::features::NyaTermApp;

impl NyaTermApp {
    pub(in crate::features) fn open_update_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if window.has_active_nya_dialog(cx) {
            cx.notify();
            return;
        }
        self.open_content_dialog(
            String::new(),
            560.,
            |app, window, cx| app.update_dialog_content(window, cx).into_any_element(),
            |app, cx| {
                #[cfg(target_os = "linux")]
                app.cancel_update_password(cx);
                #[cfg(not(target_os = "linux"))]
                let _ = (app, cx);
            },
            window,
            cx,
        );
        let should_check = !matches!(
            self.update.read(cx).phase(),
            super::UpdatePhase::Available
                | super::UpdatePhase::Downloading { .. }
                | super::UpdatePhase::Ready
                | super::UpdatePhase::Checking
        );
        if should_check {
            self.start_update_check(cx);
        }
    }

    pub(in crate::features) fn close_update_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        #[cfg(target_os = "linux")]
        self.cancel_update_password(cx);
        window.close_nya_dialog(cx);
        cx.notify();
    }

    pub(in crate::features) fn start_update_check(&mut self, cx: &mut Context<Self>) {
        if let Some(controller) = self.desktop_controller.clone() {
            let _ = controller.update(cx, |controller, cx| {
                controller.start_update_check(UpdateCheckKind::Manual, cx)
            });
        }
        cx.notify();
    }
}
