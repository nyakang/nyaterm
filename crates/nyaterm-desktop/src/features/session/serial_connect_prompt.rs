//! Connect-time serial device and framing questions.
//!
//! A saved serial connection may defer its port and baud rate to the moment it
//! connects: USB adapters change names when replugged (`COM7` → `COM9`,
//! `/dev/ttyUSB0` → `/dev/ttyACM0`), and one device is often talked to at several
//! baud rates. The saved values stay untouched — the answer becomes
//! `SerialConnectSelection` on the pending start.

use gpui::{
    AnyElement, Context, IntoElement, ParentElement, Styled, Window, div, prelude::FluentBuilder,
    px, rgb,
};
use nyaterm_core::models::connection::SerialConnectPrompt;
use nyaterm_core::{ConnectionType, SavedConnection};
use nyaterm_ui::NyaSelectOption;
use rust_i18n::t;

use crate::features::NyaTermApp;
use crate::features::session::SessionStartFocusTarget;
use crate::features::session::{
    SavedConnectionStartOptions, SerialConnectPromptState, SerialConnectSelection,
};
use crate::features::text_inputs::TextInputSetup;

/// Every text field owned by the dialog; resetting the prefix re-seeds them from
/// the prompt state.
const SERIAL_PROMPT_TEXT_PREFIX: &str = "serial-prompt.";

/// Dropdown value that stands for "I will type the device myself" instead of a
/// port name. It only reveals the free-text field and is never used as a device.
pub(in crate::features) const SERIAL_PROMPT_CUSTOM_PORT: &str = "__custom_port__";

/// Whether the free-text device field belongs on screen.
///
/// Without a single enumerated device there is nothing to pick, so typing is the
/// only way forward; otherwise the field appears once "custom" is chosen.
fn manual_device_field_visible(device_count: usize, custom_port: bool) -> bool {
    device_count == 0 || custom_port
}

impl NyaTermApp {
    /// Ask for the deferred serial parameters before starting `connection`.
    ///
    /// Returns `false` when the start is deferred, which is what keeps callers
    /// from moving focus to a terminal that does not exist yet.
    pub(in crate::features) fn open_serial_connect_prompt(
        &mut self,
        connection: SavedConnection,
        options: SavedConnectionStartOptions,
        prompt: SerialConnectPrompt,
        port_hint: String,
        baud_hint: u32,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.session.dialogs.serial_connect_prompt().is_some() {
            self.shell
                .set_status(t!("serialPrompt.alreadyOpen").to_string());
            cx.notify();
            return false;
        }
        self.refresh_connection_serial_ports();
        let port_name = if port_hint.trim().is_empty() {
            self.connection_state
                .serial_ports()
                .iter()
                .find(|port| !self.connection_state.serial_port_in_use(&port.port_name))
                .map(|port| port.port_name.clone())
                .unwrap_or_default()
        } else {
            port_hint
        };
        // A saved device that is not enumerated any more (or a hand-written path)
        // opens straight into the free-text field instead of an empty dropdown.
        let custom_port = !self
            .connection_state
            .serial_ports()
            .iter()
            .any(|port| port.port_name == port_name);
        self.session
            .dialogs
            .begin_serial_connect_prompt(SerialConnectPromptState {
                connection,
                options,
                port_name,
                baud_rate: baud_hint.to_string(),
                custom_port,
                ask_port: prompt.port,
                ask_baud_rate: prompt.baud_rate,
                error: None,
            });
        self.forget_text_inputs(SERIAL_PROMPT_TEXT_PREFIX);
        cx.notify();

        // Dialogs need a window, and this entry point may run from a store
        // callback. Present on the next cycle through the main window.
        if self.shell.main_window().is_none() {
            self.session.dialogs.take_serial_connect_prompt();
            self.shell
                .set_status(t!("serialPrompt.noWindow").to_string());
            cx.notify();
            return false;
        }
        let app = cx.weak_entity();
        cx.defer(move |cx| {
            let _ = NyaTermApp::update_in_main_window(&app, cx, |app, window, cx| {
                app.show_serial_connect_prompt(window, cx)
            });
        });
        false
    }

    fn show_serial_connect_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.session.dialogs.serial_connect_prompt().is_none() {
            return;
        }
        self.refresh_connection_serial_ports();
        self.open_form_dialog(
            (
                t!("serialPrompt.title").to_string(),
                460.,
                t!("serialPrompt.connect").to_string(),
                |app, _, cx| app.serial_connect_prompt_content(cx),
                |app, window, cx| app.submit_serial_connect_prompt(window, cx),
                |app, cx| app.cancel_serial_connect_prompt(cx),
            ),
            window,
            cx,
        );
        cx.notify();
    }

    fn cancel_serial_connect_prompt(&mut self, cx: &mut Context<Self>) {
        if self.session.dialogs.take_serial_connect_prompt().is_some() {
            self.forget_text_inputs(SERIAL_PROMPT_TEXT_PREFIX);
            self.shell
                .set_status(t!("serialPrompt.cancelled").to_string());
        }
        cx.notify();
    }

    /// Validate and start. Returning `false` keeps the dialog open.
    pub(in crate::features) fn submit_serial_connect_prompt(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(state) = self.session.dialogs.serial_connect_prompt() else {
            return true;
        };
        let port_name = state.port_name.trim().to_string();
        let baud_text = state.baud_rate.trim().to_string();
        let port_missing = state.ask_port && port_name.is_empty();
        let baud_rate = baud_text.parse::<u32>().ok();
        // Only a field the dialog actually asked about can be wrong.
        let baud_invalid =
            state.ask_baud_rate && !baud_rate.is_some_and(|value| (1..=4_000_000).contains(&value));
        if port_missing || baud_invalid {
            let key = if port_missing {
                "serialPrompt.portRequired"
            } else {
                "serialPrompt.baudRateInvalid"
            };
            if let Some(state) = self.session.dialogs.serial_connect_prompt_mut() {
                state.error = Some(key);
            }
            cx.notify();
            return false;
        }
        let Some(state) = self.session.dialogs.take_serial_connect_prompt() else {
            return true;
        };
        self.forget_text_inputs(SERIAL_PROMPT_TEXT_PREFIX);
        // Text survives even for the field the dialog did not ask about, but the
        // saved value is the safer answer when it cannot be parsed.
        let saved_baud_rate = match &state.connection.config {
            ConnectionType::Serial { baud_rate, .. } => *baud_rate,
            _ => 0,
        };
        let mut options = state.options;
        options.serial_selection = Some(SerialConnectSelection {
            port_name,
            baud_rate: baud_rate.unwrap_or(saved_baud_rate),
        });
        if self.continue_saved_connection_start(state.connection, options, cx)
            && self.session.start_has_active_pending()
        {
            self.focus_user_started_session_surface(SessionStartFocusTarget::Terminal, window, cx);
        }
        cx.notify();
        true
    }

    pub(in crate::features) fn apply_serial_prompt_port(
        &mut self,
        text: String,
        cx: &mut Context<Self>,
    ) {
        if let Some(state) = self.session.dialogs.serial_connect_prompt_mut() {
            state.port_name = text;
            state.error = None;
        }
        cx.notify();
    }

    /// Selection from the device dropdown, including the "custom" entry.
    ///
    /// Picking a device hides the text field; picking "custom" reveals it and
    /// leaves the current value in place as the starting point to edit.
    pub(in crate::features) fn select_serial_prompt_port(
        &mut self,
        value: String,
        cx: &mut Context<Self>,
    ) {
        if let Some(state) = self.session.dialogs.serial_connect_prompt_mut() {
            state.error = None;
            state.custom_port = value == SERIAL_PROMPT_CUSTOM_PORT;
            if !state.custom_port {
                state.port_name = value;
            }
        }
        self.forget_text_inputs("serial-prompt.port-path");
        cx.notify();
    }

    pub(in crate::features) fn apply_serial_prompt_baud_rate(
        &mut self,
        text: String,
        cx: &mut Context<Self>,
    ) {
        if let Some(state) = self.session.dialogs.serial_connect_prompt_mut() {
            state.baud_rate = text.chars().filter(|ch| ch.is_ascii_digit()).collect();
            state.error = None;
        }
        cx.notify();
    }

    fn serial_connect_prompt_content(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let palette = self.theme_palette();
        let Some(state) = self.session.dialogs.serial_connect_prompt() else {
            return div().into_any_element();
        };
        let connection_name = state.connection.name.clone();
        let ask_port = state.ask_port;
        let ask_baud_rate = state.ask_baud_rate;
        let custom_port = state.custom_port;
        let port_name = state.port_name.clone();
        let baud_rate = state.baud_rate.clone();
        let error_key = state.error;
        let serial_ports = self.connection_state.serial_ports().to_vec();

        let device_options = serial_ports
            .iter()
            .map(|port| {
                let in_use = self.connection_state.serial_port_in_use(&port.port_name);
                let label = match port.detail() {
                    Some(detail) if in_use => {
                        format!(
                            "{} · {detail} · {}",
                            port.port_name,
                            t!("dialog.serialPortInUse")
                        )
                    }
                    Some(detail) => format!("{} · {detail}", port.port_name),
                    None if in_use => {
                        format!("{} · {}", port.port_name, t!("dialog.serialPortInUse"))
                    }
                    None => port.port_name.clone(),
                };
                // A busy device stays listed so the reason is visible; it cannot
                // be picked for a second session.
                NyaSelectOption::new(port.port_name.clone(), label).disabled(in_use)
            })
            .collect::<Vec<_>>();

        let mut content = div().min_w_0().flex().flex_col().gap_3().child(
            div()
                .text_xs()
                .text_color(rgb(palette.text_muted))
                .line_height(px(16.))
                .child(t!(
                    "serialPrompt.description",
                    name = connection_name.as_str()
                )),
        );

        if ask_port {
            let devices = device_options.len();
            let mut group = div().flex().flex_col().gap_1().child(
                div()
                    .text_xs()
                    .text_color(rgb(palette.text_muted))
                    .child(t!("dialog.serialPort")),
            );
            if devices == 0 {
                group = group.child(
                    div()
                        .h(px(32.))
                        .w_full()
                        .rounded_sm()
                        .border_1()
                        .border_color(rgb(palette.border))
                        .px_2()
                        .flex()
                        .items_center()
                        .text_xs()
                        .text_color(rgb(palette.text_dimmed))
                        .child(t!("dialog.noSerialPortsFound")),
                );
            } else {
                let mut choices = device_options;
                choices.push(NyaSelectOption::new(
                    SERIAL_PROMPT_CUSTOM_PORT,
                    t!("serialPrompt.customPort").to_string(),
                ));
                let selected = if custom_port {
                    SERIAL_PROMPT_CUSTOM_PORT.to_string()
                } else {
                    port_name.clone()
                };
                group = group.child(self.form_select_control(
                    "serial-prompt.port",
                    choices,
                    Some(selected),
                    false,
                    cx,
                ));
            }
            if manual_device_field_visible(devices, custom_port) {
                group = group
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(palette.text_dimmed))
                            .child(t!("serialPrompt.manualPortHint")),
                    )
                    .child(self.text_input_box(
                        "serial-prompt.port-path",
                        &port_name,
                        TextInputSetup::placeholder(t!("serialPrompt.portPathPlaceholder")),
                        cx,
                    ));
            }
            content = content.child(group);
        }

        if ask_baud_rate {
            content = content.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(palette.text_muted))
                            .child(t!("dialog.baudRate")),
                    )
                    .child(self.text_input_box(
                        "serial-prompt.baud",
                        &baud_rate,
                        TextInputSetup::placeholder(t!("serialPrompt.baudRatePlaceholder")),
                        cx,
                    )),
            );
        }

        content
            .when_some(error_key, |this, key| {
                this.child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(palette.danger))
                        .child(t!(key)),
                )
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::manual_device_field_visible;

    /// The free-text field is the fallback when nothing is enumerated, so a
    /// machine without a single device is not a dead end.
    #[test]
    fn the_free_text_field_shows_whenever_there_is_nothing_to_pick() {
        assert!(manual_device_field_visible(0, false));
        assert!(!manual_device_field_visible(3, false));
        assert!(manual_device_field_visible(3, true));
    }
}
