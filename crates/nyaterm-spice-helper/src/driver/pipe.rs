use std::collections::VecDeque;
use std::io::{self, BufWriter, Read, Write};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use nyaterm_remote_desktop::spice::ipc::encode_control;
use nyaterm_remote_desktop::spice::protocol::{SpiceControl, SpiceErrorKind};
use nyaterm_remote_desktop::{CONTROL_PAYLOAD_LIMIT, Packet, PacketType, write_packet_into};
use zeroize::Zeroizing;

const MAX_PACKETS: usize = 128;
const MAX_BYTES: usize = 16 * 1024 * 1024;
const MAX_FRAMES: usize = 2;

#[derive(Default)]
struct Queue {
    packets: VecDeque<Packet>,
    bytes: usize,
    frames: usize,
    closed: bool,
    failed: bool,
}

#[derive(Default)]
pub(super) struct Outbox {
    queue: Mutex<Queue>,
    changed: Condvar,
}

impl Outbox {
    pub fn start(writer: impl Write + Send + 'static) -> io::Result<(Arc<Self>, JoinHandle<()>)> {
        let outbox = Arc::new(Self::default());
        let shared = outbox.clone();
        let worker = thread::Builder::new()
            .name("spice-stdout".into())
            .spawn(move || {
                if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| shared.write(writer)))
                    .is_err()
                {
                    shared
                        .queue
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .failed = true;
                }
            })?;
        Ok((outbox, worker))
    }

    pub fn control(&self, message: SpiceControl) -> Result<(), SpiceErrorKind> {
        let packet = encode_control(&message).map_err(|_| SpiceErrorKind::Protocol)?;
        self.send(packet)
    }

    pub fn send(&self, packet: Packet) -> Result<(), SpiceErrorKind> {
        let bytes = packet.encoded_len().map_err(|_| SpiceErrorKind::Protocol)?;
        let frame = packet.packet_type == PacketType::Frame;
        let mut queue = self.queue.lock().unwrap_or_else(|p| p.into_inner());
        if queue.closed || queue.failed {
            return Err(SpiceErrorKind::HelperCrash);
        }
        if queue.packets.len() >= MAX_PACKETS
            || queue.bytes + bytes > MAX_BYTES
            || (frame && queue.frames >= MAX_FRAMES)
        {
            return Err(SpiceErrorKind::ResourceLimit);
        }
        queue.bytes += bytes;
        queue.frames += usize::from(frame);
        queue.packets.push_back(packet);
        drop(queue);
        self.changed.notify_one();
        Ok(())
    }

    pub fn can_frame(&self) -> bool {
        let queue = self.queue.lock().unwrap_or_else(|p| p.into_inner());
        !queue.closed
            && !queue.failed
            && queue.frames < MAX_FRAMES
            && queue.bytes < MAX_BYTES / 2
            && queue.packets.len() < MAX_PACKETS / 2
    }

    pub fn failed(&self) -> bool {
        self.queue.lock().unwrap_or_else(|p| p.into_inner()).failed
    }

    pub fn finish(&self, result: Result<(), SpiceErrorKind>, worker: JoinHandle<()>) {
        let mut queue = self.queue.lock().unwrap_or_else(|p| p.into_inner());
        // One final small error is reserved outside ordinary queue limits.
        // Keep frame/reset ordering even when closing under backpressure.
        if let Err(error) = result
            && let Ok(packet) = encode_control(&SpiceControl::Error { error, fatal: true })
        {
            queue.bytes += packet.encoded_len().unwrap_or(0);
            queue.packets.push_back(packet);
        }
        queue.closed = true;
        drop(queue);
        self.changed.notify_one();
        let deadline = Instant::now() + Duration::from_millis(500);
        while !worker.is_finished() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(2));
        }
        if worker.is_finished() {
            let _ = worker.join();
        }
    }

    fn write(&self, writer: impl Write) {
        let mut writer = BufWriter::with_capacity(256 * 1024, writer);
        loop {
            let packet = {
                let mut queue = self.queue.lock().unwrap_or_else(|p| p.into_inner());
                while queue.packets.is_empty() && !queue.closed {
                    queue = self.changed.wait(queue).unwrap_or_else(|p| p.into_inner());
                }
                let Some(packet) = queue.packets.pop_front() else {
                    return;
                };
                packet
            };
            let result = write_packet_into(&mut writer, &packet).and_then(|()| writer.flush());
            let mut queue = self.queue.lock().unwrap_or_else(|p| p.into_inner());
            // In-flight bytes count until the OS write completes. A blocked
            // pipe cannot hide an extra frame outside the memory budget.
            queue.bytes = queue
                .bytes
                .saturating_sub(packet.encoded_len().unwrap_or(0));
            queue.frames -= usize::from(packet.packet_type == PacketType::Frame);
            if result.is_err() {
                queue.failed = true;
                return;
            }
        }
    }
}

pub(super) fn start_reader(
    reader: impl Read + Send + 'static,
) -> io::Result<mpsc::Receiver<io::Result<Option<SpiceControl>>>> {
    let (sender, receiver) = mpsc::sync_channel(32);
    thread::Builder::new()
        .name("spice-stdin".into())
        .spawn(move || {
            let mut reader = reader;
            loop {
                let next = read_command(&mut reader);
                let done = !matches!(next, Ok(Some(_)));
                if sender.send(next).is_err() || done {
                    break;
                }
            }
        })?;
    Ok(receiver)
}

fn read_command(reader: &mut impl Read) -> io::Result<Option<SpiceControl>> {
    let mut header = [0u8; 5];
    loop {
        match reader.read(&mut header[..1]) {
            Ok(0) => return Ok(None),
            Ok(_) => break,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    reader.read_exact(&mut header[1..])?;
    let size = u32::from_le_bytes(header[1..].try_into().unwrap()) as usize;
    // Reject wrong-direction pixel packets before reading/allocating a body.
    if header[0] != PacketType::Control as u8 || size > CONTROL_PAYLOAD_LIMIT {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid SPICE command header",
        ));
    }
    let mut payload = Zeroizing::new(vec![0; size]);
    reader.read_exact(&mut payload)?;
    let command: SpiceControl = serde_json::from_slice(&payload).map_err(|_| {
        io::Error::new(io::ErrorKind::InvalidData, "invalid SPICE command document")
    })?;
    command
        .validate()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid SPICE command"))?;
    Ok(Some(command))
}

#[cfg(test)]
mod tests {
    use crate::driver::pipe::{Outbox, read_command};
    use nyaterm_remote_desktop::spice::protocol::{SpiceControl, SpiceErrorKind};

    #[test]
    fn pixel_packet_is_rejected_without_reading_its_body() {
        let mut header = &[2, 0, 0, 0, 10][..];
        assert!(read_command(&mut header).is_err());
        assert!(header.is_empty());
        assert!(read_command(&mut &[1, 0][..]).is_err());
        assert!(read_command(&mut &[][..]).unwrap().is_none());
    }

    #[test]
    fn slow_consumer_has_a_bounded_control_queue() {
        let outbox = Outbox::default();
        for sequence in 0..128 {
            outbox.control(SpiceControl::Pong { sequence }).unwrap();
        }
        assert_eq!(
            outbox.control(SpiceControl::Pong { sequence: 129 }),
            Err(SpiceErrorKind::ResourceLimit)
        );
        assert!(!outbox.can_frame());
    }
}
