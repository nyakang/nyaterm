use crate::features::NyaTermApp;
use crate::features::remote_desktop::state::spice::SpiceTransferView;
use crate::models::SessionLaunchConfig;
use gpui::{Context, PathPromptOptions};
use nyaterm_remote_desktop::spice::display::plan_monitor_resize;
use nyaterm_remote_desktop::spice::keyboard::GuestShortcut;
use nyaterm_remote_desktop::spice::protocol::{SpiceControl, SpiceTransferState};

impl NyaTermApp {
    pub(in crate::features::remote_desktop) fn send_spice_shortcut(
        &mut self,
        id: &str,
        shortcut: GuestShortcut,
    ) -> bool {
        if !self.spice_input_enabled(id) {
            return false;
        }
        self.release_remote_keys(id);
        let events = shortcut.events();
        if events.is_empty() {
            return false;
        }
        self.remote_desktop
            .spice_manager
            .send(id, SpiceControl::Input { events })
            .is_ok()
    }

    pub(in crate::features::remote_desktop) fn toggle_spice_mute(
        &mut self,
        id: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(spice) = self
            .remote_desktop
            .sessions
            .get_mut(id)
            .and_then(|s| s.spice.as_mut())
        else {
            return;
        };
        let muted = !spice.muted;
        if self
            .remote_desktop
            .spice_manager
            .send(id, SpiceControl::SetMuted { muted })
            .is_ok()
        {
            spice.muted = muted;
            cx.notify();
        }
    }

    pub(in crate::features::remote_desktop) fn select_next_spice_monitor(
        &mut self,
        id: &str,
        cx: &mut Context<Self>,
    ) {
        self.release_remote_keys(id);
        let Some(session) = self.remote_desktop.sessions.get_mut(id) else {
            return;
        };
        let Some(spice) = session.spice.as_mut() else {
            return;
        };
        if spice.monitors.len() < 2 {
            return;
        }
        let index = spice
            .monitors
            .iter()
            .position(|m| Some(m.id) == spice.selected)
            .unwrap_or(0);
        spice.selected = Some(spice.monitors[(index + 1) % spice.monitors.len()].id);
        spice.displayed = None;
        spice.pending_resize = None;
        spice.last_resize = None;
        session.last_pointer = None;
        session.cursor_visible = false;
        session.cursor_shape = None;
        if let Some(texture) = session.cursor_texture.take() {
            self.remote_desktop.pending_texture_removals.push(texture);
        }
        self.remote_desktop.spice_manager.configure_presentation(
            id,
            spice.visible == Some(true),
            spice.selected,
            std::time::Duration::from_millis(16),
        );
        cx.notify();
    }

    pub(in crate::features::remote_desktop) fn prompt_spice_files(
        &mut self,
        id: &str,
        cx: &mut Context<Self>,
    ) {
        if !self.spice_files_allowed(id) {
            return;
        }
        let Some(session) = self.remote_desktop.sessions.get_mut(id) else {
            return;
        };
        let Some(spice) = session.spice.as_mut() else {
            return;
        };
        if spice.file_picker_open {
            return;
        }
        spice.file_picker_open = true;
        let token = session.start_token.clone();
        let id = id.to_owned();
        let picker = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some(rust_i18n::t!("spice.sendFiles").into()),
        });
        cx.spawn(async move |this, cx| {
            let paths = picker.await;
            let _ = this.update(cx, |this, cx| {
                if !this.remote_desktop.start_matches(&id, &token) {
                    return;
                }
                if let Some(spice) = this
                    .remote_desktop
                    .sessions
                    .get_mut(&id)
                    .and_then(|s| s.spice.as_mut())
                {
                    spice.file_picker_open = false;
                }
                let Ok(Ok(Some(paths))) = paths else {
                    cx.notify();
                    return;
                };
                if !this.spice_files_allowed(&id) {
                    cx.notify();
                    return;
                }
                let paths = paths
                    .iter()
                    .map(|path| {
                        path.to_str()
                            .filter(|_| path.is_absolute())
                            .map(nyaterm_core::SecretString::from)
                    })
                    .collect::<Option<Vec<_>>>();
                let Some(paths) = paths.filter(|paths| !paths.is_empty() && paths.len() <= 128)
                else {
                    this.shell
                        .set_status(rust_i18n::t!("spice.invalidFiles").to_string());
                    cx.notify();
                    return;
                };
                let spice = this
                    .remote_desktop
                    .sessions
                    .get_mut(&id)
                    .and_then(|s| s.spice.as_mut())
                    .expect("checked SPICE session");
                spice.next_transfer_id = spice.next_transfer_id.saturating_add(1);
                let transfer_id = spice.next_transfer_id;
                match this
                    .remote_desktop
                    .spice_manager
                    .send(&id, SpiceControl::SendFiles { transfer_id, paths })
                {
                    Ok(()) => {
                        spice.transfer = Some(SpiceTransferView {
                            id: transfer_id,
                            completed: 0,
                            total: 0,
                            state: SpiceTransferState::Running,
                        })
                    }
                    Err(error) => this.shell.set_status(super::spice_error(error).message),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(in crate::features::remote_desktop) fn spice_files_allowed(&self, id: &str) -> bool {
        self.spice_input_enabled(id)
            && self.session.metadata(id).is_some_and(|m| matches!(&m.launch_config, SessionLaunchConfig::Spice(config) if config.session.settings.file_transfer))
            && self.remote_desktop.sessions.get(id).and_then(|s| s.spice.as_ref()).is_some_and(|s| s.capabilities.file_transfer && s.transfer.as_ref().is_none_or(|t| t.state != SpiceTransferState::Running))
    }

    pub(in crate::features::remote_desktop) fn cancel_spice_files(
        &mut self,
        id: &str,
        cx: &mut Context<Self>,
    ) {
        let transfer = self
            .remote_desktop
            .sessions
            .get(id)
            .and_then(|s| s.spice.as_ref())
            .and_then(|s| s.transfer.as_ref());
        if let Some(transfer) = transfer
            && transfer.state == SpiceTransferState::Running
        {
            let _ = self.remote_desktop.spice_manager.send(
                id,
                SpiceControl::CancelTransfer {
                    transfer_id: transfer.id,
                },
            );
        }
        cx.notify();
    }

    pub(in crate::features::remote_desktop) fn queue_spice_resize(
        &mut self,
        id: &str,
        width: u32,
        height: u32,
    ) {
        if !self.session.metadata(id).is_some_and(|m| matches!(&m.launch_config, SessionLaunchConfig::Spice(config) if config.session.settings.dynamic_resolution && !config.session.settings.view_only)) { return; }
        let Some(spice) = self
            .remote_desktop
            .sessions
            .get_mut(id)
            .and_then(|s| s.spice.as_mut())
        else {
            return;
        };
        let Some(display) = spice.displayed.as_ref() else {
            return;
        };
        if !spice.capabilities.monitor_config
            || spice.last_resize == Some((display.id, width, height))
            || (display.width == width && display.height == height)
        {
            return;
        }
        // Repeated layout notifications for the same size must not reset the timer.
        if spice
            .pending_resize
            .is_none_or(|(w, h, _)| (w, h) != (width, height))
        {
            spice.pending_resize = Some((width, height, std::time::Instant::now()));
        }
    }

    pub(in crate::features::remote_desktop) fn drive_spice_resize(&mut self) {
        for (id, session) in &mut self.remote_desktop.sessions {
            let Some(spice) = session.spice.as_mut() else {
                continue;
            };
            let Some((width, height, since)) = spice.pending_resize else {
                continue;
            };
            if since.elapsed() < std::time::Duration::from_millis(150)
                || !spice.capabilities.monitor_config
            {
                continue;
            }
            spice.pending_resize = None;
            let Some(selected) = spice.displayed.as_ref() else {
                continue;
            };
            let Ok(monitors) = plan_monitor_resize(&spice.monitors, selected, width, height) else {
                continue;
            };
            if self
                .remote_desktop
                .spice_manager
                .send(id, SpiceControl::Resize { monitors })
                .is_ok()
            {
                spice.last_resize = Some((selected.id, width, height));
            }
        }
    }
}
