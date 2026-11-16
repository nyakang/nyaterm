//! Connect-time serial device and framing questions.
//!
//! A saved serial connection may defer its port and baud rate to the moment it
//! connects: USB adapters change names when replugged (`COM7` → `COM9`,
//! `/dev/ttyUSB0` → `/dev/ttyACM0`), and one device is often talked to at several
//! baud rates. The saved values stay untouched — the answer becomes
//! `SerialConnectSelection` on the pending start.

use gpui::{Context, Window};
use nyaterm_core::models::connection::SerialConnectPrompt;
use nyaterm_core::{ConnectionType, SavedConnection};
use rust_i18n::t;

use crate::features::NyaTermApp;
use crate::features::session::SessionStartFocusTarget;
use crate::features::session::state::SerialConnectContinuation;
use crate::features::session::{SerialConnectPromptState, SerialConnectSelection};

/// Every text field owned by the dialog; resetting the prefix re-seeds them from
/// the prompt state.
const SERIAL_PROMPT_TEXT_PREFIX: &str = "serial-prompt.";

/// Dropdown value that stands for "I will type the device myself" instead of a
/// port name. It only reveals the free-text field and is never used as a device.
pub(in crate::features) const SERIAL_PROMPT_CUSTOM_PORT: &str = "__custom_port__";

impl NyaTermApp {
    /// Ask for the deferred serial parameters before starting `connection`.
    ///
    /// Returns `false` when the start is deferred, which is what keeps callers
    /// from moving focus to a terminal that does not exist yet.
    pub(in crate::features) fn open_serial_connect_prompt(
        &mut self,
        connection: SavedConnection,
        continuation: SerialConnectContinuation,
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
        let in_use = self.session.serial_ports_in_use(
            continuation.reconnect_session_id(),
            continuation.request_id(),
        );
        let port_name = if port_hint.trim().is_empty() {
            self.connection_state
                .serial_ports()
                .iter()
                .find(|port| !in_use.contains(&port.port_name))
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
                continuation,
                port_name,
                baud_rate: baud_hint.to_string(),
                custom_port,
                device_edited: false,
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
        self.request_connection_serial_ports_refresh(cx);
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
        let in_use = self.session.serial_ports_in_use(
            state.continuation.reconnect_session_id(),
            state.continuation.request_id(),
        );
        if in_use.contains(&port_name) {
            if let Some(state) = self.session.dialogs.serial_connect_prompt_mut() {
                state.error = Some("dialog.serialPortInUse");
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
        let selection = SerialConnectSelection {
            port_name,
            baud_rate: baud_rate.unwrap_or(saved_baud_rate),
        };
        match state.continuation {
            SerialConnectContinuation::Start(options) => {
                let mut options = *options;
                options.serial_selection = Some(selection);
                if self.continue_saved_connection_start(state.connection, options, cx)
                    && self.session.start_has_active_pending()
                {
                    self.focus_user_started_session_surface(
                        SessionStartFocusTarget::Terminal,
                        window,
                        cx,
                    );
                }
            }
            SerialConnectContinuation::Reconnect { session_id } => {
                self.reconnect_session_with_serial_selection(
                    session_id,
                    Some(selection),
                    window,
                    cx,
                );
            }
        }
        cx.notify();
        true
    }

    pub(in crate::features) fn seed_serial_connect_prompt_device(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        let Some(state) = self.session.dialogs.serial_connect_prompt() else {
            return;
        };
        let in_use = self.session.serial_ports_in_use(
            state.continuation.reconnect_session_id(),
            state.continuation.request_id(),
        );
        let ports = self.connection_state.serial_ports();
        if let Some(state) = self.session.dialogs.serial_connect_prompt_mut()
            && state.seed_device(ports, &in_use)
        {
            self.forget_text_inputs("serial-prompt.port-path");
            cx.notify();
        }
    }

    pub(in crate::features) fn apply_serial_prompt_port(
        &mut self,
        text: String,
        cx: &mut Context<Self>,
    ) {
        if let Some(state) = self.session.dialogs.serial_connect_prompt_mut() {
            state.set_port_text(text);
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
            let port = (value != SERIAL_PROMPT_CUSTOM_PORT).then_some(value);
            state.select_port(port);
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
            state.set_baud_text(text);
        }
        cx.notify();
    }
}
