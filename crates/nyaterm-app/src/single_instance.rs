use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context as _, bail};
use nyaterm_core::{
    ACTIVATION_QUEUE_CAPACITY, ActivationAck, ActivationAckStatus, ActivationQueueError,
    ActivationReceiver, ActivationRequest, ActivationSender, MAX_ACTIVATION_FRAME_BYTES,
    activation_channel, decode_activation_ack, decode_activation_request, encode_activation_ack,
    encode_activation_request,
};

const OWNER_LOCK_NAME: &str = "owner.lock";
const ENDPOINT_NAME: &str = "activation.endpoint";
const CONNECT_DEADLINE: Duration = Duration::from_secs(3);
pub(crate) const RESTART_HANDOFF_DEADLINE: Duration = Duration::from_secs(60);
const IO_TIMEOUT: Duration = Duration::from_secs(1);
const RETRY_DELAY: Duration = Duration::from_millis(40);

pub(crate) enum SingleInstanceOutcome {
    Owner(SingleInstanceOwner),
    Forwarded,
}

pub(crate) struct SingleInstanceOwner {
    lock_file: File,
    endpoint_path: PathBuf,
    stop: Arc<AtomicBool>,
    listener_thread: Option<JoinHandle<()>>,
    activation_tx: ActivationSender,
    activation_rx: Option<ActivationReceiver>,
}

impl SingleInstanceOwner {
    pub(crate) fn activation_sender(&self) -> ActivationSender {
        self.activation_tx.clone()
    }

    pub(crate) fn take_activation_receiver(&mut self) -> ActivationReceiver {
        self.activation_rx
            .take()
            .expect("activation receiver can only be taken once")
    }
}

impl Drop for SingleInstanceOwner {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(listener_thread) = self.listener_thread.take() {
            let _ = listener_thread.join();
        }
        let _ = std::fs::remove_file(&self.endpoint_path);
        let _ = self.lock_file.unlock();
    }
}

pub(crate) fn acquire(
    instance_dir: &Path,
    initial_request: ActivationRequest,
) -> anyhow::Result<SingleInstanceOutcome> {
    acquire_linked(instance_dir, initial_request, None)
}

pub(crate) fn acquire_linked(
    instance_dir: &Path,
    initial_request: ActivationRequest,
    linked_sender: Option<ActivationSender>,
) -> anyhow::Result<SingleInstanceOutcome> {
    std::fs::create_dir_all(instance_dir).with_context(|| {
        format!(
            "create single-instance directory '{}'",
            instance_dir.display()
        )
    })?;
    restrict_instance_directory(instance_dir)?;
    let lock_path = instance_dir.join(OWNER_LOCK_NAME);
    let endpoint_path = instance_dir.join(ENDPOINT_NAME);
    let started = Instant::now();

    loop {
        let lock_file = open_private_file(&lock_path)?;
        match lock_file.try_lock() {
            Ok(()) => {
                return start_owner(lock_file, endpoint_path, linked_sender)
                    .map(SingleInstanceOutcome::Owner);
            }
            Err(std::fs::TryLockError::WouldBlock) => {}
            Err(std::fs::TryLockError::Error(error)) => {
                return Err(error).context("acquire NyaTerm single-instance lock");
            }
        }

        if let Some(endpoint) = read_endpoint(&endpoint_path)?
            && forward_activation(endpoint, &initial_request).is_ok()
        {
            return Ok(SingleInstanceOutcome::Forwarded);
        }
        if started.elapsed() >= CONNECT_DEADLINE {
            bail!(
                "another NyaTerm instance owns this data directory but did not accept activation"
            );
        }
        thread::sleep(RETRY_DELAY);
    }
}

/// Waits until the process that owned `instance_dir` has exited.
///
/// A restart spawns its successor while the old process is still shutting down.
/// Blocking on the old owner lock keeps the successor from applying a pending
/// data-directory migration while the database is still open, and keeps it from
/// forwarding its activation to the dying process and exiting.
pub(crate) fn wait_for_previous_instance(
    instance_dir: &Path,
    deadline: Duration,
) -> anyhow::Result<()> {
    let lock_path = instance_dir.join(OWNER_LOCK_NAME);
    let started = Instant::now();
    loop {
        if !lock_path.exists() {
            return Ok(());
        }
        let lock_file = open_private_file(&lock_path)?;
        match lock_file.try_lock() {
            Ok(()) => {
                let _ = lock_file.unlock();
                return Ok(());
            }
            Err(std::fs::TryLockError::WouldBlock) => {}
            Err(std::fs::TryLockError::Error(error)) => {
                return Err(error).context("wait for the previous NyaTerm instance");
            }
        }
        if started.elapsed() >= deadline {
            bail!("the previous NyaTerm instance did not exit in time");
        }
        thread::sleep(RETRY_DELAY);
    }
}

fn start_owner(
    lock_file: File,
    endpoint_path: PathBuf,
    linked_sender: Option<ActivationSender>,
) -> anyhow::Result<SingleInstanceOwner> {
    let _ = std::fs::remove_file(&endpoint_path);
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .context("bind NyaTerm activation listener")?;
    listener
        .set_nonblocking(true)
        .context("configure NyaTerm activation listener")?;
    let address = listener.local_addr()?;
    let auth_token = *uuid::Uuid::new_v4().as_bytes();
    write_endpoint(&endpoint_path, address, auth_token)?;

    let (activation_tx, activation_rx) = activation_channel(ACTIVATION_QUEUE_CAPACITY);
    let activation_tx = linked_sender.unwrap_or(activation_tx);
    let stop = Arc::new(AtomicBool::new(false));
    let listener_stop = Arc::clone(&stop);
    let listener_activation_tx = activation_tx.clone();
    let listener_thread = thread::Builder::new()
        .name("nyaterm-activation-listener".to_string())
        .spawn(move || run_listener(listener, auth_token, listener_activation_tx, listener_stop))
        .context("spawn NyaTerm activation listener")?;

    Ok(SingleInstanceOwner {
        lock_file,
        endpoint_path,
        stop,
        listener_thread: Some(listener_thread),
        activation_tx,
        activation_rx: Some(activation_rx),
    })
}

fn run_listener(
    listener: TcpListener,
    auth_token: [u8; 16],
    activation_tx: ActivationSender,
    stop: Arc<AtomicBool>,
) {
    while !stop.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, _)) => {
                let sender = activation_tx.clone();
                thread::spawn(move || handle_connection(stream, auth_token, sender));
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(20));
            }
            Err(_) => thread::sleep(Duration::from_millis(50)),
        }
    }
    activation_tx.close();
}

fn handle_connection(mut stream: TcpStream, auth_token: [u8; 16], activation_tx: ActivationSender) {
    // Accepted sockets inherit the listener's nonblocking mode on macOS.
    // Reading the frame through EOF requires blocking I/O with a timeout.
    if stream.set_nonblocking(false).is_err() {
        return;
    }
    let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
    let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
    let mut frame = Vec::new();
    let read_result = (&mut stream)
        .take((MAX_ACTIVATION_FRAME_BYTES + 1) as u64)
        .read_to_end(&mut frame);
    let (request_id, status) = match read_result
        .ok()
        .filter(|_| frame.len() <= MAX_ACTIVATION_FRAME_BYTES)
        .and_then(|_| decode_activation_request(&frame).ok())
    {
        Some((received_token, request)) if received_token == auth_token => {
            let request_id = request.request_id;
            let status = match activation_tx.try_send(request) {
                Ok(()) => ActivationAckStatus::Accepted,
                Err(ActivationQueueError::Full) => ActivationAckStatus::QueueFull,
                Err(ActivationQueueError::Closed) => ActivationAckStatus::ShuttingDown,
            };
            (request_id, status)
        }
        Some((_, request)) => (request.request_id, ActivationAckStatus::InvalidRequest),
        None => ([0; 16], ActivationAckStatus::InvalidRequest),
    };
    if let Ok(ack) = encode_activation_ack(ActivationAck { request_id, status }) {
        let _ = stream.write_all(&ack);
        let _ = stream.flush();
    }
}

#[derive(Clone, Copy)]
struct ActivationEndpoint {
    address: SocketAddr,
    auth_token: [u8; 16],
}

fn forward_activation(
    endpoint: ActivationEndpoint,
    request: &ActivationRequest,
) -> anyhow::Result<()> {
    let frame = encode_activation_request(endpoint.auth_token, request)?;
    let mut stream = TcpStream::connect_timeout(&endpoint.address, IO_TIMEOUT)
        .context("connect to primary NyaTerm instance")?;
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    stream.write_all(&frame)?;
    stream.shutdown(Shutdown::Write)?;
    let mut ack = Vec::new();
    (&mut stream)
        .take((MAX_ACTIVATION_FRAME_BYTES + 1) as u64)
        .read_to_end(&mut ack)?;
    let ack = decode_activation_ack(&ack)?;
    if ack.request_id != request.request_id {
        bail!("primary NyaTerm instance returned an unrelated activation ACK");
    }
    match ack.status {
        ActivationAckStatus::Accepted => Ok(()),
        ActivationAckStatus::QueueFull => bail!("primary NyaTerm activation queue is full"),
        ActivationAckStatus::ShuttingDown => bail!("primary NyaTerm instance is shutting down"),
        ActivationAckStatus::InvalidRequest => {
            bail!("primary NyaTerm instance rejected the activation request")
        }
    }
}

fn write_endpoint(
    endpoint_path: &Path,
    address: SocketAddr,
    auth_token: [u8; 16],
) -> anyhow::Result<()> {
    let temporary_path = endpoint_path.with_extension(format!("tmp-{}", std::process::id()));
    let token = auth_token
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let content = format!("NTSI1\n{}\n{token}\n", address.port());
    let mut file = open_private_file(&temporary_path)?;
    file.set_len(0)?;
    file.write_all(content.as_bytes())?;
    file.sync_all()?;
    std::fs::rename(&temporary_path, endpoint_path).with_context(|| {
        format!(
            "publish NyaTerm activation endpoint '{}'",
            endpoint_path.display()
        )
    })
}

fn read_endpoint(endpoint_path: &Path) -> anyhow::Result<Option<ActivationEndpoint>> {
    let content = match std::fs::read_to_string(endpoint_path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("read NyaTerm activation endpoint"),
    };
    let mut lines = content.lines();
    if lines.next() != Some("NTSI1") {
        return Ok(None);
    }
    let Some(port) = lines.next().and_then(|value| value.parse::<u16>().ok()) else {
        return Ok(None);
    };
    let Some(token) = lines.next().and_then(parse_token) else {
        return Ok(None);
    };
    Ok(Some(ActivationEndpoint {
        address: SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, port)),
        auth_token: token,
    }))
}

fn parse_token(value: &str) -> Option<[u8; 16]> {
    if value.len() != 32 {
        return None;
    }
    let mut token = [0; 16];
    for (index, byte) in token.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(token)
}

fn open_private_file(path: &Path) -> anyhow::Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    options
        .open(path)
        .with_context(|| format!("open private instance file '{}'", path.display()))
}

#[cfg_attr(not(unix), allow(unused_variables))]
fn restrict_instance_directory(path: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).with_context(
            || {
                format!(
                    "restrict single-instance directory permissions '{}'",
                    path.display()
                )
            },
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::{Read as _, Write as _};
    use std::net::{Shutdown, TcpListener, TcpStream};
    use std::time::Duration;

    use futures::FutureExt as _;

    use nyaterm_core::{
        ActivationAck, ActivationAckStatus, ActivationRequest, RawActivationArg,
        activation_channel, decode_activation_ack, encode_activation_request,
    };

    use super::{
        ActivationEndpoint, SingleInstanceOutcome, acquire, forward_activation, handle_connection,
        read_endpoint, wait_for_previous_instance,
    };

    fn request(id: u8) -> ActivationRequest {
        ActivationRequest {
            request_id: [id; 16],
            args: vec![RawActivationArg::Bytes(vec![id])],
        }
    }

    fn temporary_root(test_name: &str) -> nyaterm_core::test_support::TestTempDir {
        let root = nyaterm_core::test_support::TestTempDir::new(&format!(
            "nyaterm-single-instance-{test_name}"
        ));
        std::fs::create_dir_all(root.path()).unwrap();
        root
    }

    fn send_frame(endpoint: ActivationEndpoint, frame: &[u8]) -> ActivationAck {
        let mut stream = TcpStream::connect(endpoint.address).unwrap();
        stream.write_all(frame).unwrap();
        stream.shutdown(Shutdown::Write).unwrap();
        let mut ack = Vec::new();
        stream.read_to_end(&mut ack).unwrap();
        decode_activation_ack(&ack).unwrap()
    }

    #[test]
    fn stable_and_preview_can_own_instances_simultaneously() {
        let root = temporary_root("flavor-isolation");
        let stable = nyaterm_core::runtime::AppRuntime::installed_config_dir(
            &root,
            nyaterm_core::app_identity::AppFlavor::Stable,
        );
        let preview = nyaterm_core::runtime::AppRuntime::installed_config_dir(
            &root,
            nyaterm_core::app_identity::AppFlavor::Preview,
        );
        let stable_owner = acquire(&stable, request(1)).unwrap();
        let preview_owner = acquire(&preview, request(2)).unwrap();
        assert!(matches!(stable_owner, SingleInstanceOutcome::Owner(_)));
        assert!(matches!(preview_owner, SingleInstanceOutcome::Owner(_)));
        assert!(matches!(
            acquire(&stable, request(3)).unwrap(),
            SingleInstanceOutcome::Forwarded
        ));
        assert!(matches!(
            acquire(&preview, request(4)).unwrap(),
            SingleInstanceOutcome::Forwarded
        ));
        drop(stable_owner);
        drop(preview_owner);
    }

    #[test]
    fn secondary_forwards_to_owner_and_lock_recovers_after_drop() {
        let root = temporary_root("handoff");

        let SingleInstanceOutcome::Owner(mut owner) = acquire(&root, request(1)).unwrap() else {
            panic!("first process must own the instance");
        };
        let mut receiver = owner.take_activation_receiver();
        assert!(receiver.recv().now_or_never().is_none());
        assert!(matches!(
            acquire(&root, request(2)).unwrap(),
            SingleInstanceOutcome::Forwarded
        ));
        assert_eq!(
            futures::executor::block_on(receiver.recv()),
            Some(request(2))
        );
        drop(receiver);
        drop(owner);

        let third = acquire(&root, request(3)).unwrap();
        assert!(matches!(third, SingleInstanceOutcome::Owner(_)));
        drop(third);
    }

    #[test]
    fn nonblocking_accepted_socket_waits_for_the_complete_activation_frame() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, _) = listener.accept().unwrap();
        // Reproduce macOS accept inheritance even on platforms that clear it.
        server.set_nonblocking(true).unwrap();
        let (sender, mut receiver) = activation_channel(1);
        let token = [7; 16];
        let request = request(4);
        let frame = encode_activation_request(token, &request).unwrap();
        client.write_all(&frame[..1]).unwrap();
        let handler = std::thread::spawn(move || handle_connection(server, token, sender));

        client
            .set_read_timeout(Some(Duration::from_millis(100)))
            .unwrap();
        let error = client.read(&mut [0; 1]).unwrap_err();
        assert!(matches!(
            error.kind(),
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
        ));
        client.write_all(&frame[1..]).unwrap();
        client.shutdown(Shutdown::Write).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut ack = Vec::new();
        client.read_to_end(&mut ack).unwrap();
        handler.join().unwrap();
        let ack = decode_activation_ack(&ack).unwrap();
        assert_eq!(ack.request_id, request.request_id);
        assert_eq!(ack.status, ActivationAckStatus::Accepted);
        assert_eq!(receiver.recv().now_or_never(), Some(Some(request)));
    }

    #[test]
    fn invalid_auth_and_malformed_frames_are_rejected_without_enqueueing() {
        let root = temporary_root("rejection");
        let SingleInstanceOutcome::Owner(mut owner) = acquire(&root, request(1)).unwrap() else {
            panic!("first process must own the instance");
        };
        let mut receiver = owner.take_activation_receiver();
        let endpoint = read_endpoint(&owner.endpoint_path).unwrap().unwrap();

        let mut wrong_token = endpoint.auth_token;
        wrong_token[0] ^= 0xff;
        let rejected_request = request(2);
        let rejected_ack = send_frame(
            endpoint,
            &encode_activation_request(wrong_token, &rejected_request).unwrap(),
        );
        assert_eq!(rejected_ack.request_id, rejected_request.request_id);
        assert_eq!(rejected_ack.status, ActivationAckStatus::InvalidRequest);

        let malformed_ack = send_frame(endpoint, b"not-an-activation-frame");
        assert_eq!(malformed_ack.request_id, [0; 16]);
        assert_eq!(malformed_ack.status, ActivationAckStatus::InvalidRequest);

        forward_activation(endpoint, &request(3)).unwrap();
        assert_eq!(
            futures::executor::block_on(receiver.recv()),
            Some(request(3))
        );

        drop(receiver);
        drop(owner);
    }

    #[test]
    fn successor_waits_for_the_previous_instance_to_release_its_lock() {
        let root = temporary_root("restart-handoff");
        let SingleInstanceOutcome::Owner(owner) = acquire(&root, request(1)).unwrap() else {
            panic!("first process must own the instance");
        };
        assert!(
            wait_for_previous_instance(&root, Duration::from_millis(150)).is_err(),
            "a live owner must not be mistaken for a finished shutdown"
        );
        drop(owner);
        wait_for_previous_instance(&root, Duration::from_secs(2))
            .expect("a released instance lock must let the successor continue");
    }
}
