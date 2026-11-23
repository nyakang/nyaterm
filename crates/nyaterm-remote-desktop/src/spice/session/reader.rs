use std::collections::BTreeMap;
use std::io;
use std::process::ChildStdout;
use std::sync::Arc;
use std::thread::JoinHandle;

use crate::helper_process::IpcWriter;
use crate::spice::ipc::{decode_control, decode_cursor, decode_frame};
use crate::spice::protocol::{
    SPICE_PROTOCOL_VERSION, SpiceControl, SpiceErrorKind, SpiceSessionConfig, SpiceSessionState,
};
use crate::spice::session::queue::EventQueue;
use crate::spice::session::{SpiceEvent, send_critical};
use crate::{PacketType, RemoteFrameEvent};

pub(super) fn spawn(
    mut stdout: ChildStdout,
    queue: Arc<EventQueue>,
    writer: Arc<IpcWriter>,
    config: SpiceSessionConfig,
    presentation: Option<crate::spice::presentation::PresentationClient>,
) -> io::Result<JoinHandle<()>> {
    nyaterm_core::runtime::worker::spawn("nyaterm-spice-reader".into(), move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            read(&mut stdout, &queue, &writer, config, presentation)
        }))
        .unwrap_or(Err(SpiceErrorKind::HelperCrash));
        if let Err(error) = result {
            queue.fail(error);
        }
    })
}

fn read(
    stdout: &mut impl io::Read,
    queue: &EventQueue,
    writer: &IpcWriter,
    config: SpiceSessionConfig,
    presentation: Option<crate::spice::presentation::PresentationClient>,
) -> Result<(), SpiceErrorKind> {
    let mut pending_config = Some(config);
    let mut surfaces = Surfaces::default();
    loop {
        let packet = crate::ipc::read_packet_budgeted(stdout)
            .map_err(|_| SpiceErrorKind::Protocol)?
            .ok_or(SpiceErrorKind::HelperCrash)?;
        let bytes = packet.encoded_len().map_err(|_| SpiceErrorKind::Protocol)?;
        if pending_config.is_some() {
            if decode_control(&packet).ok()
                != Some(SpiceControl::HelloAck {
                    version: SPICE_PROTOCOL_VERSION,
                })
            {
                return Err(SpiceErrorKind::Protocol);
            }
            queue.heartbeat.hello();
            send_critical(
                writer,
                SpiceControl::Connect {
                    config: Box::new(pending_config.take().unwrap()),
                },
            )?;
            continue;
        }
        let event = match packet.packet_type {
            PacketType::Control => {
                let control = decode_control(&packet).map_err(|_| SpiceErrorKind::Protocol)?;
                match &control {
                    SpiceControl::Pong { sequence } => {
                        queue.heartbeat.pong(*sequence);
                        continue;
                    }
                    SpiceControl::State {
                        state: SpiceSessionState::Disconnected,
                    } => {
                        queue.disconnected();
                        return Ok(());
                    }
                    SpiceControl::Error { error, fatal: true } => return Err(*error),
                    SpiceControl::SurfaceReset {
                        channel,
                        epoch,
                        width,
                        height,
                    } => {
                        surfaces.reset(*channel, *epoch, *width, *height)?;
                    }
                    SpiceControl::SurfaceRemoved { channel } => {
                        surfaces.0.remove(channel);
                    }
                    SpiceControl::State { .. }
                    | SpiceControl::Capabilities { .. }
                    | SpiceControl::Monitors { .. }
                    | SpiceControl::ClipboardOffer { .. }
                    | SpiceControl::TransferProgress { .. }
                    | SpiceControl::Error { .. } => {}
                    _ => return Err(SpiceErrorKind::Protocol),
                }
                if let Some(presentation) = &presentation
                    && !presentation.control(&control)?
                {
                    return Ok(());
                }
                SpiceEvent::Control(control)
            }
            PacketType::Frame => {
                let (channel, event) =
                    decode_frame(packet).map_err(|_| SpiceErrorKind::Protocol)?;
                if !surfaces.accept(channel, &event)? {
                    continue;
                }
                if let Some(presentation) = &presentation {
                    if !presentation.frame(channel, event)? {
                        return Ok(());
                    }
                    continue;
                }
                SpiceEvent::Frame { channel, event }
            }
            PacketType::Cursor => {
                let (channel, event) =
                    decode_cursor(packet).map_err(|_| SpiceErrorKind::Protocol)?;
                SpiceEvent::Cursor { channel, event }
            }
        };
        if !queue.push(event, bytes) {
            return Ok(());
        }
    }
}

#[derive(Default)]
struct Surfaces(BTreeMap<u32, (u64, u32, u32)>);

impl Surfaces {
    fn reset(
        &mut self,
        channel: u32,
        epoch: u64,
        width: u32,
        height: u32,
    ) -> Result<(), SpiceErrorKind> {
        let bytes = self
            .0
            .iter()
            .filter(|(id, _)| **id != channel)
            .map(|(_, (_, w, h))| u64::from(*w) * u64::from(*h) * 4)
            .sum::<u64>()
            + u64::from(width) * u64::from(height) * 4;
        if bytes > 512 * 1024 * 1024 || (self.0.len() >= 64 && !self.0.contains_key(&channel)) {
            return Err(SpiceErrorKind::ResourceLimit);
        }
        if self
            .0
            .get(&channel)
            .is_some_and(|(old, _, _)| epoch <= *old)
        {
            return Err(SpiceErrorKind::Protocol);
        }
        self.0.insert(channel, (epoch, width, height));
        Ok(())
    }

    fn accept(&self, channel: u32, frame: &RemoteFrameEvent) -> Result<bool, SpiceErrorKind> {
        let Some(&(current, surface_width, surface_height)) = self.0.get(&channel) else {
            return Err(SpiceErrorKind::Protocol);
        };
        let RemoteFrameEvent::Bitmap {
            epoch,
            x,
            y,
            width,
            height,
            pixels,
            ..
        } = frame
        else {
            return Err(SpiceErrorKind::Protocol);
        };
        if *epoch < current {
            return Ok(false);
        }
        if *epoch != current
            || u64::from(*x) + u64::from(*width) > u64::from(surface_width)
            || u64::from(*y) + u64::from(*height) > u64::from(surface_height)
            || pixels.len() > 4 * 1024 * 1024
        {
            return Err(SpiceErrorKind::Protocol);
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use crate::spice::session::reader::Surfaces;
    use crate::{PixelFormat, RemoteFrameEvent};

    #[test]
    fn stale_frames_are_ignored_and_future_or_out_of_surface_frames_rejected() {
        let mut surfaces = Surfaces::default();
        surfaces.reset(0, 2, 16, 16).unwrap();
        let frame = |epoch, x| RemoteFrameEvent::Bitmap {
            epoch,
            full: false,
            x,
            y: 0,
            width: 2,
            height: 1,
            stride: 8,
            format: PixelFormat::Bgra8,
            pixels: vec![0; 8].into(),
        };
        assert!(!surfaces.accept(0, &frame(1, 0)).unwrap());
        assert!(surfaces.accept(0, &frame(2, 14)).unwrap());
        assert!(surfaces.accept(0, &frame(2, 15)).is_err());
        assert!(surfaces.accept(0, &frame(3, 0)).is_err());
        assert!(surfaces.accept(1, &frame(2, 0)).is_err());
        assert!(surfaces.reset(0, 2, 16, 16).is_err());
        assert!(surfaces.reset(1, 3, 16_384, 16_384).is_err());
    }
}
