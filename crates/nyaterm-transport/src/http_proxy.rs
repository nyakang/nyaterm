use async_http_proxy::HttpError;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

pub(crate) async fn connect<IO>(
    stream: &mut IO,
    host: &str,
    port: u16,
    credentials: Option<(&str, &str)>,
) -> Result<(), HttpError>
where
    IO: AsyncRead + AsyncWrite + Unpin,
{
    let mut stream = HandshakeStream(stream);
    match credentials {
        Some((username, password)) => {
            async_http_proxy::http_connect_tokio_with_basic_auth(
                &mut stream,
                host,
                port,
                username,
                password,
            )
            .await
        }
        None => async_http_proxy::http_connect_tokio(&mut stream, host, port).await,
    }
}

// async-http-proxy drops its BufStream after parsing the response. Restrict
// handshake reads so that a coalesced server greeting stays in the underlying
// stream. Callers resume unrestricted reads when connect returns.
struct HandshakeStream<'a, IO>(&'a mut IO);

impl<IO: AsyncRead + Unpin> AsyncRead for HandshakeStream<'_, IO> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        let mut byte = [0];
        let mut read = ReadBuf::new(&mut byte);
        let result = Pin::new(&mut *self.0).poll_read(cx, &mut read);
        if let Poll::Ready(Ok(())) = result {
            buf.put_slice(read.filled());
        }
        result
    }
}

impl<IO: AsyncWrite + Unpin> AsyncWrite for HandshakeStream<'_, IO> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut *self.0).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut *self.0).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut *self.0).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::connect;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _, duplex};

    #[tokio::test]
    async fn connect_response_coalesced_with_server_greeting_preserves_every_byte() {
        for credentials in [None, Some(("proxy-user", "proxy-password"))] {
            for greeting in [b"\xff\xfb\x03Username: ".as_slice(), b"SSH-2.0-test\r\n"] {
                let (mut client, mut proxy) = duplex(16 * 1024);
                let mut response = b"HTTP/1.1 200 Connection Established\r\n\r\n".to_vec();
                response.extend_from_slice(greeting);
                proxy.write_all(&response).await.unwrap();
                proxy.shutdown().await.unwrap();

                connect(&mut client, "device.internal", 23, credentials)
                    .await
                    .unwrap();
                let mut received = vec![0; greeting.len()];
                client.read_exact(&mut received).await.unwrap();
                assert_eq!(received, greeting);
            }
        }
    }
}
