//! Tracks inputs sent to the server so interruptions release all held fingers.
use crate::{Geometry, protocol};
use nyaterm_android::ipc::Control;
use std::collections::HashSet;
use std::io::{self, Write};
use std::net::{Shutdown, TcpStream};

pub(super) struct Inputs {
    keys: HashSet<u32>,
    // Single-point controls in press order, including their last valid geometry.
    touches: Vec<Control>,
    socket: TcpStream,
    geometry: Geometry,
}

impl Inputs {
    pub(super) fn new(socket: TcpStream, geometry: Geometry) -> Self {
        Self {
            keys: HashSet::new(),
            touches: Vec::new(),
            socket,
            geometry,
        }
    }

    pub(super) fn send(&mut self, control: &Control) -> io::Result<()> {
        control
            .validate_input()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid input"))?;
        if let Control::Touch {
            width,
            height,
            generation,
            ..
        }
        | Control::Scroll {
            width,
            height,
            generation,
            ..
        } = control
        {
            let current = *self.geometry.lock().unwrap_or_else(|p| p.into_inner());
            if current != (*width, *height, *generation) {
                self.release()?;
                return Ok(());
            }
        }
        if let Control::Touch {
            action,
            points,
            width,
            height,
            generation,
        } = control
        {
            if *action == 0 {
                // A fresh press cannot inherit a half-finished gesture.
                self.release_touches()?;
            } else if points.len() != self.touches.len() || !points.iter().zip(&self.touches).all(|(point, active)| {
                matches!(active, Control::Touch { points: old, width: w, height: h, generation: g, .. }
                    if old[0].pointer == point.pointer && (w, h, g) == (width, height, generation))
            }) {
                self.release_touches()?;
                return Ok(());
            }
            for point in points {
                let single = Control::Touch {
                    action: *action,
                    points: vec![*point],
                    width: *width,
                    height: *height,
                    generation: *generation,
                };
                self.socket.write_all(&protocol::encode(&single)?)?;
                let index = self.touches.iter().position(|active| {
                    matches!(active, Control::Touch { points, .. } if points[0].pointer == point.pointer)
                });
                match (*action, index) {
                    (0, _) => self.touches.push(single),
                    (1, Some(index)) => {
                        self.touches.remove(index);
                    }
                    (2, Some(index)) => self.touches[index] = single,
                    _ => {}
                }
            }
            return Ok(());
        }
        if matches!(control, Control::Rotate) {
            self.release()?;
        }
        self.socket.write_all(&protocol::encode(control)?)?;
        match control {
            Control::Key {
                action: 0, keycode, ..
            } => {
                self.keys.insert(*keycode);
            }
            Control::Key {
                action: 1, keycode, ..
            } => {
                self.keys.remove(keycode);
            }
            _ => {}
        }
        Ok(())
    }

    fn release_touches(&mut self) -> io::Result<()> {
        let mut result = Ok(());
        let (current_width, current_height, current_generation) =
            *self.geometry.lock().unwrap_or_else(|p| p.into_inner());
        for mut touch in self.touches.drain(..) {
            if let Control::Touch {
                action,
                points,
                width,
                height,
                generation,
            } = &mut touch
            {
                // scrcpy removes pointer records on UP, not CANCEL.
                *action = 1;
                // The server rejects stale screen sizes before releasing pointers.
                // Use the latest capture geometry even when ending an old gesture.
                if current_width > 0 && current_height > 0 {
                    *width = current_width;
                    *height = current_height;
                    *generation = current_generation;
                    for point in points {
                        point.x = point.x.min(current_width - 1);
                        point.y = point.y.min(current_height - 1);
                    }
                }
            }
            if let Err(error) =
                protocol::encode(&touch).and_then(|bytes| self.socket.write_all(&bytes))
            {
                result = Err(error);
            }
        }
        result
    }

    pub(super) fn release(&mut self) -> io::Result<()> {
        let mut result = self.release_touches();
        for keycode in self.keys.drain() {
            let key = Control::Key {
                action: 1,
                keycode,
                repeat: 0,
                metastate: 0,
            };
            if let Err(error) =
                protocol::encode(&key).and_then(|bytes| self.socket.write_all(&bytes))
            {
                result = Err(error);
            }
        }
        result
    }
}

impl Drop for Inputs {
    fn drop(&mut self) {
        let _ = self.release();
        let _ = self.socket.shutdown(Shutdown::Both);
    }
}

#[cfg(test)]
mod tests {
    use crate::inputs::Inputs;
    use crate::protocol;
    use nyaterm_android::ipc::{Control, TouchPoint, TouchPointer};
    use std::io::Read;
    use std::net::{TcpListener, TcpStream};
    use std::sync::{Arc, Mutex};

    fn touch(action: u8, x: u32) -> Control {
        Control::Touch {
            action,
            points: vec![
                TouchPoint {
                    pointer: TouchPointer::Finger,
                    x,
                    y: 2,
                },
                TouchPoint {
                    pointer: TouchPointer::VirtualFinger,
                    x: 99 - x,
                    y: 197,
                },
            ],
            width: 100,
            height: 200,
            generation: 1,
        }
    }

    fn fixture() -> (Inputs, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let socket = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (reader, _) = listener.accept().unwrap();
        reader
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .unwrap();
        (
            Inputs::new(socket, Arc::new(Mutex::new((100, 200, 1)))),
            reader,
        )
    }

    #[test]
    fn releases_both_fingers_at_last_positions_and_can_start_again() {
        let (mut inputs, mut reader) = fixture();
        inputs.send(&touch(2, 10)).unwrap(); // Orphan moves must not press a finger.
        inputs.send(&touch(1, 10)).unwrap();
        inputs.send(&touch(0, 10)).unwrap();
        inputs.send(&touch(2, 20)).unwrap();
        inputs.release().unwrap();
        inputs.release().unwrap();
        inputs.send(&touch(2, 30)).unwrap();
        inputs.send(&touch(0, 40)).unwrap();
        inputs.send(&touch(1, 40)).unwrap();
        drop(inputs);
        let mut actual = Vec::new();
        reader.read_to_end(&mut actual).unwrap();
        let expected: Vec<u8> = [
            touch(0, 10),
            touch(2, 20),
            touch(1, 20),
            touch(0, 40),
            touch(1, 40),
        ]
        .iter()
        .flat_map(|event| protocol::encode(event).unwrap())
        .collect();
        assert_eq!(actual, expected);
    }

    #[test]
    fn geometry_change_releases_old_gesture_and_discards_stale_move() {
        let (mut inputs, mut reader) = fixture();
        inputs.send(&touch(0, 10)).unwrap();
        *inputs.geometry.lock().unwrap() = (200, 100, 2);
        inputs.send(&touch(2, 20)).unwrap();
        assert!(inputs.touches.is_empty());
        drop(inputs);
        let mut actual = Vec::new();
        reader.read_to_end(&mut actual).unwrap();
        let mut release = touch(1, 10);
        if let Control::Touch {
            points,
            width,
            height,
            generation,
            ..
        } = &mut release
        {
            *width = 200;
            *height = 100;
            *generation = 2;
            points[1].y = 99;
        }
        let expected: Vec<u8> = [touch(0, 10), release]
            .iter()
            .flat_map(|event| protocol::encode(event).unwrap())
            .collect();
        assert_eq!(actual, expected);
    }
}
