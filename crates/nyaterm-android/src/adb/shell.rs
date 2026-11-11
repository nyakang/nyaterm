use std::io::{Read, Write};
use std::net::TcpStream;

use crate::adb::client::DeviceBinding;
use crate::error::{AdbError, AdbResult};

pub const MAX_SHELL_PACKET: usize = 1024 * 1024;
pub const STDIN: u8 = 0;
pub const STDOUT: u8 = 1;
pub const STDERR: u8 = 2;
pub const EXIT: u8 = 3;
pub const CLOSE_STDIN: u8 = 4;
pub const WINDOW_SIZE: u8 = 5;

pub struct ShellPacket {
    pub id: u8,
    pub data: Vec<u8>,
}

#[derive(Default)]
pub struct ShellDecoder {
    buffer: Vec<u8>,
}

impl ShellDecoder {
    pub fn push(&mut self, bytes: &[u8]) -> AdbResult<Vec<ShellPacket>> {
        if bytes.len() > MAX_SHELL_PACKET
            || self.buffer.len() + bytes.len() > MAX_SHELL_PACKET * 2 + 10
        {
            return Err(AdbError::Protocol("shell buffer limit"));
        }
        self.buffer.extend_from_slice(bytes);
        let mut packets = Vec::new();
        let mut offset = 0;
        while self.buffer.len() - offset >= 5 {
            let id = self.buffer[offset];
            if id > WINDOW_SIZE {
                return Err(AdbError::Protocol("shell packet type"));
            }
            let length = u32::from_le_bytes(self.buffer[offset + 1..offset + 5].try_into().unwrap())
                as usize;
            if length > MAX_SHELL_PACKET {
                return Err(AdbError::Protocol("shell packet limit"));
            }
            if self.buffer.len() - offset < length + 5 {
                break;
            }
            if id == EXIT && length != 1 {
                return Err(AdbError::Protocol("shell exit status"));
            }
            packets.push(ShellPacket {
                id,
                data: self.buffer[offset + 5..offset + 5 + length].to_vec(),
            });
            offset += length + 5;
        }
        self.buffer.drain(..offset);
        Ok(packets)
    }

    pub fn finish(&self) -> AdbResult<()> {
        if self.buffer.is_empty() {
            Ok(())
        } else {
            Err(AdbError::Protocol("truncated shell packet"))
        }
    }
}

pub fn open_shell(binding: &DeviceBinding, command: &[u8], pty: bool) -> AdbResult<TcpStream> {
    open_shell_with_cancel(binding, command, pty, || Ok(()))
}
pub fn open_shell_with_cancel(
    binding: &DeviceBinding,
    command: &[u8],
    pty: bool,
    check: impl FnMut() -> AdbResult<()>,
) -> AdbResult<TcpStream> {
    let mut request = if pty {
        b"shell,v2,pty,TERM=xterm-256color:".to_vec()
    } else {
        b"shell,v2,raw:".to_vec()
    };
    request.extend_from_slice(command);
    crate::adb::client::open_service_with_cancel(binding, &request, check)
}

pub fn write_packet(writer: &mut impl Write, id: u8, data: &[u8]) -> AdbResult<()> {
    if id > WINDOW_SIZE || data.len() > MAX_SHELL_PACKET {
        return Err(AdbError::Protocol("shell write limit"));
    }
    writer.write_all(&[id])?;
    writer.write_all(&(data.len() as u32).to_le_bytes())?;
    writer.write_all(data)?;
    Ok(())
}

pub fn window_size(cols: u16, rows: u16, width: u16, height: u16) -> Vec<u8> {
    format!("{rows}x{cols},{width}x{height}\0").into_bytes()
}

/// Output remains bytes so callers do not lose filenames or UTF-8 boundaries.
pub fn execute(binding: &DeviceBinding, command: &[u8], limit: usize) -> AdbResult<Vec<u8>> {
    execute_with_cancel(binding, command, limit, || Ok(()))
}
pub fn execute_with_cancel(
    binding: &DeviceBinding,
    command: &[u8],
    limit: usize,
    mut check: impl FnMut() -> AdbResult<()>,
) -> AdbResult<Vec<u8>> {
    check()?;
    let mut stream = open_shell_with_cancel(binding, command, false, &mut check)?;
    stream.set_read_timeout(Some(std::time::Duration::from_millis(200)))?;
    let mut deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    let mut decoder = ShellDecoder::default();
    let mut output = Vec::new();
    let mut buffer = [0; 8192];
    loop {
        check()?;
        let count = match stream.read(&mut buffer) {
            Ok(count) => count,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                ) =>
            {
                if std::time::Instant::now() >= deadline {
                    return Err(AdbError::Timeout);
                }
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        if count == 0 {
            decoder.finish()?;
            return Err(AdbError::Protocol("shell exit missing"));
        }
        for packet in decoder.push(&buffer[..count])? {
            match packet.id {
                STDOUT => {
                    if output.len() + packet.data.len() > limit {
                        return Err(AdbError::Protocol("shell output limit"));
                    }
                    output.extend_from_slice(&packet.data);
                }
                STDERR => {} // Never include command context or device stderr in diagnostics.
                EXIT => {
                    return if packet.data[0] == 0 {
                        Ok(output)
                    } else {
                        Err(AdbError::ShellExit(packet.data[0]))
                    };
                }
                _ => return Err(AdbError::Protocol("unexpected shell response")),
            }
        }
    }
}

pub fn quote(path: &[u8]) -> AdbResult<Vec<u8>> {
    if path.contains(&0) {
        return Err(AdbError::Protocol("NUL in shell argument"));
    }
    let mut result = vec![b'\''];
    for byte in path {
        if *byte == b'\'' {
            result.extend_from_slice(b"'\\''");
        } else {
            result.push(*byte);
        }
    }
    result.push(b'\'');
    Ok(result)
}

#[cfg(test)]
mod tests {
    use crate::adb::shell::{
        EXIT, MAX_SHELL_PACKET, STDERR, STDOUT, ShellDecoder, quote, window_size, write_packet,
    };
    #[test]
    fn fragmented_and_coalesced_shell_packets_preserve_bytes_and_exit() {
        let mut data = Vec::new();
        write_packet(&mut data, STDOUT, "中文".as_bytes()).unwrap();
        write_packet(&mut data, STDERR, b"error").unwrap();
        write_packet(&mut data, EXIT, &[7]).unwrap();
        for chunk in 1..=data.len() {
            let mut decoder = ShellDecoder::default();
            let mut packets = Vec::new();
            for part in data.chunks(chunk) {
                packets.extend(decoder.push(part).unwrap());
            }
            decoder.finish().unwrap();
            assert_eq!(packets.len(), 3);
            assert_eq!(packets[0].data, "中文".as_bytes());
            assert_eq!(packets[2].data, [7]);
        }
    }
    #[test]
    fn oversized_and_truncated_packets_are_rejected() {
        let mut decoder = ShellDecoder::default();
        let mut header = vec![STDOUT];
        header.extend_from_slice(&(MAX_SHELL_PACKET as u32 + 1).to_le_bytes());
        assert!(decoder.push(&header).is_err());
        let mut decoder = ShellDecoder::default();
        decoder.push(&[STDOUT, 2, 0]).unwrap();
        assert!(decoder.finish().is_err());
    }
    #[test]
    fn shell_quoting_does_not_interpret_command_substitution() {
        assert_eq!(
            quote(b"a'$(touch /tmp/x)\n").unwrap(),
            b"'a'\\''$(touch /tmp/x)\n'"
        );
        assert!(quote(b"a\0b").is_err());
        assert_eq!(window_size(120, 40, 600, 400), b"40x120,600x400\0");
    }
}
