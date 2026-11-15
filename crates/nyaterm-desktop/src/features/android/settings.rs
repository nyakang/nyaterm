use crate::features::NyaTermApp;
use gpui::{Context, Entity, IntoElement, Window, div, prelude::*, px};
use nyaterm_core::models::android::{
    AndroidScreenSettings, CaptureOrientation, ScreenQualityPreset,
};
use nyaterm_ui::{
    NyaNumberInput, NyaNumberInputOptions, NyaNumberInputState, NyaSelectOption, NyaSwitch,
};
use rust_i18n::t;

pub(super) struct ScreenSettingsDraft {
    settings: AndroidScreenSettings,
    size: Entity<NyaNumberInputState>,
    fps: Entity<NyaNumberInputState>,
    rate: Entity<NyaNumberInputState>,
    invalid: bool,
}
impl ScreenSettingsDraft {
    fn read(&self, cx: &gpui::App) -> Option<AndroidScreenSettings> {
        let mut settings = self.settings;
        settings.max_size = self.size.read(cx).value(cx).parse().ok()?;
        settings.max_fps = self.fps.read(cx).value(cx).parse().ok()?;
        let rate: f64 = self.rate.read(cx).value(cx).parse().ok()?;
        if !rate.is_finite() || !(0.1..=100.).contains(&rate) {
            return None;
        }
        settings.bit_rate = (rate * 1_000_000.).round() as u32;
        settings.validate().ok()?;
        Some(settings)
    }
    fn preset(&mut self, preset: ScreenQualityPreset, cx: &mut Context<NyaTermApp>) {
        preset.apply(&mut self.settings);
        self.size.update(cx, |input, cx| {
            input.set_content(&self.settings.max_size.to_string(), cx)
        });
        self.fps.update(cx, |input, cx| {
            input.set_content(&self.settings.max_fps.to_string(), cx)
        });
        self.rate.update(cx, |input, cx| {
            input.set_content(
                &(self.settings.bit_rate as f64 / 1_000_000.).to_string(),
                cx,
            )
        });
        self.invalid = false;
    }
}
impl NyaTermApp {
    pub(in crate::features) fn android_screen_select_controls(
        &mut self,
        prefix: &str,
        current: Option<AndroidScreenSettings>,
        settings: AndroidScreenSettings,
        cx: &mut Context<Self>,
    ) -> (gpui::AnyElement, gpui::AnyElement) {
        let selected = current.and_then(|settings| {
            ScreenQualityPreset::ALL
                .into_iter()
                .find(|preset| preset.matches(settings))
        });
        let mut options = ScreenQualityPreset::ALL
            .into_iter()
            .map(|preset| NyaSelectOption::new(preset.label_key(), t!(preset.label_key())))
            .collect::<Vec<_>>();
        options.push(NyaSelectOption::new("custom", t!("android.qualityCustom")).disabled(true));
        let quality = self
            .form_select_control(
                format!("{prefix}.quality"),
                options,
                Some(
                    selected
                        .map_or("custom", ScreenQualityPreset::label_key)
                        .to_owned(),
                ),
                false,
                cx,
            )
            .into_any_element();
        let orientation = self
            .form_select_control(
                format!("{prefix}.orientation"),
                CaptureOrientation::ALL
                    .into_iter()
                    .map(|value| NyaSelectOption::new(value.server_value(), t!(value.label_key())))
                    .collect(),
                Some(settings.capture_orientation.server_value().to_owned()),
                false,
                cx,
            )
            .into_any_element();
        (quality, orientation)
    }

    pub(in crate::features) fn change_android_screen_select(
        &mut self,
        id: &str,
        value: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some((prefix, field)) = id.rsplit_once('.') else {
            return false;
        };
        let editor = prefix == "connection-editor-android";
        let session = prefix.strip_prefix("android-session-settings:");
        if !editor && session.is_none() {
            return false;
        }
        let preset = ScreenQualityPreset::ALL
            .into_iter()
            .find(|preset| preset.label_key() == value);
        let orientation = CaptureOrientation::ALL
            .into_iter()
            .find(|orientation| orientation.server_value() == value);
        if editor {
            if let Some(draft) = self.connection_state.active_editor_draft() {
                let mut settings = draft.android_screen;
                match field {
                    "quality" => {
                        if let Some(preset) = preset {
                            preset.apply(&mut settings);
                        }
                    }
                    "orientation" => {
                        if let Some(orientation) = orientation {
                            settings.capture_orientation = orientation;
                        }
                    }
                    _ => return false,
                }
                self.connection_state.set_android_screen_settings(settings);
                if field == "quality" {
                    self.connection_state.sync_editor_fields_from_draft(cx);
                }
            }
        } else if let Some(draft) = session
            .and_then(|id| self.android.screens.get_mut(id))
            .and_then(|screen| screen.settings_draft.as_mut())
        {
            match field {
                "quality" => {
                    if let Some(preset) = preset {
                        draft.preset(preset, cx);
                    }
                }
                "orientation" => {
                    if let Some(orientation) = orientation {
                        draft.settings.capture_orientation = orientation;
                    }
                }
                _ => return false,
            }
        }
        cx.notify();
        true
    }

    pub(super) fn open_android_screen_settings(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(screen) = self.android.screens.get_mut(id) else {
            return;
        };
        let settings = screen.settings;
        screen.settings_draft = Some(ScreenSettingsDraft {
            settings,
            size: cx.new(|cx| {
                NyaNumberInputState::new(
                    cx,
                    settings.max_size.to_string(),
                    NyaNumberInputOptions::default()
                        .range(128., 2560.)
                        .decimal_places(0),
                )
            }),
            fps: cx.new(|cx| {
                NyaNumberInputState::new(
                    cx,
                    settings.max_fps.to_string(),
                    NyaNumberInputOptions::default()
                        .range(1., 120.)
                        .decimal_places(0),
                )
            }),
            rate: cx.new(|cx| {
                NyaNumberInputState::new(
                    cx,
                    (settings.bit_rate as f64 / 1_000_000.).to_string(),
                    NyaNumberInputOptions::default()
                        .range(0.1, 100.)
                        .step(0.1)
                        .decimal_places(6)
                        .suffix("Mbps"),
                )
            }),
            invalid: false,
        });
        self.android.release_inputs(id);
        self.focus_android_screen(id, window, cx);
        let render_id = id.to_owned();
        let submit_id = id.to_owned();
        let cancel_id = id.to_owned();
        self.open_form_dialog(
            (
                t!("android.screenSettings").into_owned(),
                420.,
                t!("android.applyScreenSettings").into_owned(),
                move |app: &mut Self, _, cx: &mut Context<Self>| {
                    app.android_settings_content(&render_id, cx)
                },
                move |app: &mut Self, window: &mut Window, cx: &mut Context<Self>| {
                    let settings = app
                        .android
                        .screens
                        .get(&submit_id)
                        .and_then(|s| s.settings_draft.as_ref())
                        .and_then(|d| d.read(cx));
                    let Some(settings) = settings else {
                        if let Some(draft) = app
                            .android
                            .screens
                            .get_mut(&submit_id)
                            .and_then(|s| s.settings_draft.as_mut())
                        {
                            draft.invalid = true;
                        }
                        cx.notify();
                        return false;
                    };
                    let Some(mut config) = app.android_config_for_session(&submit_id) else {
                        return true;
                    };
                    config.screen = settings;
                    if let Some(metadata) = app.session.metadata_mut(&submit_id) {
                        metadata.disconnected = false;
                    }
                    app.begin_android_screen(submit_id.clone(), config, cx);
                    let target = submit_id.clone();
                    let weak = cx.weak_entity();
                    window.defer(cx, move |window, cx| {
                        let _ = weak
                            .update(cx, |app, cx| app.focus_android_screen(&target, window, cx));
                    });
                    cx.notify();
                    true
                },
                move |app: &mut Self, cx: &mut Context<Self>| {
                    if let Some(screen) = app.android.screens.get_mut(&cancel_id) {
                        screen.settings_draft = None;
                    }
                    cx.notify();
                },
            ),
            window,
            cx,
        );
    }
    fn android_settings_content(&mut self, id: &str, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(draft) = self
            .android
            .screens
            .get(id)
            .and_then(|s| s.settings_draft.as_ref())
        else {
            return div().into_any_element();
        };
        let settings = draft.settings;
        let current = draft.read(cx);
        let fields = [draft.size.clone(), draft.fps.clone(), draft.rate.clone()];
        let invalid = draft.invalid;
        let (quality, orientation) = self.android_screen_select_controls(
            &format!("android-session-settings:{id}"),
            current,
            settings,
            cx,
        );
        let mut content = div()
            .flex()
            .flex_col()
            .gap_3()
            .child(t!("android.sessionSettingsHint"))
            .child(quality);
        for (label, field) in [
            ("android.maxSize", &fields[0]),
            ("android.maxFps", &fields[1]),
            ("android.bitRate", &fields[2]),
        ] {
            content = content.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(t!(label))
                    .child(div().w_full().h(px(32.)).child(NyaNumberInput::new(field))),
            );
        }
        let target = id.to_owned();
        content = content.child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(t!("android.powerOn"))
                .child(
                    NyaSwitch::new("android-session-power")
                        .checked(settings.power_on)
                        .on_click(cx.listener(move |app, value: &bool, _, cx| {
                            if let Some(draft) = app
                                .android
                                .screens
                                .get_mut(&target)
                                .and_then(|s| s.settings_draft.as_mut())
                            {
                                draft.settings.power_on = *value;
                            }
                            cx.notify();
                        })),
                ),
        );
        content = content.child(orientation);
        if invalid {
            content = content.child(t!("android.invalidScreenSettings"));
        }
        content.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use crate::features::android::ScreenState;
    use crate::features::test_support::app_with_visible_android_session;
    use crate::test_support::{TestAppCleanup, TestConfigDir};
    use gpui::{AppContext as _, IntoElement as _, ParentElement as _, Styled as _};
    use nyaterm_ui::NyaDialogWindowExt as _;
    struct Host(gpui::Entity<crate::features::NyaTermApp>);
    impl gpui::Render for Host {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            gpui::div().size_full().child(
                self.0
                    .update(cx, |app, cx| {
                        app.android_screen_content("fixture".into(), cx)
                    })
                    .into_any_element(),
            )
        }
    }
    #[test]
    fn android_settings_select_cancel_discards_preset_and_restores_screen_focus() {
        let root = TestConfigDir::new("nyaterm-screen-draft-cancel");
        let mut cx = gpui::TestAppContext::single();
        cx.update(nyaterm_ui::init_components);
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_android_session(&mut cx, root.path(), "fixture", "/sdcard/");
        app.update(&mut cx, |app, cx| {
            app.sync_component_theme(cx);
            app.android.screens.insert(
                "fixture".into(),
                ScreenState::new(cx.focus_handle(), 1, Default::default()),
            );
        });
        let host = cx.new(|_| Host(app.clone()));
        let (_, vcx) =
            cx.add_window_view(move |window, cx| nyaterm_ui::nya_root(host.clone(), window, cx));
        vcx.update(|window, cx| {
            window.activate_window();
            let _ = window.draw(cx);
        });
        vcx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.open_android_screen_settings("fixture", window, cx);
                app.on_select_changed(
                    "android-session-settings:fixture.quality",
                    Some("android.qualitySmooth"),
                    cx,
                );
                app.on_select_changed(
                    "android-session-settings:fixture.orientation",
                    Some("@90"),
                    cx,
                );
                let screen = app.android.screens.get_mut("fixture").unwrap();
                let draft = screen.settings_draft.as_mut().unwrap();
                assert_eq!(
                    draft.settings.capture_orientation,
                    nyaterm_core::models::android::CaptureOrientation::Degrees90
                );
                assert_eq!(draft.read(cx).unwrap().max_fps, 60);
                draft.rate.update(cx, |rate, cx| rate.set_content("0", cx));
                assert!(draft.read(cx).is_none());
                assert_eq!(screen.settings.max_fps, 30);
            });
            let _ = window.draw(cx);
        });
        vcx.simulate_keystrokes("escape");
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            app.update(cx, |app, cx| {
                assert!(!window.has_active_nya_dialog(cx));
                let screen = &app.android.screens["fixture"];
                assert!(screen.settings_draft.is_none());
                assert_eq!(screen.settings.max_fps, 30);
                assert!(screen.focus.is_focused(window));
            });
        });
    }
}
