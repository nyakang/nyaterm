//! Cancellable host handshakes. Partial responses stay on the same socket.
use crate::adb::client::{DeviceBinding, DeviceStatus, parse_devices, read_host_payload, request};
use crate::error::{AdbError, AdbResult};
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

struct CheckedSocket<'a, F> {
    socket: &'a mut TcpStream,
    check: &'a mut F,
    failure: Option<AdbError>,
    stalled: Duration,
}
impl<F: FnMut() -> AdbResult<()>> CheckedSocket<'_, F> {
    fn ready(&mut self) -> io::Result<()> {
        if let Err(error) = (self.check)() {
            self.failure = Some(error);
            return Err(io::Error::other("ADB operation stopped"));
        }
        Ok(())
    }
    fn stopped(&mut self, waited: Duration) -> io::Result<()> {
        self.stalled += waited;
        if self.stalled >= Duration::from_secs(15) {
            self.failure = Some(AdbError::Timeout);
            return Err(io::Error::from(io::ErrorKind::TimedOut));
        }
        Ok(())
    }
}
impl<F: FnMut() -> AdbResult<()>> Read for CheckedSocket<'_, F> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        loop {
            self.ready()?;
            let started = Instant::now();
            match self.socket.read(bytes) {
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                    ) =>
                {
                    self.stopped(started.elapsed())?
                }
                result => {
                    if result.is_ok() {
                        self.stalled = Duration::ZERO;
                    }
                    return result;
                }
            }
        }
    }
}
impl<F: FnMut() -> AdbResult<()>> Write for CheckedSocket<'_, F> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        loop {
            self.ready()?;
            let started = Instant::now();
            match self.socket.write(bytes) {
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                    ) =>
                {
                    self.stopped(started.elapsed())?
                }
                result => {
                    if result.is_ok() {
                        self.stalled = Duration::ZERO;
                    }
                    return result;
                }
            }
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        self.socket.flush()
    }
}
fn connect(
    binding: &DeviceBinding,
    check: &mut impl FnMut() -> AdbResult<()>,
) -> AdbResult<TcpStream> {
    check()?;
    let stream = TcpStream::connect_timeout(&binding.server, Duration::from_secs(1))?;
    check()?;
    stream.set_read_timeout(Some(Duration::from_millis(100)))?;
    stream.set_write_timeout(Some(Duration::from_millis(100)))?;
    stream.set_nodelay(true)?;
    Ok(stream)
}
fn checked<T, F: FnMut() -> AdbResult<()>>(
    stream: &mut TcpStream,
    check: &mut F,
    run: impl FnOnce(&mut CheckedSocket<'_, F>) -> AdbResult<T>,
) -> AdbResult<T> {
    let mut socket = CheckedSocket {
        socket: stream,
        check,
        failure: None,
        stalled: Duration::ZERO,
    };
    let result = run(&mut socket);
    if let Some(error) = socket.failure {
        Err(error)
    } else {
        result
    }
}
pub(super) fn features(
    binding: &DeviceBinding,
    mut check: impl FnMut() -> AdbResult<()>,
) -> AdbResult<Vec<String>> {
    let mut stream = connect(binding, &mut check)?;
    let payload = checked(&mut stream, &mut check, |socket| {
        request(
            socket,
            format!("host-serial:{}:features", binding.serial).as_bytes(),
        )?;
        read_host_payload(socket)
    })?;
    Ok(std::str::from_utf8(&payload)
        .map_err(|_| AdbError::Protocol("features encoding"))?
        .split(',')
        .map(str::to_owned)
        .collect())
}
pub(super) fn validate(
    binding: &DeviceBinding,
    check: &mut impl FnMut() -> AdbResult<()>,
) -> AdbResult<()> {
    if binding.transport_id.is_none() {
        return check();
    }
    let mut stream = connect(binding, check)?;
    let devices = checked(&mut stream, check, |socket| {
        request(socket, b"host:devices-l")?;
        parse_devices(&read_host_payload(socket)?)
    })?;
    if devices.iter().any(|device| {
        device.serial == binding.serial
            && device.status == DeviceStatus::Online
            && device.transport_id == binding.transport_id
    }) {
        Ok(())
    } else {
        Err(AdbError::Offline)
    }
}
pub(super) fn open(
    binding: &DeviceBinding,
    service: &[u8],
    mut check: impl FnMut() -> AdbResult<()>,
) -> AdbResult<TcpStream> {
    let mut stream = connect(binding, &mut check)?;
    if let Some(expected) = binding.transport_id {
        let selected = checked(&mut stream, &mut check, |socket| {
            request(
                socket,
                format!("host:tport:serial:{}", binding.serial).as_bytes(),
            )?;
            let mut identity = [0; 8];
            socket.read_exact(&mut identity)?;
            if u64::from_le_bytes(identity) != expected {
                return Err(AdbError::Offline);
            }
            Ok(())
        });
        match selected {
            Err(AdbError::Rejected) => {
                validate(binding, &mut check)?;
                stream = connect(binding, &mut check)?;
                checked(&mut stream, &mut check, |socket| {
                    request(
                        socket,
                        format!("host:transport:{}", binding.serial).as_bytes(),
                    )
                })?;
                validate(binding, &mut check)?;
            }
            result => result?,
        }
    } else {
        checked(&mut stream, &mut check, |socket| {
            request(
                socket,
                format!("host:transport:{}", binding.serial).as_bytes(),
            )
        })?;
    }
    checked(&mut stream, &mut check, |socket| request(socket, service))?;
    // Established protocols set their own timeout/cancellation policy.
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    stream.set_write_timeout(Some(Duration::from_secs(10)))?;
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use crate::adb::client::{DeviceBinding, open_service_with_cancel};
    use crate::error::AdbError;
    use crate::operation::AdbOperationControl;
    use std::io::Read as _;
    use std::time::{Duration, Instant};
    #[test]
    fn cancellation_interrupts_an_incomplete_host_handshake_without_waiting_for_socket_timeout() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let binding = DeviceBinding {
            server: listener.local_addr().unwrap(),
            serial: "fixture".into(),
            transport_id: None,
            generation: 1,
        };
        let control = AdbOperationControl::default();
        let cancel = control.clone();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0; 4];
            socket.read_exact(&mut request).unwrap();
            cancel.cancel();
            std::thread::sleep(Duration::from_millis(200));
        });
        let started = Instant::now();
        assert!(matches!(
            open_service_with_cancel(&binding, b"sync:", || control.check()),
            Err(AdbError::Cancelled)
        ));
        assert!(started.elapsed() < Duration::from_secs(1));
        server.join().unwrap();
    }
}
