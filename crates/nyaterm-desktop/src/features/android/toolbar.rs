use crate::features::NyaTermApp;
use gpui::{Context, IntoElement, Window, div, prelude::*, px, rgb};
use nyaterm_android::ipc::Control;
use nyaterm_ui::{NyaDropdownMenu, NyaIconButton, NyaMenuItem};
use rust_i18n::t;

#[derive(Clone, Copy)]
enum Action {
    Key(u32),
    Rotate,
    Fit,
    Zoom(f32),
    Paste,
    Reconnect,
    Settings,
    Panel(bool),
}
impl NyaTermApp {
    fn android_toolbar_action(
        &mut self,
        id: &str,
        action: Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            Action::Key(code) => {
                self.android_key(id, code, 0, 0, cx);
                self.android_key(id, code, 1, 0, cx);
            }
            Action::Rotate => {
                self.android.release_inputs(id);
                self.android_send(id, Control::Rotate, cx);
            }
            Action::Fit => {
                self.android.release_inputs(id);
                if let Some(screen) = self.android.screens.get_mut(id) {
                    screen.viewport = Default::default();
                    screen.bounds = None;
                    screen.layout_epoch += 1;
                }
            }
            Action::Zoom(factor) => self.android_zoom(id, factor, None, cx),
            Action::Paste => self.android_paste(id, cx),
            Action::Reconnect => self.reconnect_session(id.to_owned(), window, cx),
            Action::Settings => {
                self.open_android_screen_settings(id, window, cx);
                return;
            }
            Action::Panel(expand) => {
                self.android_send(id, Control::NotificationPanel { expand }, cx)
            }
        }
        self.focus_android_screen(id, window, cx);
        cx.notify();
    }
    pub(super) fn android_toolbar(
        &self,
        id: &str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let screen = &self.android.screens[id];
        let connected = screen.handle.is_some() && screen.error.is_none();
        let narrow = screen
            .viewport_bounds
            .is_some_and(|bounds| f32::from(bounds.size.width) < 440.);
        let mut toolbar = div()
            .id(format!("android-toolbar-{id}"))
            .flex()
            .items_center()
            .gap_1()
            .h(px(36.))
            .px_2()
            .bg(rgb(self.theme_palette().surface));
        let actions = [
            (
                "back",
                "icons/android/back.svg",
                "android.back",
                Action::Key(4),
            ),
            (
                "home",
                "icons/android/home.svg",
                "android.home",
                Action::Key(3),
            ),
            (
                "recent",
                "icons/android/recent.svg",
                "android.recent",
                Action::Key(187),
            ),
            (
                "rotate",
                "icons/menu/rotate-right.svg",
                "android.rotate",
                Action::Rotate,
            ),
            (
                "zoom-out",
                "icons/menu/zoom-out.svg",
                "android.zoomOut",
                Action::Zoom(0.8),
            ),
            (
                "zoom",
                "icons/menu/zoom-in.svg",
                "android.zoom",
                Action::Zoom(1.25),
            ),
            ("fit", "icons/android/fit.svg", "android.fit", Action::Fit),
            (
                "paste",
                "icons/android/paste.svg",
                "android.paste",
                Action::Paste,
            ),
        ];
        let mut menu = Vec::new();
        for (index, (name, icon, label, action)) in actions.into_iter().enumerate() {
            let target = id.to_owned();
            let handler = cx.listener(move |app, _: &gpui::ClickEvent, window, cx| {
                cx.stop_propagation();
                app.android_toolbar_action(&target, action, window, cx);
            });
            if narrow && (3..7).contains(&index) {
                menu.push(
                    NyaMenuItem::action(t!(label))
                        .icon(icon)
                        .disabled(!connected)
                        .on_click(handler),
                );
            } else {
                if index == 3 || index == 7 {
                    toolbar = toolbar.child(
                        div()
                            .w(px(1.))
                            .h(px(16.))
                            .mx_1()
                            .bg(rgb(self.theme_palette().border)),
                    );
                }
                toolbar = toolbar.child(
                    NyaIconButton::new(format!("android-{name}-{id}"), icon)
                        .size(px(30.))
                        .icon_size(px(18.))
                        .tooltip(if matches!(action, Action::Paste) {
                            format!("{} (Ctrl/Cmd+V)", t!(label))
                        } else {
                            t!(label).into_owned()
                        })
                        .disabled(!connected)
                        .on_click(handler),
                );
            }
        }
        let target = id.to_owned();
        toolbar = toolbar.child(div().flex_1()).child(
            NyaIconButton::new(format!("android-settings-{id}"), "icons/settings.svg")
                .size(px(30.))
                .icon_size(px(18.))
                .tooltip(t!("android.screenSettings"))
                .on_click(cx.listener(move |app, _, window, cx| {
                    app.android_toolbar_action(&target, Action::Settings, window, cx)
                })),
        );
        for (label, action) in [
            ("android.volumeUp", Action::Key(24)),
            ("android.volumeDown", Action::Key(25)),
            ("android.powerKey", Action::Key(26)),
            ("android.expandPanel", Action::Panel(true)),
            ("android.collapsePanel", Action::Panel(false)),
        ] {
            let target = id.to_owned();
            menu.push(
                NyaMenuItem::action(t!(label))
                    .disabled(!connected)
                    .on_click(cx.listener(move |app, _, window, cx| {
                        app.android_toolbar_action(&target, action, window, cx)
                    })),
            );
        }
        menu.push(NyaMenuItem::separator());
        let target = id.to_owned();
        menu.push(
            NyaMenuItem::action(t!("android.reconnect")).on_click(cx.listener(
                move |app, _, window, cx| {
                    app.android_toolbar_action(&target, Action::Reconnect, window, cx)
                },
            )),
        );
        let target = id.to_owned();
        toolbar = toolbar.child(
            NyaDropdownMenu::new(format!("android-more-{id}"))
                .icon("icons/session/more.svg")
                .icon_size(px(18.))
                .tooltip(t!("android.more"))
                .items(menu)
                .on_open_change(cx.listener(move |app, open: &bool, window, cx| {
                    if !open {
                        app.focus_android_screen(&target, window, cx);
                    }
                })),
        );
        if screen.error.is_some() {
            let target = id.to_owned();
            toolbar = toolbar.child(
                NyaIconButton::new(format!("android-reconnect-{id}"), "icons/fe/refresh.svg")
                    .size(px(30.))
                    .icon_size(px(18.))
                    .tooltip(t!("android.reconnect"))
                    .on_click(cx.listener(move |app, _, window, cx| {
                        app.android_toolbar_action(&target, Action::Reconnect, window, cx)
                    })),
            );
        }
        toolbar
    }
}
