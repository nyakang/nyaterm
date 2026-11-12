//! A local ADB server fixture. It never starts an ADB process or accesses a device.
use crate::adb::AdbFileService;
use nyaterm_android::adb::client::DeviceBinding;
use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread::JoinHandle;

#[derive(Clone)]
pub(crate) struct Node {
    pub mode: u32,
    pub data: Vec<u8>,
    pub modified: u32,
}

#[derive(Default)]
pub(crate) struct Device {
    pub nodes: BTreeMap<Vec<u8>, Node>,
    pub receives: usize,
    pub sends: usize,
    pub fail_sends: usize,
    pub fail_receives: usize,
    pub renames: usize,
    pub fail_rename: bool,
    pub permission_probe: Option<Vec<u8>>,
    pub ignore_chmod: bool,
    pub preserve_change_time: bool,
    pub change_on_receive: bool,
    pub change_on_send: Option<(Vec<u8>, Vec<u8>)>,
}

pub(crate) struct AdbFixture {
    pub service: Arc<AdbFileService>,
    pub device: Arc<Mutex<Device>>,
    stopped: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl AdbFixture {
    pub fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let device = Arc::new(Mutex::new(Device::default()));
        for path in [b"/".as_slice(), b"/device"] {
            device.lock().unwrap().nodes.insert(
                path.to_vec(),
                Node {
                    mode: 0o040777,
                    data: Vec::new(),
                    modified: 7,
                },
            );
        }
        let stopped = Arc::new(AtomicBool::new(false));
        let stop = stopped.clone();
        let state = device.clone();
        let worker = std::thread::spawn(move || {
            let mut connections = Vec::new();
            while !stop.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((socket, _)) => {
                        let state = state.clone();
                        connections.push(std::thread::spawn(move || {
                            let _ = handle(socket, state);
                        }));
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(1))
                    }
                    Err(error) => panic!("fixture accept: {error}"),
                }
            }
            for connection in connections {
                connection.join().unwrap();
            }
        });
        Self {
            service: Arc::new(AdbFileService::new(DeviceBinding {
                server: address,
                serial: "fixture".into(),
                transport_id: None,
                generation: 1,
            })),
            device,
            stopped,
            worker: Some(worker),
        }
    }

    pub fn file(&self, path: &[u8], data: &[u8]) {
        self.device.lock().unwrap().nodes.insert(
            path.to_vec(),
            Node {
                mode: 0o100644,
                data: data.to_vec(),
                modified: 7,
            },
        );
    }
}

impl Drop for AdbFixture {
    fn drop(&mut self) {
        self.service.cancel();
        self.stopped.store(true, Ordering::Release);
        self.worker.take().unwrap().join().unwrap();
    }
}

fn host_request(socket: &mut TcpStream) -> io::Result<Vec<u8>> {
    let mut length = [0; 4];
    socket.read_exact(&mut length)?;
    let length = usize::from_str_radix(std::str::from_utf8(&length).unwrap(), 16).unwrap();
    let mut bytes = vec![0; length];
    socket.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn handle(mut socket: TcpStream, state: Arc<Mutex<Device>>) -> io::Result<()> {
    socket.set_nonblocking(false)?;
    socket.set_read_timeout(Some(std::time::Duration::from_secs(2)))?;
    socket.set_write_timeout(Some(std::time::Duration::from_secs(2)))?;
    let request = host_request(&mut socket)?;
    socket.write_all(b"OKAY")?;
    if request.ends_with(b":features") {
        return socket.write_all(b"0007stat_v2");
    }
    assert!(request.starts_with(b"host:transport:"));
    let request = host_request(&mut socket)?;
    socket.write_all(b"OKAY")?;
    if let Some(command) = request.strip_prefix(b"shell,v2,raw:") {
        let args = shell_paths(command);
        let mut state = state.lock().unwrap();
        let mut exit = 0;
        if command.starts_with(b"realpath --") {
            let probe = state.permission_probe.clone().unwrap_or_else(|| {
                b"/device\n\0ext4\n\x001 0 0:1 / / rw - ext4 /dev/block rw\n".to_vec()
            });
            socket.write_all(&[1])?;
            socket.write_all(&(probe.len() as u32).to_le_bytes())?;
            socket.write_all(&probe)?;
        } else if command.starts_with(b"set -C;") {
            if state.nodes.contains_key(&args[0]) {
                exit = 1;
            } else {
                state.nodes.insert(
                    args[0].clone(),
                    Node {
                        mode: 0o100644,
                        data: Vec::new(),
                        modified: 7,
                    },
                );
            }
        } else if command.starts_with(b"mkdir --") {
            state.nodes.insert(
                args[0].clone(),
                Node {
                    mode: 0o040777,
                    data: Vec::new(),
                    modified: 7,
                },
            );
        } else if command.starts_with(b"mv ") {
            state.renames += 1;
            if state.fail_rename {
                return socket.write_all(&[3, 1, 0, 0, 0, 1]);
            }
            if !command.starts_with(b"mv -n") || !state.nodes.contains_key(&args[1]) {
                if let Some(node) = state.nodes.remove(&args[0]) {
                    state.nodes.insert(args[1].clone(), node);
                } else {
                    exit = 1;
                }
            }
        } else if command.starts_with(b"chmod ") {
            let mode = std::str::from_utf8(command)
                .unwrap()
                .split_whitespace()
                .nth(1)
                .unwrap();
            let mode = u32::from_str_radix(mode, 8).unwrap();
            if !state.ignore_chmod {
                state.nodes.get_mut(&args[0]).unwrap().mode = 0o100000 | mode;
            }
        } else if command.starts_with(b"touch ") {
            let time = std::str::from_utf8(&args[0])
                .unwrap()
                .trim_start_matches('@')
                .parse::<u32>()
                .unwrap();
            state.nodes.get_mut(&args[1]).unwrap().modified = time;
        } else if command.starts_with(b"rm -") {
            state.nodes.remove(&args[0]);
        } else {
            panic!("unexpected fixture shell operation");
        }
        return socket.write_all(&[3, 1, 0, 0, 0, exit]);
    }
    assert_eq!(request, b"sync:");
    loop {
        let mut tag = [0; 4];
        match socket.read_exact(&mut tag) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(error) => return Err(error),
        };
        let mut length = [0; 4];
        socket.read_exact(&mut length)?;
        let mut path = vec![0; u32::from_le_bytes(length) as usize];
        socket.read_exact(&mut path)?;
        match &tag {
            b"LST2" => {
                let node = state.lock().unwrap().nodes.get(&path).cloned();
                let mut data = [0u8; 68];
                if let Some(node) = node {
                    data[20..24].copy_from_slice(&node.mode.to_le_bytes());
                    data[28..32].copy_from_slice(&2000u32.to_le_bytes());
                    data[32..36].copy_from_slice(&2000u32.to_le_bytes());
                    data[36..44].copy_from_slice(&(node.data.len() as u64).to_le_bytes());
                    data[52..60].copy_from_slice(&(node.modified as u64).to_le_bytes());
                } else {
                    data[0..4].copy_from_slice(&2u32.to_le_bytes());
                }
                socket.write_all(b"LST2")?;
                socket.write_all(&data)?;
            }
            b"STAT" => {
                let node = state.lock().unwrap().nodes.get(&path).cloned();
                socket.write_all(b"STAT")?;
                for value in node.map_or([0, 0, 0], |node| {
                    [node.mode, node.data.len() as u32, node.modified]
                }) {
                    socket.write_all(&value.to_le_bytes())?;
                }
            }
            b"LIST" => {
                let mut prefix = path;
                if !prefix.ends_with(b"/") {
                    prefix.push(b'/');
                }
                for (path, node) in &state.lock().unwrap().nodes {
                    if let Some(name) = path
                        .strip_prefix(prefix.as_slice())
                        .filter(|name| !name.is_empty() && !name.contains(&b'/'))
                    {
                        socket.write_all(b"DENT")?;
                        for value in [
                            node.mode,
                            node.data.len() as u32,
                            node.modified,
                            name.len() as u32,
                        ] {
                            socket.write_all(&value.to_le_bytes())?;
                        }
                        socket.write_all(name)?;
                    }
                }
                socket.write_all(b"DONE")?;
                socket.write_all(&[0; 16])?;
            }
            b"RECV" => {
                let mut state = state.lock().unwrap();
                let data = state.nodes.get(&path).unwrap().data.clone();
                state.receives += 1;
                let fail = state.fail_receives > 0;
                state.fail_receives = state.fail_receives.saturating_sub(1);
                if state.change_on_receive {
                    state.nodes.get_mut(&path).unwrap().modified += 1;
                    state.change_on_receive = false;
                }
                drop(state);
                for chunk in data.chunks(64 * 1024) {
                    socket.write_all(b"DATA")?;
                    socket.write_all(&(chunk.len() as u32).to_le_bytes())?;
                    socket.write_all(chunk)?;
                    if fail {
                        return Ok(());
                    }
                }
                socket.write_all(b"DONE\0\0\0\0")?;
            }
            b"SEND" => {
                let comma = path.iter().rposition(|byte| *byte == b',').unwrap();
                let mode = std::str::from_utf8(&path[comma + 1..])
                    .unwrap()
                    .parse::<u32>()
                    .unwrap();
                path.truncate(comma);
                let mut data = Vec::new();
                loop {
                    socket.read_exact(&mut tag)?;
                    socket.read_exact(&mut length)?;
                    if &tag == b"DONE" {
                        break;
                    }
                    assert_eq!(&tag, b"DATA");
                    let mut chunk = vec![0; u32::from_le_bytes(length) as usize];
                    socket.read_exact(&mut chunk)?;
                    data.extend(chunk);
                }
                let mut state = state.lock().unwrap();
                state.sends += 1;
                state.nodes.insert(
                    path,
                    Node {
                        mode: 0o100000 | mode | ((mode & 0o700) >> 3) | ((mode & 0o700) >> 6),
                        data,
                        modified: u32::from_le_bytes(length),
                    },
                );
                let preserve_time = state.preserve_change_time;
                if let Some((path, data)) = state.change_on_send.take() {
                    let node = state.nodes.get_mut(&path).unwrap();
                    node.data = data;
                    if !preserve_time {
                        node.modified += 1;
                    }
                }
                if state.fail_sends > 0 {
                    state.fail_sends -= 1;
                    return Ok(());
                }
                socket.write_all(b"OKAY\0\0\0\0")?;
            }
            _ => panic!("unexpected fixture Sync request"),
        }
    }
}

fn shell_paths(command: &[u8]) -> Vec<Vec<u8>> {
    let mut args = Vec::new();
    let mut cursor = 0;
    while cursor < command.len() {
        if command[cursor] != b'\'' {
            cursor += 1;
            continue;
        }
        let mut arg = Vec::new();
        loop {
            cursor += 1;
            while cursor < command.len() && command[cursor] != b'\'' {
                arg.push(command[cursor]);
                cursor += 1;
            }
            cursor += 1;
            if command.get(cursor..cursor + 3) == Some(b"\\''") {
                arg.push(b'\'');
                cursor += 2;
            } else {
                break;
            }
        }
        args.push(arg);
    }
    args
}
