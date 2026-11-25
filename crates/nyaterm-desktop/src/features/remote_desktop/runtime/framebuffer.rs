use super::events::{remote_desktop_frame_interval, set_rdp_view_error};
use super::lifecycle::clear_rdp_reconnect_after_frame;
use crate::features::NyaTermApp;
use crate::features::remote_desktop::state::RemoteDesktopSessionState;
use crate::features::remote_desktop::state::frame_source::RemoteDesktopFrameSource;
use gpui::{Bounds, Context, DevicePixels, Window, point, size};
use nyaterm_remote_desktop::presentation::{
    RdpPresentation, RdpPresentationConfig, RemotePresentationId, VncPresentation,
    VncPresentationConfig, VncPresentationId,
};
use nyaterm_remote_desktop::{
    FramebufferLimits, RdpErrorKind, RemoteCursorEvent, validate_framebuffer_dimensions,
};

impl NyaTermApp {
    pub(super) fn stop_remote_desktop_uploads(
        &mut self,
        session_id: &str,
        window: &mut Window,
        kind: RdpErrorKind,
        message: String,
    ) {
        if let Some(session) = self.remote_desktop.sessions.get_mut(session_id) {
            session.uploads_stopped = true;
            session.reconnect_at = None;
            session.framebuffer = None;
            session.cursor_shape = None;
            release_textures(session, window);
            set_rdp_view_error(session, kind, message);
        }
        // Capacity rejection keeps ownership in the manager. Maintenance retries
        // submission while uploads remain stopped.
        let _ = self.remote_desktop.manager.close_detached(session_id);
        let _ = self.remote_desktop.vnc_manager.close_detached(session_id);
        let _ = self.remote_desktop.spice_manager.close_detached(session_id);
    }

    pub(super) fn reset_rdp_framebuffer(
        &mut self,
        session_id: &str,
        epoch: u64,
        width: u32,
        height: u32,
        limits: FramebufferLimits,
        window: &mut Window,
    ) {
        if let Err(error) = validate_framebuffer_dimensions(width, height, limits) {
            self.stop_remote_desktop_uploads(
                session_id,
                window,
                RdpErrorKind::Protocol,
                error.to_string(),
            );
            return;
        }
        let Some(session) = self.remote_desktop.sessions.get_mut(session_id) else {
            return;
        };
        if session.uploads_stopped {
            return;
        }
        // A worker can publish this epoch after the UI drained controls but
        // before it took the presentation. A delayed reset must not retire an
        // already-uploaded surface whose damage has been acknowledged.
        if session.framebuffer.as_ref().is_some_and(|source| {
            matches!(source, RemoteDesktopFrameSource::Rdp { id, .. } if epoch <= id.epoch)
        }) {
            return;
        }
        release_textures(session, window);
        session.framebuffer = Some(RemoteDesktopFrameSource::Rdp {
            width,
            height,
            id: RemotePresentationId {
                epoch,
                generation: 0,
                sequence: 0,
            },
        });
        session.cursor_shape = None;
        session.cursor_position = Default::default();
        session.cursor_visible = true;
        session.texture_dirty = false;
    }

    pub(super) fn sync_remote_presentation_visibility(&mut self, window: &Window) {
        let software = crate::platform::is_software_renderer(self.shell.renderer());
        let config = VncPresentationConfig {
            frame_interval: remote_desktop_frame_interval(software),
        };
        let visibility = self
            .remote_desktop
            .sessions
            .iter()
            .filter(|(id, _)| self.remote_desktop_session_in_window(id, window))
            .map(|(id, session)| {
                (
                    id.clone(),
                    !session.uploads_stopped && self.remote_desktop_session_visible(id, window),
                )
            })
            .collect::<Vec<_>>();
        for (id, visible) in visibility {
            let session = self
                .remote_desktop
                .sessions
                .get_mut(&id)
                .expect("visibility snapshot");
            self.remote_desktop.manager.set_presentation_visible(
                &id,
                visible,
                RdpPresentationConfig {
                    frame_interval: config.frame_interval,
                },
            );
            self.remote_desktop
                .vnc_manager
                .set_presentation_visible(&id, visible, config);
            if visible && session.texture_dirty {
                match session.framebuffer.as_ref() {
                    Some(RemoteDesktopFrameSource::Rdp { .. }) => {
                        self.remote_desktop.manager.invalidate_presentation(&id)
                    }
                    Some(RemoteDesktopFrameSource::Vnc { .. }) => {
                        self.remote_desktop.vnc_manager.invalidate_presentation(&id)
                    }
                    _ => continue,
                }
                session.texture_dirty = false;
            }
        }
    }

    pub(super) fn reset_vnc_surface(
        &mut self,
        session_id: &str,
        epoch: u64,
        width: u32,
        height: u32,
        window: &mut Window,
    ) {
        let Some(session) = self.remote_desktop.sessions.get_mut(session_id) else {
            return;
        };
        if session.uploads_stopped {
            return;
        }
        if session.framebuffer.as_ref().is_some_and(|source| {
            matches!(source, RemoteDesktopFrameSource::Vnc { id, .. } if epoch <= id.epoch)
        }) {
            return;
        }
        release_textures(session, window);
        session.framebuffer = Some(RemoteDesktopFrameSource::Vnc {
            width,
            height,
            id: VncPresentationId {
                epoch,
                generation: 0,
                sequence: 0,
            },
        });
        session.cursor_shape = None;
        session.cursor_position = Default::default();
        session.cursor_visible = true;
        session.texture_dirty = false;
    }

    pub(super) fn apply_rdp_presentation(
        &mut self,
        session_id: &str,
        presentation: RdpPresentation,
        window: &mut Window,
    ) {
        if !self.remote_desktop_session_visible(session_id, window) {
            return;
        }
        let Some(session) = self.remote_desktop.sessions.get_mut(session_id) else {
            return;
        };
        if session.uploads_stopped {
            return;
        }
        if session
            .framebuffer
            .as_ref()
            .is_some_and(|source| source.presentation_is_stale(presentation.id))
        {
            return;
        }
        session.texture_window = Some(window.window_handle());
        let result = upload_prepared_frame(window, &mut session.texture, &presentation);
        if matches!(result, Ok(false)) {
            self.remote_desktop
                .manager
                .invalidate_presentation(session_id);
            return;
        }
        if let Err(error) = result {
            self.stop_remote_desktop_uploads(
                session_id,
                window,
                RdpErrorKind::ResourceExhausted,
                format!("RDP texture upload failed: {error}"),
            );
            return;
        }
        session.framebuffer = Some(RemoteDesktopFrameSource::Rdp {
            width: presentation.width,
            height: presentation.height,
            id: presentation.id,
        });
        session.texture_dirty = false;
        clear_rdp_reconnect_after_frame(session);
        self.remote_desktop
            .manager
            .acknowledge_presentation(session_id, presentation.id);
        if self.remote_desktop.metrics_enabled {
            tracing::debug!(
                regions = presentation.regions.len(),
                upload_bytes = presentation
                    .regions
                    .iter()
                    .map(|r| r.pixels.len())
                    .sum::<usize>(),
                submit_latency_us = presentation.completed_at.elapsed().as_micros() as u64,
                prepare_latency_us = presentation
                    .prepared_at
                    .saturating_duration_since(presentation.completed_at)
                    .as_micros() as u64,
                "RDP presentation submitted"
            );
        }
    }

    pub(super) fn apply_vnc_presentation(
        &mut self,
        session_id: &str,
        presentation: VncPresentation,
        window: &mut Window,
    ) {
        if !self.remote_desktop_session_visible(session_id, window) {
            return;
        }
        let Some(session) = self.remote_desktop.sessions.get_mut(session_id) else {
            return;
        };
        if session.uploads_stopped {
            return;
        }
        if session
            .framebuffer
            .as_ref()
            .is_some_and(|source| source.presentation_is_stale(presentation.id))
        {
            return;
        }
        session.texture_window = Some(window.window_handle());
        let result = upload_prepared_frame(window, &mut session.texture, &presentation);
        if matches!(result, Ok(false)) {
            self.remote_desktop
                .vnc_manager
                .invalidate_presentation(session_id);
            return;
        }
        if let Err(error) = result {
            self.stop_remote_desktop_uploads(
                session_id,
                window,
                RdpErrorKind::ResourceExhausted,
                format!("VNC texture upload failed: {error}"),
            );
            return;
        }
        session.framebuffer = Some(RemoteDesktopFrameSource::Vnc {
            width: presentation.width,
            height: presentation.height,
            id: presentation.id,
        });
        session.texture_dirty = false;
        clear_rdp_reconnect_after_frame(session);
        self.remote_desktop
            .vnc_manager
            .acknowledge_presentation(session_id, presentation.id);
        if self.remote_desktop.metrics_enabled {
            tracing::debug!(
                regions = presentation.regions.len(),
                upload_bytes = presentation
                    .regions
                    .iter()
                    .map(|r| r.pixels.len())
                    .sum::<usize>(),
                submit_latency_us = presentation.completed_at.elapsed().as_micros() as u64,
                prepare_latency_us = presentation
                    .prepared_at
                    .saturating_duration_since(presentation.completed_at)
                    .as_micros() as u64,
                "VNC presentation submitted"
            );
        }
    }

    pub(super) fn apply_remote_cursor_batch(
        &mut self,
        session_id: &str,
        cursors: Vec<RemoteCursorEvent>,
        window: &mut Window,
    ) {
        let visible = self.remote_desktop_session_visible(session_id, window);
        let Some(session) = self.remote_desktop.sessions.get_mut(session_id) else {
            return;
        };
        if session.uploads_stopped {
            return;
        }
        for cursor in cursors {
            match cursor {
                RemoteCursorEvent::Shape(shape) => {
                    if session
                        .cursor_shape
                        .as_ref()
                        .is_some_and(|current| current.shape_id == shape.shape_id)
                        && session.cursor_texture.is_some()
                    {
                        continue;
                    }
                    if let Some(texture) = session.cursor_texture.take() {
                        window.remove_dynamic_texture(texture);
                    }
                    session.cursor_shape = Some(shape);
                }
                RemoteCursorEvent::Position(position) => session.cursor_position = position,
                RemoteCursorEvent::Visibility(visibility) => {
                    session.cursor_visible = visibility.visible
                }
            }
        }
        if visible {
            self.prepare_remote_desktop_textures(session_id, window);
        }
    }

    /// Re-upload whatever the hidden window missed once it is presented again.
    ///
    /// Background compositors keep pixels current while hidden and publish a
    /// full upload on resume. Registering before first paint also covers a
    /// session restored into a minimized window.
    pub(in crate::features) fn observe_remote_desktop_window_visibility(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.observe_window_visibility(window, |this, visibility, window, cx| {
            this.reconcile_spice_pointer(window, cx);
            this.sync_remote_presentation_visibility(window);
            this.sync_spice_presentation_visibility(window);
            if visibility.is_visible() {
                this.resume_remote_desktop_uploads(window, cx);
            }
        })
        .detach();
    }

    fn resume_remote_desktop_uploads(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session_id) = self
            .remote_desktop_input_session(window)
            .map(str::to_string)
        else {
            return;
        };
        if self
            .remote_desktop
            .sessions
            .get(&session_id)
            .is_some_and(|session| session.texture_dirty || session.texture.is_none())
        {
            self.prepare_remote_desktop_textures(&session_id, window);
        }
        self.sync_remote_presentation_visibility(window);
        cx.notify();
    }

    pub(in crate::features::remote_desktop) fn prepare_remote_desktop_textures(
        &mut self,
        session_id: &str,
        window: &mut Window,
    ) {
        if !self.remote_desktop_session_in_window(session_id, window) {
            return;
        }
        // Hidden windows cannot drain texture uploads. Pixel publication resumes
        // through the compositor when the owning window becomes visible.
        if !window.is_visible() {
            return;
        }
        let Some(session) = self.remote_desktop.sessions.get_mut(session_id) else {
            return;
        };
        if session.uploads_stopped {
            return;
        }
        if let Err(error) = prepare_textures(session, window) {
            self.stop_remote_desktop_uploads(
                session_id,
                window,
                RdpErrorKind::ResourceExhausted,
                format!("remote desktop texture upload failed: {error}"),
            );
        }
    }
}

fn release_textures(session: &mut RemoteDesktopSessionState, window: &mut Window) {
    if let Some(texture) = session.texture.take() {
        window.remove_dynamic_texture(texture);
    }
    if let Some(texture) = session.cursor_texture.take() {
        window.remove_dynamic_texture(texture);
    }
}

fn prepare_textures(
    session: &mut RemoteDesktopSessionState,
    window: &mut Window,
) -> anyhow::Result<()> {
    session.texture_window = Some(window.window_handle());
    if session.cursor_texture.is_none()
        && let Some(shape) = session.cursor_shape.as_ref()
        && shape.width > 0
        && shape.height > 0
    {
        session.cursor_texture = Some(window.create_dynamic_texture(
            size(
                DevicePixels(shape.width as i32),
                DevicePixels(shape.height as i32),
            ),
            &shape.pixels,
            shape.width * 4,
        )?);
    }
    Ok(())
}

/// Uploads reference-counted, background-packed regions without a UI-side pixel copy.
pub(super) fn upload_prepared_frame(
    window: &mut Window,
    texture: &mut Option<gpui::DynamicTexture>,
    frame: &nyaterm_remote_desktop::presentation::RemotePresentation,
) -> anyhow::Result<bool> {
    let dimensions = size(
        DevicePixels(frame.width as i32),
        DevicePixels(frame.height as i32),
    );
    if texture.is_some_and(|t| t.size() != dimensions)
        && let Some(old) = texture.take()
    {
        window.remove_dynamic_texture(old);
    }
    if let Some(texture) = *texture {
        let updates = frame
            .regions
            .iter()
            .map(|region| gpui::DynamicTextureUpdate {
                bounds: Bounds::new(
                    point(
                        DevicePixels(region.rect.x as i32),
                        DevicePixels(region.rect.y as i32),
                    ),
                    size(
                        DevicePixels(region.rect.width as i32),
                        DevicePixels(region.rect.height as i32),
                    ),
                ),
                bytes: std::sync::Arc::new(region.pixels.clone()),
                bytes_per_row: region.rect.width * 4,
            })
            .collect();
        window.update_dynamic_texture_owned_batch(texture, updates)?;
    } else if frame.full && frame.regions.len() == 1 {
        *texture = Some(window.create_dynamic_texture_owned(
            dimensions,
            std::sync::Arc::new(frame.regions[0].pixels.clone()),
        )?);
    } else {
        return Ok(false);
    }
    Ok(true)
}
