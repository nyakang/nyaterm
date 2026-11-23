use crate::features::NyaTermApp;
use crate::features::remote_desktop::state::frame_source::RemoteDesktopFrameSource;
use crate::models::SessionLaunchConfig;
use gpui::{ClipboardItem, Context, Window};
use nyaterm_remote_desktop::spice::protocol::{SpiceControl, SpiceErrorKind, SpiceSessionState};
use nyaterm_remote_desktop::spice::session::SpiceEvent;
use nyaterm_remote_desktop::{ClipboardOrigin, RemoteDesktopViewState};

impl NyaTermApp {
    pub(in crate::features::remote_desktop) fn sync_spice_presentation_visibility(
        &mut self,
        window: &Window,
    ) {
        let interval = std::time::Duration::from_millis(
            if crate::platform::is_software_renderer(self.shell.renderer()) {
                33
            } else {
                16
            },
        );
        for (id, session) in &mut self.remote_desktop.sessions {
            let Some(spice) = session.spice.as_mut() else {
                continue;
            };
            let visible = !session.uploads_stopped
                && window.is_visible()
                && self.session.active_id() == Some(id.as_str());
            if spice.visible != Some(visible)
                && self
                    .remote_desktop
                    .spice_manager
                    .send(id, SpiceControl::SetVisible { visible })
                    .is_ok()
            {
                spice.visible = Some(visible);
            }
            self.remote_desktop.spice_manager.configure_presentation(
                id,
                visible,
                spice.selected,
                interval,
            );
            if visible && session.texture_dirty {
                self.remote_desktop
                    .spice_manager
                    .invalidate_presentation(id);
                session.texture_dirty = false;
            }
        }
    }

    pub(in crate::features::remote_desktop) fn drain_spice_events(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let events = self.remote_desktop.spice_manager.drain(id, 128);
        let presentation = self.remote_desktop.spice_manager.take_presentation(id);
        if events.is_empty() && presentation.is_none() {
            return false;
        }
        for event in events {
            match event {
                SpiceEvent::Control(control) => self.apply_spice_control(id, control, cx),
                SpiceEvent::Cursor { channel, event } => {
                    let Some(spice) = self
                        .remote_desktop
                        .sessions
                        .get_mut(id)
                        .and_then(|s| s.spice.as_mut())
                    else {
                        continue;
                    };
                    if spice.cursors.apply(channel, event).is_err() {
                        self.fail_spice_session(id, SpiceErrorKind::ResourceLimit, cx);
                        break;
                    }
                    if let Some(display) = spice.displayed.as_ref()
                        && display.id.channel == channel
                    {
                        let events = spice.cursors.for_monitor(display);
                        self.apply_remote_cursor_batch(id, events, window);
                    }
                }
                // This manager is constructed with the presentation pipeline.
                SpiceEvent::Frame { .. } => {
                    self.fail_spice_session(id, SpiceErrorKind::Protocol, cx)
                }
            }
        }
        if let Some(presentation) = presentation {
            self.apply_spice_presentation(id, presentation, window, cx);
        }
        self.reconcile_spice_pointer(window, cx);
        true
    }

    fn apply_spice_presentation(
        &mut self,
        id: &str,
        presentation: nyaterm_remote_desktop::spice::presentation::SpicePresentation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !window.is_visible() || self.session.active_id() != Some(id) {
            return;
        }
        let Some(session) = self.remote_desktop.sessions.get_mut(id) else {
            return;
        };
        if session.uploads_stopped
            || matches!(
                session.state,
                RemoteDesktopViewState::Failed | RemoteDesktopViewState::Disconnected
            )
        {
            return;
        }
        // A frame prepared before a monitor switch must never restore old input
        // coordinates or acknowledge pixels for the newly selected monitor.
        if session
            .spice
            .as_ref()
            .and_then(|s| s.selected)
            .is_some_and(|selected| selected != presentation.display.id)
        {
            return;
        }
        let incoming = presentation.id();
        if let Some(RemoteDesktopFrameSource::Spice { id, .. }) = session.framebuffer.as_ref()
            && (
                incoming.frame.generation,
                incoming.frame.epoch,
                incoming.frame.sequence,
            ) < (id.frame.generation, id.frame.epoch, id.frame.sequence)
        {
            return;
        }
        match crate::features::remote_desktop::runtime::framebuffer::upload_prepared_frame(
            window,
            &mut session.texture,
            &presentation.frame,
        ) {
            Ok(true) => {
                session.framebuffer = Some(RemoteDesktopFrameSource::Spice {
                    width: presentation.frame.width,
                    height: presentation.frame.height,
                    id: incoming,
                });
                session.texture_dirty = false;
                session.reconnect_attempts = 0;
                session.reconnect_at = None;
                let cursor_events = if let Some(spice) = session.spice.as_mut() {
                    let changed = spice.displayed.as_ref() != Some(&presentation.display);
                    let events = changed.then(|| spice.cursors.for_monitor(&presentation.display));
                    spice.displayed = Some(presentation.display);
                    if changed {
                        session.cursor_shape = None;
                        if let Some(texture) = session.cursor_texture.take() {
                            window.remove_dynamic_texture(texture);
                        }
                    }
                    events
                } else {
                    None
                };
                if let Some(events) = cursor_events {
                    self.apply_remote_cursor_batch(id, events, window);
                }
                self.remote_desktop
                    .spice_manager
                    .acknowledge_presentation(id, incoming);
            }
            Ok(false) => self
                .remote_desktop
                .spice_manager
                .invalidate_presentation(id),
            Err(_) => {
                session.uploads_stopped = true;
                self.fail_spice_session(id, SpiceErrorKind::ResourceLimit, cx);
            }
        }
    }

    fn apply_spice_control(&mut self, id: &str, control: SpiceControl, cx: &mut Context<Self>) {
        let Some(session) = self.remote_desktop.sessions.get_mut(id) else {
            return;
        };
        match control {
            SpiceControl::State { state } => {
                session.state = match state {
                    SpiceSessionState::Connecting => RemoteDesktopViewState::Connecting,
                    SpiceSessionState::Connected => RemoteDesktopViewState::Connected,
                    SpiceSessionState::Migrating => RemoteDesktopViewState::Reconnecting,
                    SpiceSessionState::Disconnected => RemoteDesktopViewState::Disconnected,
                };
                if let Some(metadata) = self.session.metadata_mut(id) {
                    metadata.disconnected = state == SpiceSessionState::Disconnected;
                }
                if state == SpiceSessionState::Disconnected {
                    if let Some(spice) = session.spice.as_mut() {
                        spice.fail_transfer();
                    }
                    self.remote_desktop.spice_routes.remove(id);
                    let _ = self.remote_desktop.spice_manager.close_detached(id);
                }
                cx.notify();
            }
            SpiceControl::Capabilities { capabilities } => {
                if let Some(spice) = session.spice.as_mut() {
                    spice.capabilities = capabilities;
                }
                cx.notify();
            }
            SpiceControl::Monitors { monitors } => {
                if let Some(spice) = session.spice.as_mut() {
                    if !monitors.iter().any(|m| Some(m.id) == spice.selected) {
                        spice.selected = monitors.first().map(|m| m.id);
                    }
                    spice.monitors = monitors;
                }
                cx.notify();
            }
            SpiceControl::ClipboardOffer { text } => {
                let allowed = !self.security.screen_locked() && self.session.active_id() == Some(id) && self.session.metadata(id).is_some_and(|m| matches!(&m.launch_config, SessionLaunchConfig::Spice(config) if config.session.settings.clipboard.can_receive()));
                if allowed
                    && session
                        .clipboard
                        .accept(ClipboardOrigin::Remote, text.expose_secret())
                        .is_ok_and(|generation| generation.is_some())
                {
                    cx.write_to_clipboard(ClipboardItem::new_string(
                        text.expose_secret().to_owned(),
                    ));
                }
            }
            SpiceControl::Error { error, fatal } => {
                if fatal {
                    let prompt = error == SpiceErrorKind::Authentication
                        && self.session.metadata(id).is_some_and(|m| matches!(&m.launch_config,
                            SessionLaunchConfig::Spice(config)
                            if config.session.credential_lifetime == nyaterm_remote_desktop::spice::protocol::SpiceCredentialLifetime::Saved
                                && config.session.password.is_empty()));
                    self.fail_spice_session(id, error, cx);
                    if prompt {
                        self.prompt_remote_desktop_password(id, false, cx);
                    }
                } else {
                    if error == SpiceErrorKind::FileTransfer
                        && let Some(spice) = session.spice.as_mut()
                    {
                        spice.fail_transfer();
                    }
                    self.shell.set_status(super::spice_error(error).message);
                    cx.notify();
                }
            }
            SpiceControl::SurfaceRemoved { channel } => {
                if let Some(spice) = session.spice.as_mut() {
                    spice.cursors.remove(channel);
                }
                if let Some(spice) = session.spice.as_mut()
                    && spice
                        .displayed
                        .as_ref()
                        .is_some_and(|d| d.id.channel == channel)
                {
                    spice.displayed = None;
                    session.framebuffer = None;
                    if let Some(texture) = session.texture.take() {
                        self.remote_desktop.pending_texture_removals.push(texture);
                    }
                }
            }
            SpiceControl::SurfaceReset { .. } => {}
            SpiceControl::TransferProgress {
                transfer_id,
                completed,
                total,
                state,
            } => {
                if let Some(transfer) = session.spice.as_mut().and_then(|s| s.transfer.as_mut())
                    && transfer.id == transfer_id
                {
                    transfer.completed = completed;
                    transfer.total = total;
                    transfer.state = state;
                    cx.notify();
                }
            }
            _ => self.fail_spice_session(id, SpiceErrorKind::Protocol, cx),
        }
    }

    fn fail_spice_session(&mut self, id: &str, error: SpiceErrorKind, cx: &mut Context<Self>) {
        let reconnect = self
            .session
            .metadata(id)
            .and_then(|m| match &m.launch_config {
                SessionLaunchConfig::Spice(config) if config.session.can_reconnect() => {
                    Some(config.session.settings.reconnect_attempts)
                }
                _ => None,
            });
        let Some(session) = self.remote_desktop.sessions.get_mut(id) else {
            return;
        };
        session.keys = Default::default();
        session.pending_pointer = None;
        if let Some(spice) = session.spice.as_mut() {
            spice.fail_transfer();
        }
        session.error = Some(super::spice_error(error));
        let retryable = matches!(
            error,
            SpiceErrorKind::Connection | SpiceErrorKind::Timeout | SpiceErrorKind::HelperCrash
        );
        if retryable && reconnect.is_some_and(|max| session.reconnect_attempts < max) {
            session.reconnect_attempts += 1;
            session.reconnect_at = Some(
                std::time::Instant::now()
                    + std::time::Duration::from_secs(1 << session.reconnect_attempts.min(5)),
            );
            session.state = RemoteDesktopViewState::Reconnecting;
        } else {
            session.state = RemoteDesktopViewState::Failed;
        }
        if let Some(metadata) = self.session.metadata_mut(id) {
            metadata.disconnected = true;
        }
        self.remote_desktop.spice_routes.remove(id);
        let _ = self.remote_desktop.spice_manager.close_detached(id);
        self.ensure_remote_desktop_periodic_clock(cx);
        cx.notify();
    }
}
