use std::io::{self, IoSliceMut};
use std::net::{SocketAddr, UdpSocket};

use quinn_udp::{EcnCodepoint, RecvMeta, Transmit, UdpSockRef, UdpSocketState};
use socket2::SockRef;

/// A complete stock-mosh instruction can be 4 MiB before protocol decoding.
/// Leave room for fragment headers and a concurrent retransmission burst so
/// the kernel queue does not discard fragments before the poll loop sees them.
const DESIRED_RECEIVE_QUEUE_BYTES: usize = 8 * 1024 * 1024;

fn request_large_receive_queue(socket: &UdpSocket) {
    let socket = SockRef::from(socket);
    for bytes in [
        DESIRED_RECEIVE_QUEUE_BYTES,
        4 * 1024 * 1024,
        2 * 1024 * 1024,
    ] {
        if socket.set_recv_buffer_size(bytes).is_ok() {
            break;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReceivedDatagram {
    pub offset: usize,
    pub len: usize,
    pub ecn: Option<EcnCodepoint>,
}

pub(crate) struct EcnSocket {
    socket: UdpSocket,
    peer: SocketAddr,
    ecn: Option<UdpSocketState>,
}

impl EcnSocket {
    pub fn bind(peer: SocketAddr) -> io::Result<Self> {
        let socket = UdpSocket::bind(if peer.is_ipv4() {
            "0.0.0.0:0"
        } else {
            "[::]:0"
        })?;
        socket.set_nonblocking(true)?;
        // The OS may clamp this to its configured maximum. Requesting the
        // larger queue is still important when the default UDP queue is much
        // smaller than one legal mosh instruction.
        request_large_receive_queue(&socket);
        // Old Windows versions and restricted sandboxes may not expose the
        // ancillary-data APIs. Preserve basic UDP operation in that case.
        let ecn = UdpSocketState::new(UdpSockRef::from(&socket)).ok();
        Ok(Self { socket, peer, ecn })
    }

    #[cfg(test)]
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    pub fn peer_addr(&self) -> SocketAddr {
        self.peer
    }

    pub fn send(&self, bytes: &[u8]) -> io::Result<usize> {
        self.send_with_ecn(bytes, EcnCodepoint::Ect0)
    }

    pub fn receive_buffer_size(&self, max_datagram_size: usize) -> usize {
        let segments = self.ecn.as_ref().map_or(1, UdpSocketState::gro_segments);
        max_datagram_size.saturating_mul(segments).max(1)
    }

    fn send_with_ecn(&self, bytes: &[u8], ecn: EcnCodepoint) -> io::Result<usize> {
        let Some(state) = &self.ecn else {
            return self.socket.send_to(bytes, self.peer);
        };
        let result = state.try_send(
            UdpSockRef::from(&self.socket),
            &Transmit {
                destination: self.peer,
                ecn: Some(ecn),
                contents: bytes,
                segment_size: None,
                src_ip: None,
            },
        );
        match result {
            Ok(()) => Ok(bytes.len()),
            // Windows explicitly rejects sending CE with WSAEINVAL, and some
            // Windows IPv6 stacks reject the per-packet ECN control message
            // even after ECN receive setup succeeded. ECN is optional for the
            // session, so preserve UDP connectivity without the codepoint.
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::InvalidInput | io::ErrorKind::Unsupported
                ) =>
            {
                self.socket.send_to(bytes, self.peer)
            }
            Err(error) => Err(error),
        }
    }

    pub fn recv(&self, buffer: &mut [u8]) -> io::Result<Vec<ReceivedDatagram>> {
        let Some(state) = &self.ecn else {
            let (len, _) = self.socket.recv_from(buffer)?;
            return Ok(vec![ReceivedDatagram {
                offset: 0,
                len,
                ecn: None,
            }]);
        };

        let mut buffers = [IoSliceMut::new(buffer)];
        let mut metadata = [RecvMeta::default()];
        let count = state.recv(UdpSockRef::from(&self.socket), &mut buffers, &mut metadata)?;
        if count == 0 {
            return Err(io::Error::from(io::ErrorKind::WouldBlock));
        }

        let meta = metadata[0];
        if meta.len > buffer.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "UDP GRO batch exceeded the receive buffer",
            ));
        }
        let stride = meta.stride.max(1);
        let mut datagrams = Vec::with_capacity(meta.len.div_ceil(stride));
        let mut offset = 0;
        while offset < meta.len {
            datagrams.push(ReceivedDatagram {
                offset,
                len: stride.min(meta.len - offset),
                ecn: meta.ecn,
            });
            offset += stride;
        }
        Ok(datagrams)
    }
}

#[cfg(test)]
mod tests {
    use super::{EcnCodepoint, EcnSocket, ReceivedDatagram, SocketAddr, UdpSocketState, io};
    use std::time::{Duration, Instant};

    fn receive(receiver: &EcnSocket) -> ReceivedDatagram {
        let deadline = Instant::now() + Duration::from_secs(1);
        let mut buffer = [0u8; 64];
        loop {
            match receiver.recv(&mut buffer) {
                Ok(mut datagrams) => return datagrams.remove(0),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    assert!(
                        Instant::now() < deadline,
                        "timed out waiting for loopback UDP"
                    );
                    std::thread::sleep(Duration::from_millis(2));
                }
                Err(error) => panic!("loopback receive failed: {error}"),
            }
        }
    }

    fn pair() -> (EcnSocket, EcnSocket) {
        let receiver = EcnSocket::bind("127.0.0.1:9".parse().unwrap()).unwrap();
        let receiver_addr =
            SocketAddr::from(([127, 0, 0, 1], receiver.local_addr().unwrap().port()));
        let sender = EcnSocket::bind(receiver_addr).unwrap();
        (sender, receiver)
    }

    fn ipv6_pair() -> Option<(EcnSocket, EcnSocket)> {
        let receiver = EcnSocket::bind("[::1]:9".parse().unwrap()).ok()?;
        let receiver_addr = SocketAddr::new(
            std::net::Ipv6Addr::LOCALHOST.into(),
            receiver.local_addr().ok()?.port(),
        );
        let sender = EcnSocket::bind(receiver_addr).ok()?;
        Some((sender, receiver))
    }

    #[test]
    fn receive_buffer_covers_the_platform_gro_batch() {
        let (_, receiver) = pair();
        let segments = receiver
            .ecn
            .as_ref()
            .map_or(1, UdpSocketState::gro_segments);

        assert_eq!(receiver.receive_buffer_size(2048), 2048 * segments);
    }

    #[test]
    fn outgoing_mosh_datagrams_are_marked_ecn_capable() {
        let (sender, receiver) = pair();
        if sender.ecn.is_none() || receiver.ecn.is_none() {
            return;
        }
        sender.send(b"ect0").unwrap();

        assert_eq!(receive(&receiver).ecn, Some(EcnCodepoint::Ect0));
    }

    #[test]
    fn congestion_experienced_codepoint_is_received_when_the_platform_allows_sending_it() {
        let (sender, receiver) = pair();
        if sender.ecn.is_none() || receiver.ecn.is_none() {
            return;
        }
        sender
            .send_with_ecn(b"congested", EcnCodepoint::Ce)
            .unwrap();

        let received = receive(&receiver);
        #[cfg(windows)]
        assert_eq!(received.ecn, None);
        #[cfg(not(windows))]
        assert_eq!(received.ecn, Some(EcnCodepoint::Ce));
    }

    #[test]
    fn ipv6_loopback_preserves_ecn_codepoints() {
        let Some((sender, receiver)) = ipv6_pair() else {
            return;
        };
        if sender.ecn.is_none() || receiver.ecn.is_none() {
            return;
        }

        sender.send(b"ipv6-ect0").unwrap();
        let ect0 = receive(&receiver).ecn;
        #[cfg(windows)]
        assert!(matches!(ect0, None | Some(EcnCodepoint::Ect0)));
        #[cfg(not(windows))]
        assert_eq!(ect0, Some(EcnCodepoint::Ect0));
        sender.send_with_ecn(b"ipv6-ce", EcnCodepoint::Ce).unwrap();
        let ce = receive(&receiver).ecn;
        #[cfg(windows)]
        assert_eq!(ce, None);
        #[cfg(not(windows))]
        assert_eq!(ce, Some(EcnCodepoint::Ce));
    }
}
