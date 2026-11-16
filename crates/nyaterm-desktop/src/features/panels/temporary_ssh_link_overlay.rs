use rust_i18n::t;

use gpui::{
    AnyElement, Context, IntoElement as _, ParentElement as _, Styled as _, div,
    prelude::FluentBuilder as _, px, rgb,
};
use nyaterm_ui::NyaSelectOption;

use crate::features::{NyaTermApp, text_inputs::TextInputSetup};
use crate::temporary_ssh_link::{
    TemporaryLinkProtocol, build_temporary_serial_link, parse_temporary_ssh_link,
    parse_temporary_telnet_link,
};

impl NyaTermApp {
    pub(in crate::features) fn temporary_ssh_link_dialog_content(
        &mut self,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = self.theme_palette();
        let protocol = self.session.dialog_temporary_link_protocol();
        let draft = self.session.dialog_temporary_ssh_link_draft().to_string();
        let serial_port = self.session.dialog_temporary_serial_port_name().to_string();
        let serial_baud_rate = self.session.dialog_temporary_serial_baud_rate().to_string();
        let error_key =
            temporary_link_error_key(self, protocol, &draft, &serial_port, &serial_baud_rate);

        div()
            .min_w_0()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(palette.text_muted))
                    .line_height(px(16.))
                    .child(t!("temporarySsh.description")),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .min_w_0()
                    .child(div().w(px(128.)).child(self.form_select_control(
                        "temporary-link-protocol",
                        vec![
                            NyaSelectOption::new("ssh", t!("temporarySsh.protocolSsh")),
                            NyaSelectOption::new("telnet", t!("temporarySsh.protocolTelnet")),
                            NyaSelectOption::new("serial", t!("temporarySsh.protocolSerial")),
                        ],
                        Some(protocol.as_str().to_string()),
                        false,
                        cx,
                    )))
                    .child(div().min_w_0().flex_1().child(match protocol {
                        TemporaryLinkProtocol::Ssh | TemporaryLinkProtocol::Telnet => {
                            let placeholder = match protocol {
                                TemporaryLinkProtocol::Ssh => t!("temporarySsh.placeholder"),
                                TemporaryLinkProtocol::Telnet => {
                                    t!("temporarySsh.telnetPlaceholder")
                                }
                                TemporaryLinkProtocol::Serial => unreachable!(),
                            };
                            self.text_input_box(
                                "temporary-ssh.link",
                                &draft,
                                TextInputSetup::placeholder(placeholder),
                                cx,
                            )
                            .into_any_element()
                        }
                        TemporaryLinkProtocol::Serial => {
                            let serial_ports = self
                                .connection_state
                                .serial_ports()
                                .iter()
                                .map(|port| {
                                    let label = match port.detail() {
                                        Some(detail) => {
                                            format!("{} · {detail}", port.port_name)
                                        }
                                        None => port.port_name.clone(),
                                    };
                                    NyaSelectOption::new(port.port_name.clone(), label).disabled(
                                        self.connection_state.serial_port_in_use(&port.port_name),
                                    )
                                })
                                .collect::<Vec<_>>();
                            if serial_ports.is_empty() {
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
                                    .child(t!("temporarySsh.noSerialPortsFound"))
                                    .into_any_element()
                            } else {
                                self.form_select_control(
                                    "temporary-link-serial-port",
                                    serial_ports,
                                    (!serial_port.is_empty()).then_some(serial_port.clone()),
                                    false,
                                    cx,
                                )
                                .into_any_element()
                            }
                        }
                    })),
            )
            .when(protocol == TemporaryLinkProtocol::Serial, |this| {
                this.child(
                    self.text_input_box(
                        "temporary-ssh.baud-rate",
                        &serial_baud_rate,
                        TextInputSetup::placeholder(t!("temporarySsh.baudRatePlaceholder")),
                        cx,
                    )
                    .into_any_element(),
                )
            })
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

fn temporary_link_error_key(
    app: &NyaTermApp,
    protocol: TemporaryLinkProtocol,
    draft: &str,
    serial_port: &str,
    serial_baud_rate: &str,
) -> Option<&'static str> {
    app.session
        .dialog_temporary_ssh_link_error()
        .or_else(|| match protocol {
            TemporaryLinkProtocol::Ssh => {
                if draft.trim().is_empty() {
                    None
                } else {
                    parse_temporary_ssh_link(draft)
                        .as_ref()
                        .err()
                        .map(|error| error.locale_key())
                }
            }
            TemporaryLinkProtocol::Telnet => {
                if draft.trim().is_empty() {
                    None
                } else {
                    parse_temporary_telnet_link(draft)
                        .as_ref()
                        .err()
                        .map(|error| error.locale_key())
                }
            }
            TemporaryLinkProtocol::Serial => {
                if serial_port.trim().is_empty() && serial_baud_rate.trim() == "115200" {
                    None
                } else {
                    build_temporary_serial_link(serial_port, serial_baud_rate)
                        .as_ref()
                        .err()
                        .map(|error| error.locale_key())
                }
            }
        })
}
