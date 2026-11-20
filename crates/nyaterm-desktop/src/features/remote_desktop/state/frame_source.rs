use nyaterm_remote_desktop::Framebuffer;
use nyaterm_remote_desktop::presentation::VncPresentationId;

/// VNC pixels belong to its background compositor; desktop keeps only GPU metadata.
pub(in crate::features::remote_desktop) enum RemoteDesktopFrameSource {
    Rdp(Framebuffer),
    Vnc {
        width: u32,
        height: u32,
        id: VncPresentationId,
    },
}

impl From<Framebuffer> for RemoteDesktopFrameSource {
    fn from(framebuffer: Framebuffer) -> Self {
        Self::Rdp(framebuffer)
    }
}

impl RemoteDesktopFrameSource {
    pub(in crate::features::remote_desktop) fn vnc_presentation_is_stale(
        &self,
        incoming: VncPresentationId,
    ) -> bool {
        let Self::Vnc { id, .. } = self else {
            return false;
        };
        (incoming.epoch, incoming.generation, incoming.sequence)
            < (id.epoch, id.generation, id.sequence)
    }

    pub(in crate::features::remote_desktop) fn width(&self) -> u32 {
        match self {
            Self::Rdp(framebuffer) => framebuffer.width(),
            Self::Vnc { width, .. } => *width,
        }
    }

    pub(in crate::features::remote_desktop) fn height(&self) -> u32 {
        match self {
            Self::Rdp(framebuffer) => framebuffer.height(),
            Self::Vnc { height, .. } => *height,
        }
    }

    pub(in crate::features::remote_desktop) fn as_rdp(&self) -> Option<&Framebuffer> {
        match self {
            Self::Rdp(framebuffer) => Some(framebuffer),
            Self::Vnc { .. } => None,
        }
    }

    pub(in crate::features::remote_desktop) fn as_rdp_mut(&mut self) -> Option<&mut Framebuffer> {
        match self {
            Self::Rdp(framebuffer) => Some(framebuffer),
            Self::Vnc { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::RemoteDesktopFrameSource;
    use nyaterm_remote_desktop::presentation::VncPresentationId;

    #[test]
    fn vnc_surface_rejects_older_epochs_generations_and_pixel_revisions() {
        let id = VncPresentationId {
            epoch: 2,
            generation: 3,
            sequence: 100,
        };
        let source = RemoteDesktopFrameSource::Vnc {
            width: 1920,
            height: 1080,
            id,
        };
        for stale in [
            VncPresentationId {
                epoch: 1,
                generation: 4,
                sequence: 200,
            },
            VncPresentationId {
                epoch: 2,
                generation: 2,
                sequence: 200,
            },
            VncPresentationId {
                epoch: 2,
                generation: 3,
                sequence: 99,
            },
        ] {
            assert!(source.vnc_presentation_is_stale(stale));
        }
        for fresh in [
            id,
            VncPresentationId {
                epoch: 2,
                generation: 3,
                sequence: 101,
            },
            VncPresentationId {
                epoch: 2,
                generation: 4,
                sequence: 100,
            },
            VncPresentationId {
                epoch: 3,
                generation: 1,
                sequence: 0,
            },
        ] {
            assert!(!source.vnc_presentation_is_stale(fresh));
        }
    }
}
