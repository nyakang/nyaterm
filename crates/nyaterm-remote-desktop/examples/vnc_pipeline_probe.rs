//! Synthetic helper/CPU presentation probe; this does not measure GPU drawing.
use nyaterm_remote_desktop::{
    Framebuffer, RemoteFrameEvent, VNC_FRAMEBUFFER_LIMITS, VncClipboardConfig, VncDisplayConfig,
    VncReconnectConfig, VncRuntimeEvent, VncSecurityConfig, VncSessionConfig, VncSessionManager,
};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

fn main() {
    let arguments: Vec<_> = std::env::args().collect();
    let rectangles: u16 = arguments.get(1).map_or(1000, |s| s.parse().unwrap());
    let interval: u64 = arguments.get(2).map_or(16, |s| s.parse().unwrap());
    let (port, server) = spawn_no_auth_server((256, 256), move |stream| {
        push_update(stream, 256, 256, rectangles);
        Ok(())
    });
    let pipeline = arguments.get(3).is_some_and(|mode| mode == "pipeline");
    let presentation_config = nyaterm_remote_desktop::presentation::VncPresentationConfig {
        frame_interval: Duration::from_millis(interval),
    };
    let manager = if pipeline {
        VncSessionManager::with_presentation_pipeline(presentation_config)
    } else {
        VncSessionManager::new()
    };
    let id = manager.create_session(config(port)).unwrap();
    manager.set_presentation_visible(&id, true, presentation_config);
    let mut framebuffer = None;
    let mut pipeline_pixels = None;
    let mut received = 0;
    let start = Instant::now();
    let mut processing = Duration::ZERO;
    while received < usize::from(rectangles) {
        assert!(start.elapsed() < Duration::from_secs(180));
        let turn = Instant::now();
        let drain = manager.drain_batch(&id);
        for event in drain.control {
            match event {
                VncRuntimeEvent::Frame {
                    event:
                        RemoteFrameEvent::Reset {
                            epoch,
                            width,
                            height,
                        },
                    ..
                } => {
                    if !pipeline {
                        framebuffer = Some(
                            Framebuffer::new(epoch, width, height, VNC_FRAMEBUFFER_LIMITS).unwrap(),
                        );
                    }
                }
                VncRuntimeEvent::Error {
                    error, fatal: true, ..
                } => panic!("{error}"),
                _ => {}
            }
        }
        for frame in drain.frames {
            framebuffer.as_mut().unwrap().apply(&frame).unwrap();
            received += 1;
        }
        if pipeline && let Some(presentation) = manager.take_presentation(&id) {
            assert!(presentation.full);
            assert_eq!(presentation.regions.len(), 1);
            pipeline_pixels = Some(presentation.regions[0].pixels.clone());
            received = usize::from(rectangles);
            manager.acknowledge_presentation(&id, presentation.id);
        }
        processing += turn.elapsed();
        if received < usize::from(rectangles) {
            std::thread::sleep(Duration::from_millis(interval));
        }
    }
    let elapsed = start.elapsed();
    let pixels = if pipeline {
        pipeline_pixels.as_ref().unwrap().as_ref()
    } else {
        framebuffer.as_ref().unwrap().pixels()
    };
    let expected = usize::from(rectangles) * 4;
    assert!(
        pixels[..expected]
            .as_chunks::<4>()
            .0
            .iter()
            .all(|p| *p == [0xc0, 0x80, 0x40, 0xff])
    );
    manager.close(&id).unwrap();
    server.join().unwrap().unwrap();
    println!(
        "{{\"mode\":\"{}\",\"rectangles\":{rectangles},\"interval_ms\":{interval},\"elapsed_ms\":{},\"consumer_cpu_ms\":{}}}",
        if pipeline { "pipeline" } else { "legacy" },
        elapsed.as_secs_f64() * 1000.0,
        processing.as_secs_f64() * 1000.0
    );
}

fn config(port: u16) -> VncSessionConfig {
    VncSessionConfig {
        username: String::new(),
        relay: None,
        name: "manager".to_string(),
        host: "127.0.0.1".to_string(),
        port,
        password: None,
        security: VncSecurityConfig::default(),
        display: VncDisplayConfig::default(),
        clipboard: VncClipboardConfig::default(),
        // Fail on the first attempt instead of walking the reconnect ladder.
        reconnect: VncReconnectConfig {
            enabled: false,
            max_attempts: 0,
        },
        shared: true,
        view_only: false,
    }
}
fn spawn_no_auth_server(
    framebuffer: (u16, u16),
    send_first_frame: impl FnOnce(&mut TcpStream) -> std::io::Result<()> + Send + 'static,
) -> (u16, std::thread::JoinHandle<std::io::Result<()>>) {
    use std::io::{ErrorKind, Read as _, Write as _};

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || -> std::io::Result<()> {
        let (mut stream, _) = listener.accept()?;
        stream.set_read_timeout(Some(Duration::from_secs(180)))?;
        stream.write_all(b"RFB 003.008\n")?;
        let mut version = [0; 12];
        stream.read_exact(&mut version)?;
        assert_eq!(&version, b"RFB 003.008\n");
        // One security type: None (1).
        stream.write_all(&[1, 1])?;
        let mut selected = [0; 1];
        stream.read_exact(&mut selected)?;
        assert_eq!(selected, [1]);
        stream.write_all(&0_u32.to_be_bytes())?;
        let mut shared = [0; 1];
        stream.read_exact(&mut shared)?;
        assert_eq!(shared, [1]);
        // A true-colour framebuffer with an empty desktop name. The client overrides
        // the pixel format, so these components only have to be well formed.
        let (width, height) = framebuffer;
        let mut server_init = Vec::new();
        server_init.extend_from_slice(&width.to_be_bytes());
        server_init.extend_from_slice(&height.to_be_bytes());
        server_init.extend_from_slice(&[32, 24, 0, 1, 0, 255, 0, 255, 0, 255, 16, 8, 0, 0, 0, 0]);
        server_init.extend_from_slice(&0_u32.to_be_bytes());
        stream.write_all(&server_init)?;

        let mut send_first_frame = Some(send_first_frame);
        loop {
            let mut message = [0; 1];
            match stream.read_exact(&mut message) {
                Ok(()) => {}
                Err(error) if error.kind() == ErrorKind::UnexpectedEof => return Ok(()),
                Err(error) => return Err(error),
            }
            match message[0] {
                0 => stream.read_exact(&mut [0; 19])?,
                2 => {
                    let mut header = [0; 3];
                    stream.read_exact(&mut header)?;
                    let count = usize::from(u16::from_be_bytes([header[1], header[2]]));
                    stream.read_exact(&mut vec![0; count * 4])?;
                }
                3 => {
                    stream.read_exact(&mut [0; 9])?;
                    if let Some(send_first_frame) = send_first_frame.take() {
                        send_first_frame(&mut stream)?;
                    }
                }
                other => panic!("unexpected client message {other}"),
            }
        }
    });
    (port, server)
}

/// Append one `FramebufferUpdate` carrying `rectangles` 1x1 Raw rectangles.
fn push_update(stream: &mut TcpStream, width: u16, height: u16, rectangles: u16) {
    use std::io::Write as _;

    let mut update = Vec::new();
    update.push(0); // FramebufferUpdate
    update.push(0); // padding
    update.extend_from_slice(&rectangles.to_be_bytes());
    for index in 0..rectangles {
        let x = index % width;
        let y = (index / width) % height;
        update.extend_from_slice(&x.to_be_bytes());
        update.extend_from_slice(&y.to_be_bytes());
        update.extend_from_slice(&1_u16.to_be_bytes());
        update.extend_from_slice(&1_u16.to_be_bytes());
        update.extend_from_slice(&0_i32.to_be_bytes()); // Raw encoding
        update.extend_from_slice(&[0x40, 0x80, 0xc0, 0xff]);
    }
    stream.write_all(&update).unwrap();
}
