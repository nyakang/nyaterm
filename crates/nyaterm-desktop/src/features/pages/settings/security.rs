use rust_i18n::t;

use gpui::{Context, IntoElement, SharedString, div, prelude::*, px, rgb};
use nyaterm_ui::NyaSelectOption;

use crate::features::pages::settings::panel::SettingsPanel;
use nyaterm_ui::NyaTooltip;

use super::{
    settings_form_row, settings_form_section, settings_input_control, settings_switch,
    settings_switch_with_enabled,
};

impl SettingsPanel {
    pub(in crate::features) fn security_settings_section(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let master_password = self.settings.master_password();
        let master_password_draft = master_password.draft.expose_secret().to_owned();
        let master_password_enabled = master_password.enabled;
        let master_password_input = self
            .existing_text_input_box("settings.security.master-password", false)
            .into_any_element();
        let master_password_switch_enabled = !self.cloud_sync.settings().enabled;
        let has_stored_master_password = self.settings.summary().has_master_password;
        let _idle_minutes = self.settings.summary().idle_lock_minutes;
        let host_key_policy = match self.settings.summary().host_key_policy.as_str() {
            "strict" | "reject" => "strict",
            "accept" | "accept_new" => "accept",
            _ => "prompt",
        };

        let master_section_label = t!("settings.masterPasswordSection");
        let master_switch_label = t!("settings.masterPasswordSwitch");
        let master_switch_desc = t!("settings.masterPasswordSwitchDesc");
        let master_locked_desc = t!("settings.masterPasswordLockedByCloudSync");
        let master_set_label = t!("settings.masterPasswordIsSet");
        let master_input_label = t!(if has_stored_master_password {
            "settings.masterPasswordNew"
        } else {
            "settings.masterPassword"
        });
        let session_security_label = t!("settings.sessionSecurity");
        let startup_lock_label = t!("settings.enableStartupLock");
        let idle_lock_switch_label = t!("settings.enableIdleLock");
        let idle_lock_label = t!("settings.idleLockMinutes");
        let idle_lock_desc = t!("settings.idleLockMinutesDesc");
        let _minutes_label = t!("common.minutes");
        let host_key_label = t!("settings.hostKeyPolicy");
        let host_key_desc = t!("settings.hostKeyPolicyDesc");

        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(settings_form_section(
                palette,
                Some(master_section_label),
                None,
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(settings_form_row(
                        palette,
                        master_switch_label,
                        Some(SharedString::from(master_switch_desc)),
                        div()
                            .id("settings-master-password-switch-wrap")
                            .when(!master_password_switch_enabled, |this| {
                                this.tooltip(move |window, cx| {
                                    NyaTooltip::new(master_locked_desc.clone()).build(window, cx)
                                })
                            })
                            .child(settings_switch_with_enabled(
                                palette,
                                "settings-master-password-enabled",
                                master_password_enabled,
                                master_password_switch_enabled,
                                cx.listener(|this, _, _, cx| {
                                    this.toggle_settings_master_password(cx);
                                }),
                            )),
                    ))
                    .when(
                        has_stored_master_password && master_password_draft.is_empty(),
                        |this| {
                            this.child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(palette.text_muted))
                                    .child(master_set_label),
                            )
                        },
                    )
                    .child(settings_form_row(
                        palette,
                        master_input_label,
                        None,
                        settings_input_control(master_password_input)
                            .opacity(if master_password_enabled { 1.0 } else { 0.45 }),
                    )),
            ))
            .child(settings_form_section(
                palette,
                Some(session_security_label),
                None,
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(settings_form_row(
                        palette,
                        startup_lock_label,
                        None,
                        settings_switch(
                            palette,
                            "settings-startup-lock-enabled",
                            self.settings.summary().enable_startup_lock,
                            cx.listener(|this, _, _, cx| {
                                this.toggle_startup_lock_enabled(cx);
                            }),
                        ),
                    ))
                    .child(settings_form_row(
                        palette,
                        idle_lock_switch_label,
                        None,
                        settings_switch(
                            palette,
                            "settings-idle-lock-enabled",
                            self.settings.summary().enable_idle_lock,
                            cx.listener(|this, _, _, cx| this.toggle_idle_lock_enabled(cx)),
                        ),
                    ))
                    .when(self.settings.summary().enable_idle_lock, |this| {
                        this.child(settings_form_row(
                            palette,
                            idle_lock_label,
                            Some(SharedString::from(idle_lock_desc)),
                            self.existing_number_input_box("settings.number.idle-lock-minutes"),
                        ))
                    }),
            ))
            .child(settings_form_section(
                palette,
                None,
                None,
                self.settings_select_field(
                    "settings.security.host-key-policy",
                    host_key_label,
                    Some(SharedString::from(host_key_desc)),
                    (
                        vec![
                            NyaSelectOption::new("strict", t!("settings.hostKeyStrict")),
                            NyaSelectOption::new("prompt", t!("settings.hostKeyPrompt")),
                            NyaSelectOption::new("accept", t!("settings.hostKeyAccept")),
                        ],
                        host_key_policy,
                        false,
                    ),
                    cx,
                ),
            ))
    }
}
