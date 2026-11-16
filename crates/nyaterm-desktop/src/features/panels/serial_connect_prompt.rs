//! Presentation for the saved-connection serial parameter prompt.

use gpui::{
    AnyElement, Context, IntoElement, ParentElement, Styled, div, prelude::FluentBuilder, px, rgb,
};
use nyaterm_ui::NyaSelectOption;
use rust_i18n::t;

use crate::features::NyaTermApp;
use crate::features::session::serial_connect_prompt::SERIAL_PROMPT_CUSTOM_PORT;
use crate::features::text_inputs::TextInputSetup;

fn manual_device_field_visible(device_count: usize, custom_port: bool) -> bool {
    device_count == 0 || custom_port
}

impl NyaTermApp {
    pub(in crate::features) fn serial_connect_prompt_content(
        &mut self,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = self.theme_palette();
        let Some(state) = self.session.serial_connect_prompt() else {
            return div().into_any_element();
        };
        let connection_name = state.connection.name.clone();
        let ask_port = state.ask_port;
        let ask_baud_rate = state.ask_baud_rate;
        let custom_port = state.custom_port;
        let port_name = state.port_name.clone();
        let baud_rate = state.baud_rate.clone();
        let error_key = state.error;
        let in_use = self.session.serial_ports_in_use(
            state.continuation.reconnect_session_id(),
            state.continuation.request_id(),
        );
        let loading = self.connection_state.serial_ports_loading();
        let load_error = self
            .connection_state
            .serial_ports_error()
            .map(str::to_string);
        let serial_ports = self.connection_state.serial_ports().to_vec();

        let device_options = serial_ports
            .iter()
            .map(|port| {
                let in_use = in_use.contains(&port.port_name);
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
                        .child(if loading {
                            t!("dialog.loadingSerialPorts").to_string()
                        } else if let Some(error) = load_error.as_ref() {
                            format!("{}: {error}", t!("dialog.serialPortsLoadFailed"))
                        } else {
                            t!("dialog.noSerialPortsFound").to_string()
                        }),
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
