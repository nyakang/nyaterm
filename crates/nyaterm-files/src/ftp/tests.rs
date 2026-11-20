use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use nyaterm_core::character_encoding::CharacterEncoding;

use crate::file_browser::FileBrowserService;
use crate::ftp::config::{FtpConfig, FtpFilenameEncodingMode, FtpLogin};
use crate::ftp::error::FtpError;
use crate::ftp::service::FtpFileService;
use crate::remote_file::{FileCopyRequest, FileTransferEndpoint};
use crate::sftp::{RemoteFilePath, SFTP_TRANSFER_CANCELLED, SftpTransferControl};
use crate::sftp_transfer_types::{
    SftpDuplicateDecision, SftpDuplicatePolicy, SftpDuplicateRequest, SftpDuplicateResolver,
    SftpPathTransferOptions, SftpTransferOptions,
};

struct FakeServer {
    port: u16,
    commands: Arc<Mutex<Vec<String>>>,
}

impl FakeServer {
    fn start(
        utf8: bool,
        mlsd_supported: bool,
        reject_login: bool,
        data_encoding: CharacterEncoding,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake FTP server");
        let port = listener.local_addr().expect("fake FTP address").port();
        let commands = Arc::new(Mutex::new(Vec::new()));
        let command_log = Arc::clone(&commands);
        thread::spawn(move || {
            let Ok((control, _)) = listener.accept() else {
                return;
            };
            serve(
                control,
                utf8,
                mlsd_supported,
                reject_login,
                data_encoding,
                command_log,
            );
        });
        Self { port, commands }
    }

    fn config(&self, mode: FtpFilenameEncodingMode) -> FtpConfig {
        let mut config = FtpConfig::new("127.0.0.1");
        config.port = self.port;
        config.filename_encoding_mode = mode;
        config
    }

    fn commands(&self) -> Vec<String> {
        self.commands.lock().expect("command log").clone()
    }
}

fn serve(
    mut control: TcpStream,
    utf8: bool,
    mlsd_supported: bool,
    reject_login: bool,
    data_encoding: CharacterEncoding,
    commands: Arc<Mutex<Vec<String>>>,
) {
    let _ = control.write_all(b"220 fake FTP ready\r\n");
    let reader_stream = match control.try_clone() {
        Ok(stream) => stream,
        Err(_) => return,
    };
    let mut reader = BufReader::new(reader_stream);
    let mut data_listener: Option<TcpListener> = None;
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        let command = line.trim_end_matches(['\r', '\n']).to_string();
        commands
            .lock()
            .expect("record command")
            .push(command.clone());
        let upper = command.to_ascii_uppercase();
        let (verb, _) = upper.split_once(' ').unwrap_or((upper.as_str(), ""));
        match verb {
            "USER" => {
                let _ = control.write_all(b"331 password required\r\n");
            }
            "PASS" => {
                let response: &[u8] = if reject_login {
                    b"530 login incorrect\r\n"
                } else {
                    b"230 logged in\r\n"
                };
                let _ = control.write_all(response);
            }
            "FEAT" => {
                let response: &[u8] = if utf8 {
                    b"211-Features:\r\n UTF8\r\n211 End\r\n"
                } else {
                    b"211-Features:\r\n211 End\r\n"
                };
                let _ = control.write_all(response);
            }
            "OPTS" | "TYPE" => {
                let _ = control.write_all(b"200 ok\r\n");
            }
            "PWD" => {
                let _ = control.write_all(b"257 \"/\" is the current directory\r\n");
            }
            "PASV" => match TcpListener::bind("127.0.0.1:0") {
                Ok(listener) => {
                    let port = listener.local_addr().expect("data listener address").port();
                    data_listener = Some(listener);
                    // Deliberately advertise an address the client must ignore; the
                    // passive stream builder is expected to use the control peer.
                    let response = format!(
                        "227 Entering Passive Mode (10,0,0,1,{},{}).\r\n",
                        port / 256,
                        port % 256
                    );
                    let _ = control.write_all(response.as_bytes());
                }
                Err(_) => {
                    let _ = control.write_all(b"425 cannot open data connection\r\n");
                }
            },
            "EPSV" => match TcpListener::bind("127.0.0.1:0") {
                Ok(listener) => {
                    let port = listener.local_addr().expect("data listener address").port();
                    data_listener = Some(listener);
                    let response = format!("229 Entering Extended Passive Mode (|||{port}|)\r\n");
                    let _ = control.write_all(response.as_bytes());
                }
                Err(_) => {
                    let _ = control.write_all(b"425 cannot open data connection\r\n");
                }
            },
            "MLSD" => {
                if !mlsd_supported {
                    let _ = control.write_all(b"500 MLSD not supported\r\n");
                    continue;
                }
                send_listing(
                    &mut control,
                    &mut data_listener,
                    data_encoding,
                    &[
                        "type=dir;modify=20260101120000; sub",
                        "type=file;size=4; café.txt",
                    ],
                );
            }
            "LIST" => {
                send_listing(
                    &mut control,
                    &mut data_listener,
                    data_encoding,
                    &[
                        "drwxr-xr-x 2 user group 0 Jan 1 2026 sub",
                        "-rw-r--r-- 1 user group 4 Jan 1 2026 café.txt",
                    ],
                );
            }
            "RETR" => {
                send_listing_bytes(&mut control, &mut data_listener, b"test");
            }
            "STOR" => {
                receive_data(&mut control, &mut data_listener);
            }
            "RNFR" => {
                let _ = control.write_all(b"350 ready\r\n");
            }
            "RNTO" | "DELE" | "RMD" => {
                let _ = control.write_all(b"250 ok\r\n");
            }
            "MKD" => {
                let _ = control.write_all(b"257 \"/created\" created\r\n");
            }
            "MLST" => {
                let _ = control.write_all(b"500 MLST not supported\r\n");
            }
            "QUIT" => {
                let _ = control.write_all(b"221 bye\r\n");
                return;
            }
            _ => {
                let _ = control.write_all(b"502 not implemented\r\n");
            }
        }
    }
}

fn send_listing(
    control: &mut TcpStream,
    data_listener: &mut Option<TcpListener>,
    encoding: CharacterEncoding,
    lines: &[&str],
) {
    let _ = control.write_all(b"150 opening data connection\r\n");
    let Some(listener) = data_listener.take() else {
        let _ = control.write_all(b"425 data connection missing\r\n");
        return;
    };
    match listener.accept() {
        Ok((mut data, _)) => {
            for line in lines {
                let bytes = encoding.encode(line).expect("encode listing line");
                let _ = data.write_all(&bytes);
                let _ = data.write_all(b"\r\n");
            }
            let _ = data.flush();
            drop(data);
            let _ = control.write_all(b"226 transfer complete\r\n");
        }
        Err(_) => {
            let _ = control.write_all(b"425 data connection failed\r\n");
        }
    }
}

fn send_listing_bytes(
    control: &mut TcpStream,
    data_listener: &mut Option<TcpListener>,
    bytes: &[u8],
) {
    let _ = control.write_all(b"150 opening data connection\r\n");
    let Some(listener) = data_listener.take() else {
        let _ = control.write_all(b"425 data connection missing\r\n");
        return;
    };
    match listener.accept() {
        Ok((mut data, _)) => {
            let _ = data.write_all(bytes);
            let _ = data.flush();
            drop(data);
            let _ = control.write_all(b"226 transfer complete\r\n");
        }
        Err(_) => {
            let _ = control.write_all(b"425 data connection failed\r\n");
        }
    }
}

fn receive_data(control: &mut TcpStream, data_listener: &mut Option<TcpListener>) {
    let _ = control.write_all(b"150 opening data connection\r\n");
    let Some(listener) = data_listener.take() else {
        let _ = control.write_all(b"425 data connection missing\r\n");
        return;
    };
    match listener.accept() {
        Ok((mut data, _)) => {
            let mut buffer = Vec::new();
            let _ = std::io::Read::read_to_end(&mut data, &mut buffer);
            let _ = control.write_all(b"226 transfer complete\r\n");
        }
        Err(_) => {
            let _ = control.write_all(b"425 data connection failed\r\n");
        }
    }
}

#[test]
fn auto_detection_uses_feat_before_paths_and_rewrites_pasv_ip() {
    let server = FakeServer::start(true, true, false, CharacterEncoding::Utf8);
    let service = FtpFileService::connect_with_local_encoding(
        server.config(FtpFilenameEncodingMode::Auto),
        FtpLogin::anonymous(""),
        CharacterEncoding::Gbk,
    )
    .expect("connect to fake FTP server");

    assert_eq!(service.encoding(), CharacterEncoding::Utf8);
    let entries = service.list_dir("/").expect("list root");
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        vec!["sub", "café.txt"]
    );

    let commands = server.commands();
    let feat = commands
        .iter()
        .position(|command| command.eq_ignore_ascii_case("FEAT"))
        .expect("FEAT was sent");
    let pwd = commands
        .iter()
        .position(|command| command.eq_ignore_ascii_case("PWD"))
        .expect("PWD was sent");
    assert!(feat < pwd, "FEAT must precede path commands: {commands:?}");
    assert!(
        commands[..feat]
            .iter()
            .all(|command| !command.to_ascii_uppercase().starts_with("OPTS")),
        "OPTS must not be used for auto detection: {commands:?}"
    );
}

#[test]
fn ascii_mode_sets_the_ascii_transfer_type() {
    let server = FakeServer::start(true, true, false, CharacterEncoding::Utf8);
    let mut config = server.config(FtpFilenameEncodingMode::Auto);
    config.ascii_mode = true;
    FtpFileService::connect_with_local_encoding(
        config,
        FtpLogin::anonymous(""),
        CharacterEncoding::Utf8,
    )
    .expect("connect to fake FTP server");
    assert!(
        server
            .commands()
            .iter()
            .any(|command| command.eq_ignore_ascii_case("TYPE A")
                || command.eq_ignore_ascii_case("TYPE A N")),
        "ASCII mode must ask for the ASCII transfer type: {:?}",
        server.commands()
    );
}

#[test]
fn binary_mode_is_the_default_transfer_type() {
    let server = FakeServer::start(true, true, false, CharacterEncoding::Utf8);
    FtpFileService::connect_with_local_encoding(
        server.config(FtpFilenameEncodingMode::Auto),
        FtpLogin::anonymous(""),
        CharacterEncoding::Utf8,
    )
    .expect("connect to fake FTP server");
    assert!(
        server
            .commands()
            .iter()
            .any(|command| command.eq_ignore_ascii_case("TYPE I")),
        "binary mode must remain the default: {:?}",
        server.commands()
    );
}

#[test]
fn auto_without_utf8_uses_the_host_encoding() {
    let server = FakeServer::start(false, true, false, CharacterEncoding::Gbk);
    let service = FtpFileService::connect_with_local_encoding(
        server.config(FtpFilenameEncodingMode::Auto),
        FtpLogin::anonymous("anonymous"),
        CharacterEncoding::Gbk,
    )
    .expect("connect to fake FTP server");

    assert_eq!(service.encoding(), CharacterEncoding::Gbk);
    let entries = service.list_dir("/").expect("list root");
    assert!(entries.iter().any(|entry| entry.name == "café.txt"));
}

#[test]
fn custom_windows_1252_decodes_western_names() {
    let server = FakeServer::start(false, true, false, CharacterEncoding::Windows1252);
    let mut config = server.config(FtpFilenameEncodingMode::Custom);
    config.filename_encoding = "Windows-1252".to_string();
    let service = FtpFileService::connect_with_local_encoding(
        config,
        FtpLogin::anonymous(""),
        CharacterEncoding::Utf8,
    )
    .expect("connect to fake FTP server");

    assert_eq!(service.encoding(), CharacterEncoding::Windows1252);
    let entries = service.list_dir("/").expect("list root");
    assert!(entries.iter().any(|entry| entry.name == "café.txt"));
}

#[test]
fn list_falls_back_when_mlsd_is_rejected() {
    let server = FakeServer::start(false, false, false, CharacterEncoding::Utf8);
    let service = FtpFileService::connect_with_local_encoding(
        server.config(FtpFilenameEncodingMode::Auto),
        FtpLogin::anonymous(""),
        CharacterEncoding::Utf8,
    )
    .expect("connect to fake FTP server");

    let entries = service.list_dir("/").expect("fallback listing");
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        vec!["sub", "café.txt"]
    );
}

#[test]
fn anonymous_login_rejection_is_distinct_from_password_failure() {
    let server = FakeServer::start(true, true, true, CharacterEncoding::Utf8);
    let error = FtpFileService::connect_with_local_encoding(
        server.config(FtpFilenameEncodingMode::Auto),
        FtpLogin::anonymous(""),
        CharacterEncoding::Utf8,
    )
    .expect_err("anonymous login should be rejected");
    assert_eq!(error, FtpError::AnonymousLoginRejected);

    let server = FakeServer::start(true, true, true, CharacterEncoding::Utf8);
    let error = FtpFileService::connect_with_local_encoding(
        server.config(FtpFilenameEncodingMode::Auto),
        FtpLogin::password("user", "secret"),
        CharacterEncoding::Utf8,
    )
    .expect_err("password login should be rejected");
    assert_eq!(error, FtpError::AuthenticationFailed);
}

#[test]
fn custom_encoding_is_validated_and_aliases_are_normalized() {
    let mut config = FtpConfig::new("127.0.0.1");
    config.filename_encoding_mode = FtpFilenameEncodingMode::Custom;
    config.filename_encoding = "cp1252".to_string();
    assert_eq!(
        config.resolve_filename_encoding(CharacterEncoding::Utf8, false),
        Ok(CharacterEncoding::Windows1252)
    );
    config.filename_encoding = "KOI8-R".to_string();
    assert_eq!(
        config.resolve_filename_encoding(CharacterEncoding::Utf8, false),
        Err(FtpError::UnsupportedEncoding)
    );
}

#[test]
fn download_and_upload_copy_bytes_through_the_file_copy_contract() {
    let server = FakeServer::start(false, true, false, CharacterEncoding::Utf8);
    let service = Arc::new(
        FtpFileService::connect_with_local_encoding(
            server.config(FtpFilenameEncodingMode::Auto),
            FtpLogin::anonymous(""),
            CharacterEncoding::Utf8,
        )
        .expect("connect to fake FTP server"),
    );
    let temp_dir =
        std::env::temp_dir().join(format!("nyaterm-ftp-transfer-{}", nyaterm_core::uuid()));
    std::fs::create_dir_all(&temp_dir).expect("create transfer directory");
    let options = SftpPathTransferOptions::new(
        SftpDuplicatePolicy::Overwrite,
        None,
        SftpTransferOptions::default(),
    );

    let local = temp_dir.join("café.txt");
    let progress = Arc::new(Mutex::new(Vec::new()));
    let download_progress = Arc::clone(&progress);
    let summary = FileCopyRequest {
        source: FileTransferEndpoint::Ftp {
            service: Arc::clone(&service),
            path: RemoteFilePath::new("/café.txt"),
        },
        destination: FileTransferEndpoint::Local(local.clone()),
        control: SftpTransferControl::new(),
        options: options.clone(),
    }
    .execute_with_progress(move |event| download_progress.lock().expect("progress").push(event))
    .expect("download through the FTP copy contract");
    assert_eq!(summary.bytes, 4);
    assert_eq!(std::fs::read(&local).expect("download body"), b"test");
    {
        let events = progress.lock().expect("progress");
        assert!(
            events
                .iter()
                .any(|event| event.total_bytes == Some(4) && event.item_count_total == Some(1)),
            "downloads must report totals for percentage progress: {events:?}"
        );
    }

    let upload_local = temp_dir.join("upload.txt");
    std::fs::write(&upload_local, b"hello").expect("write upload fixture");
    progress.lock().expect("progress").clear();
    let upload_progress = Arc::clone(&progress);
    let summary = FileCopyRequest {
        source: FileTransferEndpoint::Local(upload_local),
        destination: FileTransferEndpoint::Ftp {
            service,
            path: RemoteFilePath::new("/upload.txt"),
        },
        control: SftpTransferControl::new(),
        options,
    }
    .execute_with_progress(move |event| upload_progress.lock().expect("progress").push(event))
    .expect("upload through the FTP copy contract");
    assert_eq!(summary.bytes, 5);
    {
        let events = progress.lock().expect("progress");
        assert!(
            events
                .iter()
                .any(|event| event.total_bytes == Some(5) && event.item_count_total == Some(1)),
            "uploads must report totals for percentage progress: {events:?}"
        );
    }

    let commands = server.commands();
    assert!(
        commands.iter().any(|command| {
            command
                .to_ascii_uppercase()
                .starts_with("STOR /UPLOAD.TXT.NYATERM-PART-")
        }),
        "uploads must write a temporary sibling first: {commands:?}"
    );
    assert!(commands.iter().any(|command| command.starts_with("RNFR ")));
    assert!(
        commands
            .iter()
            .any(|command| command.starts_with("RNTO /upload.txt"))
    );

    let _ = std::fs::remove_dir_all(temp_dir);
}

#[derive(Clone)]
enum FakeNode {
    File(Vec<u8>),
    Directory,
    Symlink,
}

/// Stateful fake FTP server used by the recursive and cancellation tests.
struct TreeServer {
    port: u16,
    commands: Arc<Mutex<Vec<String>>>,
    tree: Arc<Mutex<HashMap<String, FakeNode>>>,
}

impl TreeServer {
    fn start(entries: Vec<(&str, FakeNode)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind tree FTP server");
        let port = listener.local_addr().expect("tree FTP address").port();
        let commands = Arc::new(Mutex::new(Vec::new()));
        let tree = Arc::new(Mutex::new(HashMap::new()));
        {
            let mut nodes = tree.lock().expect("tree state");
            nodes.insert("/".to_string(), FakeNode::Directory);
            for (path, node) in entries {
                nodes.insert(path.to_string(), node);
            }
        }
        let command_log = Arc::clone(&commands);
        let server_tree = Arc::clone(&tree);
        thread::spawn(move || {
            let Ok((control, _)) = listener.accept() else {
                return;
            };
            serve_tree(control, command_log, server_tree);
        });
        Self {
            port,
            commands,
            tree,
        }
    }

    fn config(&self) -> FtpConfig {
        let mut config = FtpConfig::new("127.0.0.1");
        config.port = self.port;
        config
    }

    fn commands(&self) -> Vec<String> {
        self.commands.lock().expect("command log").clone()
    }

    fn contains(&self, path: &str) -> bool {
        self.tree.lock().expect("tree state").contains_key(path)
    }

    fn read(&self, path: &str) -> Option<Vec<u8>> {
        match self.tree.lock().expect("tree state").get(path) {
            Some(FakeNode::File(bytes)) => Some(bytes.clone()),
            _ => None,
        }
    }

    fn temporary_paths(&self) -> Vec<String> {
        self.tree
            .lock()
            .expect("tree state")
            .keys()
            .filter(|path| path.contains(".nyaterm-part-"))
            .cloned()
            .collect()
    }
}

fn connect_tree(server: &TreeServer) -> FtpFileService {
    FtpFileService::connect_with_local_encoding(
        server.config(),
        FtpLogin::anonymous(""),
        CharacterEncoding::Utf8,
    )
    .expect("connect to tree FTP server")
}

fn overwrite_options() -> SftpPathTransferOptions {
    SftpPathTransferOptions::new(
        SftpDuplicatePolicy::Overwrite,
        None,
        SftpTransferOptions::default(),
    )
}

fn test_directory(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("nyaterm-ftp-{label}-{}", nyaterm_core::uuid()));
    std::fs::create_dir_all(&path).expect("create test directory");
    path
}

fn serve_tree(
    mut control: TcpStream,
    commands: Arc<Mutex<Vec<String>>>,
    tree: Arc<Mutex<HashMap<String, FakeNode>>>,
) {
    let _ = control.write_all(b"220 fake FTP ready\r\n");
    let reader_stream = match control.try_clone() {
        Ok(stream) => stream,
        Err(_) => return,
    };
    let mut reader = BufReader::new(reader_stream);
    let mut data_listener: Option<TcpListener> = None;
    let mut rename_from: Option<String> = None;
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        let command = line.trim_end_matches(['\r', '\n']).to_string();
        commands
            .lock()
            .expect("record command")
            .push(command.clone());
        let upper = command.to_ascii_uppercase();
        let (verb, _) = upper.split_once(' ').unwrap_or((upper.as_str(), ""));
        let argument = command
            .split_once(' ')
            .map(|(_, value)| value)
            .unwrap_or("");
        match verb {
            "USER" => {
                let _ = control.write_all(b"331 password required\r\n");
            }
            "PASS" => {
                let _ = control.write_all(b"230 logged in\r\n");
            }
            "FEAT" => {
                let _ = control.write_all(b"211-Features:\r\n UTF8\r\n211 End\r\n");
            }
            "OPTS" | "TYPE" => {
                let _ = control.write_all(b"200 ok\r\n");
            }
            "PWD" => {
                let _ = control.write_all(b"257 \"/\" is the current directory\r\n");
            }
            "PASV" => match TcpListener::bind("127.0.0.1:0") {
                Ok(listener) => {
                    let port = listener.local_addr().expect("data listener address").port();
                    data_listener = Some(listener);
                    let response = format!(
                        "227 Entering Passive Mode (10,0,0,1,{},{}).\r\n",
                        port / 256,
                        port % 256
                    );
                    let _ = control.write_all(response.as_bytes());
                }
                Err(_) => {
                    let _ = control.write_all(b"425 cannot open data connection\r\n");
                }
            },
            "EPSV" => match TcpListener::bind("127.0.0.1:0") {
                Ok(listener) => {
                    let port = listener.local_addr().expect("data listener address").port();
                    data_listener = Some(listener);
                    let response = format!("229 Entering Extended Passive Mode (|||{port}|)\r\n");
                    let _ = control.write_all(response.as_bytes());
                }
                Err(_) => {
                    let _ = control.write_all(b"425 cannot open data connection\r\n");
                }
            },
            "MLSD" | "LIST" => {
                let path = normalize_tree_path(argument);
                let machine = verb == "MLSD";
                let lines = tree_listing(&tree, &path, machine);
                let references: Vec<&str> = lines.iter().map(String::as_str).collect();
                send_listing(
                    &mut control,
                    &mut data_listener,
                    CharacterEncoding::Utf8,
                    &references,
                );
            }
            "RETR" => {
                let path = normalize_tree_path(argument);
                let bytes = match tree.lock().expect("tree state").get(&path) {
                    Some(FakeNode::File(bytes)) => bytes.clone(),
                    _ => {
                        let _ = control.write_all(b"550 not found\r\n");
                        continue;
                    }
                };
                send_tree_body(&mut control, &mut data_listener, &bytes);
            }
            "STOR" => {
                let path = normalize_tree_path(argument);
                let bytes = receive_tree_body(&mut control, &mut data_listener);
                tree.lock()
                    .expect("tree state")
                    .insert(path, FakeNode::File(bytes));
                let _ = control.write_all(b"226 transfer complete\r\n");
            }
            "RNFR" => {
                rename_from = Some(normalize_tree_path(argument));
                let _ = control.write_all(b"350 ready\r\n");
            }
            "RNTO" => {
                let target = normalize_tree_path(argument);
                if let Some(from) = rename_from.take() {
                    let mut nodes = tree.lock().expect("tree state");
                    if let Some(node) = nodes.remove(&from) {
                        nodes.insert(target.clone(), node);
                    }
                    let prefix = format!("{from}/");
                    let moved: Vec<(String, FakeNode)> = nodes
                        .iter()
                        .filter(|(path, _)| path.starts_with(&prefix))
                        .map(|(path, node)| (path.clone(), node.clone()))
                        .collect();
                    for (path, node) in moved {
                        nodes.remove(&path);
                        let suffix = &path[prefix.len()..];
                        nodes.insert(format!("{target}/{suffix}"), node);
                    }
                }
                let _ = control.write_all(b"250 ok\r\n");
            }
            "MKD" => {
                let path = normalize_tree_path(argument);
                tree.lock()
                    .expect("tree state")
                    .insert(path, FakeNode::Directory);
                let _ = control.write_all(b"257 \"/created\" created\r\n");
            }
            "RMD" | "DELE" => {
                let path = normalize_tree_path(argument);
                tree.lock().expect("tree state").remove(&path);
                let _ = control.write_all(b"250 ok\r\n");
            }
            "MLST" => {
                let _ = control.write_all(b"500 MLST not supported\r\n");
            }
            "QUIT" => {
                let _ = control.write_all(b"221 bye\r\n");
                return;
            }
            _ => {
                let _ = control.write_all(b"502 not implemented\r\n");
            }
        }
    }
}

fn normalize_tree_path(argument: &str) -> String {
    let trimmed = argument.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        "/".to_string()
    } else if trimmed.starts_with('/') {
        trimmed.to_string()
    } else {
        format!("/{trimmed}")
    }
}

fn tree_listing(
    tree: &Arc<Mutex<HashMap<String, FakeNode>>>,
    path: &str,
    machine: bool,
) -> Vec<String> {
    let nodes = tree.lock().expect("tree state");
    let mut entries: Vec<(String, FakeNode)> = nodes
        .iter()
        .filter(|(candidate, _)| *candidate != "/" && tree_parent(candidate) == path)
        .map(|(candidate, node)| {
            let name = candidate
                .rsplit('/')
                .next()
                .unwrap_or(candidate)
                .to_string();
            (name, node.clone())
        })
        .collect();
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    entries
        .into_iter()
        .map(|(name, node)| match node {
            FakeNode::Directory => {
                if machine {
                    format!("type=dir; {name}")
                } else {
                    format!("drwxr-xr-x 2 user group 0 Jan 1 2026 {name}")
                }
            }
            FakeNode::File(bytes) => {
                if machine {
                    format!("type=file;size={}; {name}", bytes.len())
                } else {
                    format!("-rw-r--r-- 1 user group {} Jan 1 2026 {name}", bytes.len())
                }
            }
            FakeNode::Symlink => {
                if machine {
                    format!("type=OS.unix=symlink; {name}")
                } else {
                    format!("lrwxrwxrwx 1 user group 0 Jan 1 2026 {name}")
                }
            }
        })
        .collect()
}

fn tree_parent(path: &str) -> String {
    match path.rsplit_once('/') {
        Some(("", _)) | None => "/".to_string(),
        Some((parent, _)) => parent.to_string(),
    }
}

fn send_tree_body(control: &mut TcpStream, data_listener: &mut Option<TcpListener>, bytes: &[u8]) {
    let _ = control.write_all(b"150 opening data connection\r\n");
    let Some(listener) = data_listener.take() else {
        let _ = control.write_all(b"425 data connection missing\r\n");
        return;
    };
    match listener.accept() {
        Ok((mut data, _)) => {
            // A cancelled transfer stops reading, so bound each write.
            let _ = data.set_write_timeout(Some(Duration::from_millis(250)));
            for chunk in bytes.chunks(8 * 1024) {
                if data.write_all(chunk).is_err() {
                    break;
                }
            }
            let _ = data.flush();
            drop(data);
            let _ = control.write_all(b"226 transfer complete\r\n");
        }
        Err(_) => {
            let _ = control.write_all(b"425 data connection failed\r\n");
        }
    }
}

fn receive_tree_body(control: &mut TcpStream, data_listener: &mut Option<TcpListener>) -> Vec<u8> {
    let _ = control.write_all(b"150 opening data connection\r\n");
    let Some(listener) = data_listener.take() else {
        let _ = control.write_all(b"425 data connection missing\r\n");
        return Vec::new();
    };
    match listener.accept() {
        Ok((mut data, _)) => {
            let mut buffer = Vec::new();
            let _ = data.read_to_end(&mut buffer);
            buffer
        }
        Err(_) => Vec::new(),
    }
}

#[test]
fn recursive_download_preserves_nested_structure() {
    let server = TreeServer::start(vec![
        ("/tree", FakeNode::Directory),
        ("/tree/a.txt", FakeNode::File(b"alpha".to_vec())),
        ("/tree/nested", FakeNode::Directory),
        ("/tree/nested/b.txt", FakeNode::File(b"beta!".to_vec())),
    ]);
    let service = Arc::new(connect_tree(&server));
    let root = test_directory("recursive-download");
    let destination = root.join("tree");

    let summary = FileCopyRequest {
        source: FileTransferEndpoint::Ftp {
            service,
            path: RemoteFilePath::new("/tree"),
        },
        destination: FileTransferEndpoint::Local(destination.clone()),
        control: SftpTransferControl::new(),
        options: overwrite_options(),
    }
    .execute()
    .expect("recursive download");

    assert_eq!(summary.bytes, 10);
    assert_eq!(
        std::fs::read(destination.join("a.txt")).expect("nested root file"),
        b"alpha"
    );
    assert_eq!(
        std::fs::read(destination.join("nested").join("b.txt")).expect("nested file"),
        b"beta!"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn recursive_upload_preserves_nested_structure() {
    let server = TreeServer::start(Vec::new());
    let service = Arc::new(connect_tree(&server));
    let root = test_directory("recursive-upload");
    std::fs::create_dir_all(root.join("tree").join("nested")).expect("local tree");
    std::fs::write(root.join("tree").join("a.txt"), b"alpha").expect("local file");
    std::fs::write(root.join("tree").join("nested").join("b.txt"), b"beta!")
        .expect("local nested file");

    let summary = FileCopyRequest {
        source: FileTransferEndpoint::Local(root.join("tree")),
        destination: FileTransferEndpoint::Ftp {
            service,
            path: RemoteFilePath::new("/tree"),
        },
        control: SftpTransferControl::new(),
        options: overwrite_options(),
    }
    .execute()
    .expect("recursive upload");

    assert_eq!(summary.bytes, 10);
    assert_eq!(server.read("/tree/a.txt").as_deref(), Some(&b"alpha"[..]));
    assert_eq!(
        server.read("/tree/nested/b.txt").as_deref(),
        Some(&b"beta!"[..])
    );
    assert!(
        server
            .commands()
            .iter()
            .any(|command| command.eq_ignore_ascii_case("MKD /tree/nested"))
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn recursive_remote_copy_preserves_nested_structure() {
    let server = TreeServer::start(vec![
        ("/tree", FakeNode::Directory),
        ("/tree/a.txt", FakeNode::File(b"alpha".to_vec())),
        ("/tree/nested", FakeNode::Directory),
        ("/tree/nested/b.txt", FakeNode::File(b"beta!".to_vec())),
    ]);
    let service = Arc::new(connect_tree(&server));

    let summary = FileCopyRequest {
        source: FileTransferEndpoint::Ftp {
            service: Arc::clone(&service),
            path: RemoteFilePath::new("/tree"),
        },
        destination: FileTransferEndpoint::Ftp {
            service,
            path: RemoteFilePath::new("/copy"),
        },
        control: SftpTransferControl::new(),
        options: overwrite_options(),
    }
    .execute()
    .expect("recursive remote copy");

    assert_eq!(summary.bytes, 10);
    assert_eq!(server.read("/copy/a.txt").as_deref(), Some(&b"alpha"[..]));
    assert_eq!(
        server.read("/copy/nested/b.txt").as_deref(),
        Some(&b"beta!"[..])
    );
}

struct PerChildResolver;

impl SftpDuplicateResolver for PerChildResolver {
    fn resolve_duplicate(
        &self,
        request: &SftpDuplicateRequest,
    ) -> Result<SftpDuplicateDecision, String> {
        if request.is_directory {
            return Ok(SftpDuplicateDecision::Overwrite);
        }
        let name = request
            .target_path
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(request.target_path.as_str());
        match name {
            "a.txt" => Ok(SftpDuplicateDecision::Skip),
            "b.txt" => Ok(SftpDuplicateDecision::Rename),
            _ => Ok(SftpDuplicateDecision::Overwrite),
        }
    }
}

#[test]
fn duplicate_policy_applies_to_each_recursive_child() {
    let server = TreeServer::start(vec![
        ("/tree", FakeNode::Directory),
        ("/tree/a.txt", FakeNode::File(b"new-a".to_vec())),
        ("/tree/b.txt", FakeNode::File(b"new-b".to_vec())),
    ]);
    let service = Arc::new(connect_tree(&server));
    let root = test_directory("recursive-policy");
    let destination = root.join("tree");
    std::fs::create_dir_all(&destination).expect("existing destination tree");
    std::fs::write(destination.join("a.txt"), b"old-a").expect("existing file");
    std::fs::write(destination.join("b.txt"), b"old-b").expect("existing file");

    let options = SftpPathTransferOptions::new(
        SftpDuplicatePolicy::Ask,
        Some(Arc::new(PerChildResolver)),
        SftpTransferOptions::default(),
    );
    FileCopyRequest {
        source: FileTransferEndpoint::Ftp {
            service,
            path: RemoteFilePath::new("/tree"),
        },
        destination: FileTransferEndpoint::Local(destination.clone()),
        control: SftpTransferControl::new(),
        options,
    }
    .execute()
    .expect("recursive download with per-child decisions");

    assert_eq!(
        std::fs::read(destination.join("a.txt")).expect("skipped file"),
        b"old-a"
    );
    assert_eq!(
        std::fs::read(destination.join("b.txt")).expect("renamed source"),
        b"old-b"
    );
    assert_eq!(
        std::fs::read(destination.join("b (1).txt")).expect("renamed copy"),
        b"new-b"
    );
    assert!(!destination.join("a (1).txt").exists());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn recursive_delete_removes_children_without_following_symlinks() {
    let server = TreeServer::start(vec![
        ("/dir", FakeNode::Directory),
        ("/dir/a.txt", FakeNode::File(b"a".to_vec())),
        ("/dir/nested", FakeNode::Directory),
        ("/dir/nested/b.txt", FakeNode::File(b"b".to_vec())),
        ("/dir/link", FakeNode::Symlink),
    ]);
    let service = Arc::new(connect_tree(&server));
    let browser = FileBrowserService::Ftp(Arc::clone(&service));

    browser
        .delete_remote_path(&RemoteFilePath::new("/dir"))
        .expect("recursive delete");

    assert!(!server.contains("/dir"));
    assert!(!server.contains("/dir/a.txt"));
    assert!(!server.contains("/dir/nested"));
    assert!(!server.contains("/dir/nested/b.txt"));
    let commands = server.commands();
    assert!(
        commands
            .iter()
            .any(|command| command.eq_ignore_ascii_case("DELE /dir/link")),
        "symlinks must be deleted, not traversed: {commands:?}"
    );
    assert!(
        !commands
            .iter()
            .any(|command| command.eq_ignore_ascii_case("MLSD /dir/link")),
        "symlinks must not be listed during deletion: {commands:?}"
    );
    assert!(
        commands
            .iter()
            .any(|command| command.eq_ignore_ascii_case("RMD /dir")),
        "the directory itself must be removed last: {commands:?}"
    );
}

#[test]
fn cancelled_download_removes_partial_file() {
    let server = TreeServer::start(vec![("/big.bin", FakeNode::File(vec![0x5a; 512 * 1024]))]);
    let service = Arc::new(connect_tree(&server));
    let root = test_directory("cancel-download");
    let destination = root.join("big.bin");
    let control = SftpTransferControl::new();
    let cancel = control.clone();
    let triggered = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&triggered);

    let error = FileCopyRequest {
        source: FileTransferEndpoint::Ftp {
            service,
            path: RemoteFilePath::new("/big.bin"),
        },
        destination: FileTransferEndpoint::Local(destination.clone()),
        control,
        options: overwrite_options(),
    }
    .execute_with_progress(move |_| {
        if !flag.swap(true, Ordering::SeqCst) {
            cancel.cancel();
        }
    })
    .expect_err("download must be cancelled");

    assert_eq!(error.to_string(), SFTP_TRANSFER_CANCELLED);
    assert!(
        !destination.exists(),
        "cancelled downloads must not publish a file"
    );
    assert!(
        !root.join(".big.bin.nyaterm-part").exists(),
        "cancelled downloads must clean their temporary file"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn cancelled_upload_removes_partial_remote_file() {
    let server = TreeServer::start(Vec::new());
    let service = Arc::new(connect_tree(&server));
    let root = test_directory("cancel-upload");
    let source = root.join("big.bin");
    std::fs::write(&source, vec![0x33; 512 * 1024]).expect("local upload fixture");
    let control = SftpTransferControl::new();
    let cancel = control.clone();
    let triggered = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&triggered);

    let error = FileCopyRequest {
        source: FileTransferEndpoint::Local(source),
        destination: FileTransferEndpoint::Ftp {
            service,
            path: RemoteFilePath::new("/big.bin"),
        },
        control,
        options: overwrite_options(),
    }
    .execute_with_progress(move |_| {
        if !flag.swap(true, Ordering::SeqCst) {
            cancel.cancel();
        }
    })
    .expect_err("upload must be cancelled");

    assert_eq!(error.to_string(), SFTP_TRANSFER_CANCELLED);
    assert!(
        !server.contains("/big.bin"),
        "cancelled uploads must not publish a destination"
    );
    assert!(
        server.temporary_paths().is_empty(),
        "cancelled uploads must delete their remote temporary file: {:?}",
        server.temporary_paths()
    );
    let _ = std::fs::remove_dir_all(root);
}
