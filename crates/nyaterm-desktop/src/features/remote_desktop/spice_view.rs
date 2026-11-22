use crate::features::NyaTermApp;
use gpui::prelude::{FluentBuilder, ParentElement, Styled};
use gpui::{Context, Div, div, px, rgb};
use nyaterm_remote_desktop::spice::keyboard::GuestShortcut;
use nyaterm_remote_desktop::spice::protocol::SpiceTransferState;
use nyaterm_ui::{NyaButton, NyaDropdownMenu, NyaMenuItem};
use rust_i18n::t;

impl NyaTermApp {
    pub(super) fn spice_toolbar(&self, id: &str, cx: &mut Context<Self>) -> Option<Div> {
        let spice = self.remote_desktop.sessions.get(id)?.spice.as_ref()?;
        let palette = self.theme_palette();
        let monitor_id = id.to_owned();
        let mute_id = id.to_owned();
        let send_id = id.to_owned();
        let cancel_id = id.to_owned();
        let focus_id = id.to_owned();
        let input_enabled = self.spice_input_enabled(id);
        let shortcuts = [
            ("Ctrl+Alt+Delete".to_owned(), GuestShortcut::SecureAttention),
            ("Alt+Tab".to_owned(), GuestShortcut::NextWindow),
            ("Alt+Shift+Tab".to_owned(), GuestShortcut::PreviousWindow),
            ("Alt+F4".to_owned(), GuestShortcut::CloseWindow),
            ("Super / Windows".to_owned(), GuestShortcut::StartMenu),
            ("Super / Windows+D".to_owned(), GuestShortcut::ShowDesktop),
            ("Super / Windows+L".to_owned(), GuestShortcut::LockDesktop),
        ];
        let key_menu: Vec<NyaMenuItem> = shortcuts
            .into_iter()
            .chain((1..=12).map(|number| {
                (
                    format!("Ctrl+Alt+F{number}"),
                    GuestShortcut::VirtualTerminal(number),
                )
            }))
            .map(|(label, shortcut)| {
                let target = id.to_owned();
                NyaMenuItem::action(label)
                    .disabled(!input_enabled)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.send_spice_shortcut(&target, shortcut);
                        this.focus_user_started_session_surface(
                            crate::features::session::SessionStartFocusTarget::RemoteDesktop,
                            window,
                            cx,
                        );
                    }))
            })
            .collect();
        let index = spice
            .monitors
            .iter()
            .position(|m| Some(m.id) == spice.selected)
            .unwrap_or(0)
            + 1;
        let count = spice.monitors.len().max(1);
        let transfer_label = spice
            .transfer
            .as_ref()
            .map(|transfer| match transfer.state {
                SpiceTransferState::Running => t!(
                    "spice.transferProgress",
                    completed = transfer.completed,
                    total = transfer.total
                )
                .to_string(),
                SpiceTransferState::Completed => t!("spice.transferCompleted").to_string(),
                SpiceTransferState::Cancelled => t!("spice.transferCancelled").to_string(),
                SpiceTransferState::Failed => t!("spice.errors.fileTransfer").to_string(),
            });
        let can_cancel = spice
            .transfer
            .as_ref()
            .is_some_and(|t| t.state == SpiceTransferState::Running);
        Some(
            div()
                .w_full()
                .min_h(px(34.))
                .flex_none()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_2()
                .px_2()
                .py_1()
                .bg(rgb(palette.surface))
                .when(self.spice_relative_input(id), |bar| {
                    if self.spice_pointer_owned(id) {
                        bar.child(
                            NyaButton::new(
                                format!("spice-release-pointer-{id}"),
                                t!("spice.releasePointer"),
                            )
                            .on_click(cx.listener(
                                |this, _, window, cx| {
                                    this.release_spice_pointer(window);
                                    cx.notify();
                                },
                            )),
                        )
                    } else {
                        bar.child(
                            div()
                                .text_xs()
                                .text_color(rgb(palette.text_muted))
                                .child(t!("spice.capturePointerHint")),
                        )
                    }
                })
                .child(
                    NyaDropdownMenu::new(format!("spice-keys-{id}"))
                        .label(t!("spice.sendKeys"))
                        .tooltip(t!("spice.sendKeysHint"))
                        .items(key_menu)
                        .on_open_change(cx.listener(move |this, open: &bool, window, cx| {
                            if *open {
                                this.release_spice_pointer(window);
                                this.release_remote_keys(&focus_id);
                            } else {
                                this.focus_user_started_session_surface(
                                    crate::features::session::SessionStartFocusTarget::RemoteDesktop,
                                    window, cx,
                                );
                            }
                        })),
                )
                .child(
                    NyaButton::new(
                        format!("spice-monitor-{id}"),
                        t!("spice.monitor", index = index, count = count),
                    )
                    .disabled(count < 2)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select_next_spice_monitor(&monitor_id, cx)
                    })),
                )
                .child(
                    NyaButton::new(
                        format!("spice-audio-{id}"),
                        if spice.muted {
                            t!("spice.unmute")
                        } else {
                            t!("spice.mute")
                        },
                    )
                    .disabled(!spice.capabilities.audio_playback)
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.toggle_spice_mute(&mute_id, cx)),
                    ),
                )
                .child(
                    NyaButton::new(format!("spice-send-{id}"), t!("spice.sendFiles"))
                        .disabled(!self.spice_files_allowed(id) || spice.file_picker_open)
                        .on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.prompt_spice_files(&send_id, cx)
                            }),
                        ),
                )
                .when_some(transfer_label, |bar, label| {
                    bar.child(
                        div()
                            .text_xs()
                            .text_color(rgb(palette.text_muted))
                            .child(label),
                    )
                })
                .when(can_cancel, |bar| {
                    bar.child(
                        NyaButton::new(format!("spice-cancel-{id}"), t!("common.cancel")).on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.cancel_spice_files(&cancel_id, cx)
                            }),
                        ),
                    )
                }),
        )
    }
}
