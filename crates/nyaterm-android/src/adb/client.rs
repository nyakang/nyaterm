mod controlled;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use nyaterm_core::models::android::AdbDeviceSelector;
use serde::{Deserialize, Serialize};

use crate::error::{AdbError, AdbResult};

const REQUEST_LIMIT: usize = 65_535;
const COMMAND_OUTPUT_LIMIT: u64 = 64 * 1024;
pub const SERVER_PROTOCOL_VERSION: u32 = 41;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeviceStatus {
    Online,
    Offline,
    Unauthorized,
    Unavailable,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdbDevice {
    pub serial: String,
    pub transport_id: Option<u64>,
    pub model: Option<String>,
    pub status: DeviceStatus,
}

/// Immutable binding. Reconnection creates a new binding and generation.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeviceBinding {
    pub server: SocketAddr,
    pub serial: String,
    pub transport_id: Option<u64>,
    pub generation: u64,
}

#[derive(Clone, Debug)]
pub struct AdbClient {
    pub executable: PathBuf,
    pub server: SocketAddr,
}

impl AdbClient {
    pub fn local(custom: Option<&Path>) -> AdbResult<Self> {
        Ok(Self {
            executable: resolve_executable(custom)?,
            server: SocketAddr::from(([127, 0, 0, 1], 5037)),
        })
    }

    fn connect(&self) -> AdbResult<TcpStream> {
        let stream = TcpStream::connect_timeout(&self.server, Duration::from_secs(2))?;
        stream.set_read_timeout(Some(Duration::from_secs(10)))?;
        stream.set_write_timeout(Some(Duration::from_secs(10)))?;
        stream.set_nodelay(true)?;
        Ok(stream)
    }

    pub fn ensure_server(&self) -> AdbResult<()> {
        match self.version() {
            Ok(SERVER_PROTOCOL_VERSION) => return Ok(()),
            Ok(_) => return Err(AdbError::IncompatibleServer),
            Err(AdbError::Io(error)) if error.kind() == std::io::ErrorKind::ConnectionRefused => {}
            Err(error) => return Err(error),
        }
        // Never invoke start-server against an existing incompatible server:
        // adb itself would terminate it, disturbing other applications.
        self.command(&["start-server"])?;
        if self.version()? != SERVER_PROTOCOL_VERSION {
            return Err(AdbError::IncompatibleServer);
        }
        Ok(())
    }

    pub fn version(&self) -> AdbResult<u32> {
        let mut stream = self.connect()?;
        request(&mut stream, b"host:version")?;
        let data = read_host_payload(&mut stream)?;
        u32::from_str_radix(
            std::str::from_utf8(&data).map_err(|_| AdbError::Protocol("server version"))?,
            16,
        )
        .map_err(|_| AdbError::Protocol("server version"))
    }

    pub fn devices(&self) -> AdbResult<Vec<AdbDevice>> {
        let mut stream = self.connect()?;
        request(&mut stream, b"host:devices-l")?;
        parse_devices(&read_host_payload(&mut stream)?)
    }

    pub fn tracker(&self) -> AdbResult<TcpStream> {
        let mut stream = self.connect()?;
        request(&mut stream, b"host:track-devices-l")?;
        stream.set_read_timeout(Some(Duration::from_millis(200)))?;
        Ok(stream)
    }

    pub fn bind(&self, selector: &AdbDeviceSelector, generation: u64) -> AdbResult<DeviceBinding> {
        selector.validate().map_err(AdbError::Protocol)?;
        self.ensure_server()?;
        let serial = selector.serial();
        if matches!(selector, AdbDeviceSelector::Tcp { .. }) {
            self.command(&["connect", &serial])?;
        }
        let device = self
            .devices()?
            .into_iter()
            .find(|device| device.serial == serial)
            .ok_or(AdbError::Offline)?;
        match device.status {
            DeviceStatus::Unauthorized => return Err(AdbError::Unauthorized),
            DeviceStatus::Online => {}
            _ => return Err(AdbError::Offline),
        }
        Ok(DeviceBinding {
            server: self.server,
            serial,
            transport_id: device.transport_id,
            generation,
        })
    }

    pub fn command(&self, args: &[&str]) -> AdbResult<String> {
        self.command_with_control(args, &crate::operation::AdbOperationControl::default())
    }
    pub fn command_with_control(
        &self,
        args: &[&str],
        control: &crate::operation::AdbOperationControl,
    ) -> AdbResult<String> {
        control.check()?;
        let mut command = self.process(args);
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        let control = control.with_timeout(Duration::from_secs(15));
        let worker = nyaterm_core::runtime::worker::spawn("adb-command".into(), move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            runtime.block_on(async move {
                use tokio::io::AsyncReadExt as _;
                let mut command = tokio::process::Command::from(command);
                let mut child = command.kill_on_drop(true).spawn()?;
                let stdout = child
                    .stdout
                    .take()
                    .ok_or(AdbError::Protocol("command stdout"))?;
                let stderr = child
                    .stderr
                    .take()
                    .ok_or(AdbError::Protocol("command stderr"))?;
                async fn read(reader: impl tokio::io::AsyncRead + Unpin) -> AdbResult<Vec<u8>> {
                    let mut bytes = Vec::new();
                    reader
                        .take(COMMAND_OUTPUT_LIMIT + 1)
                        .read_to_end(&mut bytes)
                        .await?;
                    if bytes.len() > COMMAND_OUTPUT_LIMIT as usize {
                        return Err(AdbError::Protocol("command output limit"));
                    }
                    Ok(bytes)
                }
                let result = {
                    let io = async {
                        let (output, errors, status) =
                            tokio::try_join!(read(stdout), read(stderr), async {
                                child.wait().await.map_err(AdbError::from)
                            })?;
                        drop(errors);
                        if !status.success() {
                            return Err(AdbError::Rejected);
                        }
                        String::from_utf8(output)
                            .map_err(|_| AdbError::Protocol("command output encoding"))
                    };
                    tokio::select! {
                        result = io => result,
                        error = control.until_stopped() => Err(error),
                    }
                };
                if result.is_err() {
                    let _ = child.start_kill();
                    let _ = tokio::time::timeout(Duration::from_secs(1), child.wait()).await;
                }
                result
            })
        })?;
        worker
            .join()
            .map_err(|_| AdbError::Protocol("command worker panic"))?
    }

    pub fn process(&self, args: &[&str]) -> Command {
        let mut command = Command::new(&self.executable);
        // Even -H 127.0.0.1 makes adb classify the socket as remote and refuse
        // daemon startup. Use its local socket spelling for the default server.
        if self.server.ip() != std::net::Ipv4Addr::LOCALHOST {
            command.args(["-H", &self.server.ip().to_string()]);
        }
        command
            .args(["-P", &self.server.port().to_string()])
            .args(args);
        // Native channels and subprocesses must address the same explicit
        // server; an inherited socket override would silently split the binding.
        command.env_remove("ADB_SERVER_SOCKET");
        command.stdin(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        command
    }

    pub fn device_command(&self, binding: &DeviceBinding, args: &[&str]) -> AdbResult<String> {
        self.device_command_with_control(
            binding,
            args,
            &crate::operation::AdbOperationControl::default(),
        )
    }
    pub fn device_command_with_control(
        &self,
        binding: &DeviceBinding,
        args: &[&str],
        control: &crate::operation::AdbOperationControl,
    ) -> AdbResult<String> {
        control.check()?;
        controlled::validate(binding, &mut || control.check())?;
        control.check()?;
        let mut all = vec!["-s", binding.serial.as_str()];
        all.extend_from_slice(args);
        self.command_with_control(&all, control)
    }
}

pub fn resolve_executable(custom: Option<&Path>) -> AdbResult<PathBuf> {
    if let Some(path) = custom.filter(|path| !path.as_os_str().is_empty()) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if !path.is_file() || std::fs::metadata(path)?.permissions().mode() & 0o111 == 0 {
                return Err(AdbError::ExecutableMissing);
            }
        }
        return path
            .is_file()
            .then(|| path.to_path_buf())
            .ok_or(AdbError::ExecutableMissing);
    }
    let name = if cfg!(windows) { "adb.exe" } else { "adb" };
    let bundled = std::env::current_exe()?
        .parent()
        .ok_or(AdbError::ExecutableMissing)?
        .join("adb")
        .join(name);
    if bundled.is_file() {
        verify_bundled_adb(&bundled)?;
        return Ok(bundled);
    }
    if cfg!(debug_assertions) {
        if let Some(path) = std::env::var_os("PATH").and_then(|path| {
            std::env::split_paths(&path)
                .map(|dir| dir.join(name))
                .find(|path| path.is_file())
        }) {
            return Ok(path);
        }
        for variable in ["ANDROID_SDK_ROOT", "ANDROID_HOME"] {
            if let Some(root) = std::env::var_os(variable) {
                let path = PathBuf::from(root).join("platform-tools").join(name);
                if path.is_file() {
                    return Ok(path);
                }
            }
        }
    }
    Err(AdbError::ExecutableMissing)
}

fn verify_bundled_adb(executable: &Path) -> AdbResult<()> {
    let root = executable
        .parent()
        .and_then(Path::parent)
        .ok_or(AdbError::ExecutableMissing)?;
    let manifest = root.join("android-resources.json");
    if !manifest.is_file() && cfg!(debug_assertions) {
        return Ok(());
    }
    let metadata = std::fs::metadata(&manifest)?;
    if metadata.len() > 128 * 1024 {
        return Err(AdbError::Protocol("resource manifest length"));
    }
    let document: serde_json::Value = serde_json::from_slice(&std::fs::read(manifest)?)
        .map_err(|_| AdbError::Protocol("resource manifest"))?;
    let files = document
        .get("files")
        .and_then(serde_json::Value::as_object)
        .ok_or(AdbError::Protocol("resource inventory"))?;
    let expected = if cfg!(windows) {
        vec![
            "adb/adb.exe",
            "adb/AdbWinApi.dll",
            "adb/AdbWinUsbApi.dll",
            "adb/NOTICE.txt",
            "adb/SOURCE.json",
        ]
    } else {
        vec!["adb/adb", "adb/NOTICE.txt", "adb/SOURCE.json"]
    };
    if expected.iter().any(|name| !files.contains_key(*name)) {
        return Err(AdbError::Protocol("ADB companions missing"));
    }
    for (name, hash) in files.iter().filter(|(name, _)| name.starts_with("adb/")) {
        if std::path::Path::new(name)
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
            || name.contains('\\')
        {
            return Err(AdbError::Protocol("resource path"));
        }
        use sha2::Digest;
        let bytes = std::fs::read(root.join(name))?;
        if Some(hex::encode(sha2::Sha256::digest(&bytes)).as_str()) != hash.as_str() {
            return Err(AdbError::Protocol("bundled ADB checksum"));
        }
    }
    Ok(())
}

pub fn device_features(binding: &DeviceBinding) -> AdbResult<Vec<String>> {
    device_features_with_cancel(binding, || Ok(()))
}
pub fn device_features_with_cancel(
    binding: &DeviceBinding,
    check: impl FnMut() -> AdbResult<()>,
) -> AdbResult<Vec<String>> {
    controlled::features(binding, check)
}
pub fn open_service(binding: &DeviceBinding, service: &[u8]) -> AdbResult<TcpStream> {
    open_service_with_cancel(binding, service, || Ok(()))
}
pub fn open_service_with_cancel(
    binding: &DeviceBinding,
    service: &[u8],
    check: impl FnMut() -> AdbResult<()>,
) -> AdbResult<TcpStream> {
    controlled::open(binding, service, check)
}

pub fn request(stream: &mut (impl Read + Write), data: &[u8]) -> AdbResult<()> {
    if data.len() > REQUEST_LIMIT {
        return Err(AdbError::Protocol("request length"));
    }
    stream.write_all(format!("{:04x}", data.len()).as_bytes())?;
    stream.write_all(data)?;
    let mut status = [0; 4];
    stream.read_exact(&mut status)?;
    match &status {
        b"OKAY" => Ok(()),
        b"FAIL" => {
            let message = read_host_payload(stream)?;
            let message = String::from_utf8_lossy(&message);
            if message.contains("unauthorized") {
                Err(AdbError::Unauthorized)
            } else if message.contains("offline") || message.contains("not found") {
                Err(AdbError::Offline)
            } else {
                Err(AdbError::Rejected)
            }
        }
        _ => Err(AdbError::Protocol("response status")),
    }
}

pub fn read_host_payload(reader: &mut impl Read) -> AdbResult<Vec<u8>> {
    let mut header = [0; 4];
    reader.read_exact(&mut header)?;
    let length = usize::from_str_radix(
        std::str::from_utf8(&header).map_err(|_| AdbError::Protocol("host length"))?,
        16,
    )
    .map_err(|_| AdbError::Protocol("host length"))?;
    let mut data = vec![0; length];
    reader.read_exact(&mut data)?;
    Ok(data)
}

pub fn parse_devices(data: &[u8]) -> AdbResult<Vec<AdbDevice>> {
    let text = std::str::from_utf8(data).map_err(|_| AdbError::Protocol("device list encoding"))?;
    text.lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let mut fields = line.split_whitespace();
            let serial = fields
                .next()
                .ok_or(AdbError::Protocol("device serial"))?
                .to_owned();
            let status = match fields.next() {
                Some("device") => DeviceStatus::Online,
                Some("offline") => DeviceStatus::Offline,
                Some("unauthorized") => DeviceStatus::Unauthorized,
                _ => DeviceStatus::Unavailable,
            };
            let mut device = AdbDevice {
                serial,
                transport_id: None,
                model: None,
                status,
            };
            for field in fields {
                if let Some(id) = field.strip_prefix("transport_id:") {
                    device.transport_id = id.parse().ok();
                }
                if let Some(model) = field.strip_prefix("model:") {
                    device.model = Some(model.replace('_', " "));
                }
            }
            Ok(device)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use crate::adb::client::{AdbClient, DeviceStatus, parse_devices, resolve_executable};
    use crate::error::AdbError;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    #[test]
    fn incompatible_shared_server_is_rejected_without_invoking_an_executable() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let server = listener.local_addr().unwrap();
        let worker = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                .unwrap();
            let mut request = [0; 16];
            socket.read_exact(&mut request).unwrap();
            assert_eq!(&request, b"000chost:version");
            for byte in b"OKAY0004002a" {
                socket.write_all(&[*byte]).unwrap();
            }
        });
        let client = AdbClient {
            executable: "missing-fixture-adb".into(),
            server,
        };
        assert!(matches!(
            client.ensure_server(),
            Err(AdbError::IncompatibleServer)
        ));
        worker.join().unwrap();
    }
    #[test]
    fn local_daemon_startup_uses_adb_local_socket_semantics() {
        let client = AdbClient {
            executable: "fixture-adb".into(),
            server: "127.0.0.1:12345".parse().unwrap(),
        };
        let command = client.process(&["start-server"]);
        let args: Vec<_> = command.get_args().collect();
        assert_eq!(args, ["-P", "12345", "start-server"]);
        assert!(
            command
                .get_envs()
                .any(|(key, value)| key == "ADB_SERVER_SOCKET" && value.is_none())
        );
    }
    #[test]
    fn discovery_retains_authorization_and_transport_identity() {
        let devices = parse_devices(
            b"usb\tunauthorized transport_id:7\nip:123 device model:Pixel_9 transport_id:8\n",
        )
        .unwrap();
        assert_eq!(devices[0].status, DeviceStatus::Unauthorized);
        assert_eq!(devices[1].transport_id, Some(8));
        assert_eq!(devices[1].model.as_deref(), Some("Pixel 9"));
    }
    #[test]
    fn invalid_custom_executable_never_falls_back() {
        assert!(resolve_executable(Some(std::path::Path::new("/missing/nyaterm-adb"))).is_err());
    }

    #[test]
    #[cfg(unix)]
    fn custom_file_without_execute_permission_is_rejected_before_server_reuse() {
        let file = tempfile::NamedTempFile::new().unwrap();
        assert!(resolve_executable(Some(file.path())).is_err());
    }
    fn read_request(socket: &mut std::net::TcpStream) -> Vec<u8> {
        let mut length = [0; 4];
        socket.read_exact(&mut length).unwrap();
        let length = usize::from_str_radix(std::str::from_utf8(&length).unwrap(), 16).unwrap();
        let mut bytes = vec![0; length];
        socket.read_exact(&mut bytes).unwrap();
        bytes
    }
    #[test]
    fn native_device_selection_verifies_transport_on_the_same_serial_channel() {
        use crate::adb::client::{DeviceBinding, open_service};
        for (actual, succeeds) in [(7u64, true), (8, false)] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let binding = DeviceBinding {
                server: listener.local_addr().unwrap(),
                serial: "original".into(),
                transport_id: Some(7),
                generation: 1,
            };
            let worker = std::thread::spawn(move || {
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                    .unwrap();
                assert_eq!(read_request(&mut socket), b"host:tport:serial:original");
                socket.write_all(b"OKAY").unwrap();
                socket.write_all(&actual.to_le_bytes()).unwrap();
                if succeeds {
                    assert_eq!(read_request(&mut socket), b"sync:");
                    socket.write_all(b"OKAY").unwrap();
                } else {
                    assert_eq!(
                        socket.read(&mut [0; 1]).unwrap(),
                        0,
                        "no device request may follow a mismatched identity"
                    );
                }
            });
            assert_eq!(open_service(&binding, b"sync:").is_ok(), succeeds);
            worker.join().unwrap();
        }
    }
    #[cfg(unix)]
    #[test]
    fn inherited_output_pipes_cannot_extend_the_command_deadline() {
        use crate::operation::AdbOperationControl;
        use std::os::unix::fs::PermissionsExt as _;
        let path = std::env::temp_dir().join(format!("nyaterm-adb-pipe-{}", uuid::Uuid::new_v4()));
        std::fs::write(&path, "#!/bin/sh\nsleep 2 &\nexit 0\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        let client = AdbClient {
            executable: path.clone(),
            server: "127.0.0.1:5037".parse().unwrap(),
        };
        let started = std::time::Instant::now();
        assert!(matches!(
            client.command_with_control(
                &[],
                &AdbOperationControl::default().with_timeout(std::time::Duration::from_millis(100))
            ),
            Err(AdbError::Timeout)
        ));
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
        std::fs::remove_file(path).unwrap();
    }
}
