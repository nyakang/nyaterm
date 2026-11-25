use nyaterm_remote_desktop::presentation::RemotePresentationId;

/// Pixels belong to background compositors; desktop keeps only GPU metadata.
pub(in crate::features::remote_desktop) enum RemoteDesktopFrameSource {
    Rdp {
        width: u32,
        height: u32,
        id: RemotePresentationId,
    },
    Spice {
        width: u32,
        height: u32,
        id: nyaterm_remote_desktop::spice::presentation::SpicePresentationId,
    },
    Vnc {
        width: u32,
        height: u32,
        id: RemotePresentationId,
    },
}

impl RemoteDesktopFrameSource {
    pub(in crate::features::remote_desktop) fn presentation_is_stale(
        &self,
        incoming: RemotePresentationId,
    ) -> bool {
        let (Self::Vnc { id, .. } | Self::Rdp { id, .. }) = self else {
            return false;
        };
        (incoming.epoch, incoming.generation, incoming.sequence)
            < (id.epoch, id.generation, id.sequence)
    }

    pub(in crate::features::remote_desktop) fn width(&self) -> u32 {
        match self {
            Self::Rdp { width, .. } | Self::Vnc { width, .. } | Self::Spice { width, .. } => *width,
        }
    }

    pub(in crate::features::remote_desktop) fn height(&self) -> u32 {
        match self {
            Self::Rdp { height, .. } | Self::Vnc { height, .. } | Self::Spice { height, .. } => {
                *height
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::RemoteDesktopFrameSource;
    use nyaterm_remote_desktop::presentation::RemotePresentationId;

    #[test]
    fn remote_surfaces_reject_older_epochs_generations_and_pixel_revisions() {
        let id = RemotePresentationId {
            epoch: 2,
            generation: 3,
            sequence: 100,
        };
        for source in [
            RemoteDesktopFrameSource::Rdp {
                width: 1920,
                height: 1080,
                id,
            },
            RemoteDesktopFrameSource::Vnc {
                width: 1920,
                height: 1080,
                id,
            },
        ] {
            for stale in [
                RemotePresentationId {
                    epoch: 1,
                    generation: 4,
                    sequence: 200,
                },
                RemotePresentationId {
                    epoch: 2,
                    generation: 2,
                    sequence: 200,
                },
                RemotePresentationId {
                    epoch: 2,
                    generation: 3,
                    sequence: 99,
                },
            ] {
                assert!(source.presentation_is_stale(stale));
            }
            for fresh in [
                id,
                RemotePresentationId {
                    epoch: 2,
                    generation: 3,
                    sequence: 101,
                },
                RemotePresentationId {
                    epoch: 2,
                    generation: 4,
                    sequence: 100,
                },
                RemotePresentationId {
                    epoch: 3,
                    generation: 1,
                    sequence: 0,
                },
            ] {
                assert!(!source.presentation_is_stale(fresh));
            }
        }
    }
}
