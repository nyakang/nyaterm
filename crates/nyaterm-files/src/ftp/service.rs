use std::collections::HashSet;
use std::fmt;
use std::io::{Cursor, Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use nyaterm_core::character_encoding::CharacterEncoding;
use suppaftp::native_tls::{Protocol, TlsConnector};
use suppaftp::types::{FileType as SuppaFileType, FormatControl, Mode};
use suppaftp::{FtpError as SuppaError, NativeTlsConnector, NativeTlsFtpStream};

use crate::ftp::config::{
    FtpConfig, FtpFilenameEncodingMode, FtpLogin, FtpPassiveMode, FtpSecurityMode, FtpTlsVersion,
};
use crate::ftp::encoding::{FtpEncoding, system_filename_encoding};
use crate::ftp::error::FtpError;
use crate::ftp::list::{
    FtpEntry, FtpEntryKind, entries_from_lines, entry_from_mlst_line, parent_and_name,
};
use crate::ftp::text::{LocalDecoder, NvtEncoder};

/// Chunk size for streamed transfers. Cancellation is checked after each chunk,
/// so a single large file can be interrupted without buffering it in memory.
const FTP_TRANSFER_CHUNK_BYTES: usize = 32 * 1024;

/// Blocking FTP control connection used by the desktop file-browser workers.
pub struct FtpFileService {
    config: FtpConfig,
    encoding: CharacterEncoding,
    home: String,
    stream: Mutex<NativeTlsFtpStream>,
}

impl fmt::Debug for FtpFileService {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FtpFileService")
            .field("host", &self.config.host)
            .field("port", &self.config.port)
            .field("encoding", &self.encoding)
            .finish_non_exhaustive()
    }
}

impl FtpFileService {
    pub fn connect(config: FtpConfig, login: FtpLogin) -> Result<Self, FtpError> {
        Self::connect_with_local_encoding(config, login, system_filename_encoding())
    }

    pub fn connect_with_local_encoding(
        config: FtpConfig,
        login: FtpLogin,
        local_encoding: CharacterEncoding,
    ) -> Result<Self, FtpError> {
        let address = resolve_socket_address(&config.host, config.port)?;
        let mut stream = if config.security_mode == FtpSecurityMode::Implicit {
            // The implicit API performs the TLS handshake on connect. It does
            // not expose a configurable TCP connect timeout.
            let connector = build_tls_connector(&config)?;
            NativeTlsFtpStream::connect_secure_implicit(address, connector, &config.host).map_err(
                |error| FtpError::Protocol(format!("implicit FTPS connection failed: {error}")),
            )?
        } else {
            NativeTlsFtpStream::connect_timeout(address, config.connect_timeout)
                .map_err(|_| FtpError::Connection)?
        };
        let control_ip = address.ip();
        stream.set_mode(match config.passive_mode {
            FtpPassiveMode::Pasv => Mode::Passive,
            FtpPassiveMode::Epsv => Mode::ExtendedPassive,
            FtpPassiveMode::Auto => {
                if control_ip.is_ipv6() {
                    Mode::ExtendedPassive
                } else {
                    Mode::Passive
                }
            }
        });
        let stream = stream.passive_stream_builder(move |advertised: SocketAddr| {
            let target = SocketAddr::new(control_ip, advertised.port());
            TcpStream::connect_timeout(&target, Duration::from_secs(15))
                .map_err(SuppaError::ConnectionError)
        });
        let mut stream = upgrade_to_explicit_tls(stream, &config)?;

        let password = match &login {
            FtpLogin::Anonymous { .. } => "",
            FtpLogin::Password { password, .. } => password.expose_secret(),
        };
        stream
            .login(login.username(), password)
            .map_err(|error| login_error(&login, error))?;

        let server_supports_utf8 = if config.filename_encoding_mode == FtpFilenameEncodingMode::Auto
        {
            stream
                .feat()
                .map(|features| {
                    features
                        .keys()
                        .any(|name| name.eq_ignore_ascii_case("UTF8"))
                })
                .unwrap_or(false)
        } else {
            false
        };
        let encoding = config.resolve_filename_encoding(local_encoding, server_supports_utf8)?;
        stream.set_encoding(FtpEncoding(encoding));
        if encoding == CharacterEncoding::Utf8 {
            // FEAT decides auto detection; OPTS only asks the server to enable UTF-8.
            let _ = stream.opts("UTF8", Some("ON"));
        }
        stream
            .transfer_type(if config.ascii_mode {
                SuppaFileType::Ascii(FormatControl::Default)
            } else {
                SuppaFileType::Binary
            })
            .map_err(FtpError::from)?;

        let mut home = stream.pwd().unwrap_or_else(|_| "/".to_string());
        if !config.initial_directory.trim().is_empty() {
            let initial = config.initial_directory.trim().to_string();
            if stream.cwd(&initial).is_ok() {
                home = stream.pwd().unwrap_or(initial);
            }
        }

        Ok(Self {
            config,
            encoding,
            home,
            stream: Mutex::new(stream),
        })
    }

    pub fn encoding(&self) -> CharacterEncoding {
        self.encoding
    }

    pub fn home_dir(&self) -> &str {
        &self.home
    }

    pub fn list_dir(&self, path: &str) -> Result<Vec<FtpEntry>, FtpError> {
        let path = if path.trim().is_empty() {
            self.home.clone()
        } else {
            path.to_string()
        };
        let (lines, machine_listing) = {
            let mut stream = self.lock_stream()?;
            match stream.mlsd(Some(&path)) {
                Ok(lines) => (lines, true),
                Err(_) => (stream.list(Some(&path)).map_err(FtpError::from)?, false),
            }
        };
        let entries = entries_from_lines(&lines, &path, machine_listing);
        if machine_listing && !lines.is_empty() && entries.is_empty() {
            let lines = {
                let mut stream = self.lock_stream()?;
                stream.list(Some(&path)).map_err(FtpError::from)?
            };
            return Ok(entries_from_lines(&lines, &path, false));
        }
        Ok(entries)
    }

    pub fn stat(&self, path: &str) -> Result<FtpEntry, FtpError> {
        if path.trim().is_empty() || path == "/" {
            return Ok(FtpEntry {
                name: "/".to_string(),
                path: "/".to_string(),
                kind: FtpEntryKind::Directory,
                size: 0,
                modified: None,
            });
        }
        {
            let mut stream = self.lock_stream()?;
            if let Ok(line) = stream.mlst(Some(path))
                && let Some(entry) = entry_from_mlst_line(&line, path)
            {
                return Ok(entry);
            }
        }
        let (parent, name) = parent_and_name(path);
        self.list_dir(&parent)?
            .into_iter()
            .find(|entry| entry.name == name)
            .ok_or(FtpError::InvalidPath)
    }

    pub fn create_dir(&self, path: &str) -> Result<(), FtpError> {
        self.lock_stream()?.mkdir(path).map_err(FtpError::from)
    }

    /// Creates missing parent directories; existing directories are accepted.
    pub fn create_dir_all(&self, path: &str) -> Result<(), FtpError> {
        let absolute = path.starts_with('/');
        let mut current = if absolute {
            String::from("/")
        } else {
            String::new()
        };
        for component in path.split('/').filter(|component| !component.is_empty()) {
            if !current.is_empty() && !current.ends_with('/') {
                current.push('/');
            }
            current.push_str(component);
            if let Ok(entry) = self.stat(&current) {
                if !entry.is_directory() {
                    return Err(FtpError::InvalidPath);
                }
                continue;
            }
            self.create_dir(&current)?;
        }
        Ok(())
    }

    pub fn create_file(&self, path: &str) -> Result<(), FtpError> {
        let mut empty = Cursor::new(Vec::new());
        self.lock_stream()?
            .put_file(path, &mut empty)
            .map(|_| ())
            .map_err(FtpError::from)
    }

    pub fn remove(&self, path: &str) -> Result<(), FtpError> {
        let is_directory = self
            .stat(path)
            .map(|entry| entry.is_directory())
            .unwrap_or(false);
        let mut stream = self.lock_stream()?;
        if is_directory {
            stream.rmdir(path).map_err(FtpError::from)
        } else {
            stream.rm(path).map_err(FtpError::from)
        }
    }

    /// Removes a file, directory or directory tree. Directory symlinks are
    /// deleted as entries and never traversed; a visited set also stops servers
    /// that report a cycle from looping forever.
    pub fn remove_tree(&self, path: &str) -> Result<(), FtpError> {
        if path.trim().is_empty() || path == "/" {
            return Err(FtpError::InvalidPath);
        }
        let kind = self.stat(path)?.kind;
        let mut visited = HashSet::new();
        self.remove_tree_entry(path, kind, &mut visited)
    }

    fn remove_tree_entry(
        &self,
        path: &str,
        kind: FtpEntryKind,
        visited: &mut HashSet<String>,
    ) -> Result<(), FtpError> {
        if kind != FtpEntryKind::Directory {
            // Files and symlinks (including links to directories) use DELE and
            // are never followed.
            return self.lock_stream()?.rm(path).map_err(FtpError::from);
        }
        if !visited.insert(path.to_string()) {
            return Ok(());
        }
        for child in self.list_dir(path)? {
            if matches!(child.name.as_str(), "." | "..") {
                continue;
            }
            self.remove_tree_entry(&child.path, child.kind, visited)?;
        }
        self.lock_stream()?.rmdir(path).map_err(FtpError::from)
    }

    pub fn rename(&self, from: &str, to: &str) -> Result<(), FtpError> {
        self.lock_stream()?.rename(from, to).map_err(FtpError::from)
    }

    pub fn read_file_bytes(&self, path: &str) -> Result<Vec<u8>, FtpError> {
        let bytes = self
            .lock_stream()?
            .retr_as_buffer(path)
            .map(|cursor| cursor.into_inner())
            .map_err(FtpError::from)?;
        if !self.config.ascii_mode {
            return Ok(bytes);
        }
        let mut decoder = LocalDecoder::default();
        let mut decoded = Vec::with_capacity(bytes.len());
        decoder.decode(&bytes, &mut decoded);
        decoder.finish(&mut decoded);
        Ok(decoded)
    }

    pub fn write_file(&self, path: &str, bytes: &[u8]) -> Result<(), FtpError> {
        let mut stream = self.lock_stream()?;
        let mut data = stream.put_with_stream(path)?;
        // `put_with_stream` never converts, so ASCII encoding is explicit here
        // and cannot double-convert the payload.
        let written = if self.config.ascii_mode {
            let mut encoder = NvtEncoder::default();
            let mut encoded = Vec::with_capacity(bytes.len() + bytes.len() / 16);
            encoder.encode(bytes, &mut encoded);
            data.write_all(&encoded)
        } else {
            data.write_all(bytes)
        };
        let written = written.map_err(|error| FtpError::Protocol(error.to_string()));
        let finished = data.finish().map_err(FtpError::from);
        written?;
        finished
    }

    /// Streams a remote file into `writer`, invoking `on_chunk` after every
    /// written chunk. Returning an error from `on_chunk` aborts the transfer
    /// after the server connection has been finalized.
    pub fn retr_to_writer<W: Write + ?Sized>(
        &self,
        path: &str,
        writer: &mut W,
        mut on_chunk: impl FnMut(usize) -> Result<(), FtpError>,
    ) -> Result<u64, FtpError> {
        let mut stream = self.lock_stream()?;
        let mut data = stream.retr_as_stream(path)?;
        let mut buffer = vec![0u8; FTP_TRANSFER_CHUNK_BYTES];
        let mut converted = Vec::with_capacity(FTP_TRANSFER_CHUNK_BYTES * 2);
        let mut decoder = self.config.ascii_mode.then(LocalDecoder::default);
        let mut total = 0u64;
        let outcome = loop {
            let read = match data.read(&mut buffer) {
                Ok(read) => read,
                Err(error) => break Err(FtpError::Protocol(error.to_string())),
            };
            if read == 0 {
                if let Some(decoder) = decoder.as_mut() {
                    converted.clear();
                    decoder.finish(&mut converted);
                    if let Err(error) = writer.write_all(&converted) {
                        break Err(FtpError::Protocol(error.to_string()));
                    }
                }
                break Ok(());
            }
            let written = match decoder.as_mut() {
                Some(decoder) => {
                    converted.clear();
                    decoder.decode(&buffer[..read], &mut converted);
                    writer.write_all(&converted)
                }
                None => writer.write_all(&buffer[..read]),
            };
            if let Err(error) = written {
                break Err(FtpError::Protocol(error.to_string()));
            }
            total = total.saturating_add(read as u64);
            if let Err(error) = on_chunk(read) {
                break Err(error);
            }
        };
        // Finalize even after a cancellation so the control connection stays usable.
        let finished = data.finish().map_err(FtpError::from);
        outcome?;
        finished?;
        Ok(total)
    }

    /// Streams `reader` into a remote file, invoking `on_chunk` after every
    /// written chunk. Returning an error from `on_chunk` aborts the transfer
    /// after the server connection has been finalized.
    pub fn stor_from_reader<R: Read + ?Sized>(
        &self,
        path: &str,
        reader: &mut R,
        mut on_chunk: impl FnMut(usize) -> Result<(), FtpError>,
    ) -> Result<u64, FtpError> {
        let mut stream = self.lock_stream()?;
        let mut data = stream.put_with_stream(path)?;
        let mut buffer = vec![0u8; FTP_TRANSFER_CHUNK_BYTES];
        let mut converted = Vec::with_capacity(FTP_TRANSFER_CHUNK_BYTES * 2);
        let mut encoder = self.config.ascii_mode.then(NvtEncoder::default);
        let mut total = 0u64;
        let outcome = loop {
            let read = match reader.read(&mut buffer) {
                Ok(read) => read,
                Err(error) => break Err(FtpError::Protocol(error.to_string())),
            };
            if read == 0 {
                break Ok(());
            }
            let written = match encoder.as_mut() {
                Some(encoder) => {
                    converted.clear();
                    encoder.encode(&buffer[..read], &mut converted);
                    data.write_all(&converted)
                }
                None => data.write_all(&buffer[..read]),
            };
            if let Err(error) = written {
                break Err(FtpError::Protocol(error.to_string()));
            }
            total = total.saturating_add(read as u64);
            if let Err(error) = on_chunk(read) {
                break Err(error);
            }
        };
        // Finalize even after a cancellation so the control connection stays usable.
        let finished = data.finish().map_err(FtpError::from);
        outcome?;
        finished?;
        Ok(total)
    }

    fn lock_stream(&self) -> Result<MutexGuard<'_, NativeTlsFtpStream>, FtpError> {
        self.stream.lock().map_err(|_| FtpError::Unavailable)
    }
}

fn build_tls_connector(config: &FtpConfig) -> Result<NativeTlsConnector, FtpError> {
    let mut builder = TlsConnector::builder();
    if let Some(protocol) = tls_protocol(config.tls_version) {
        builder.min_protocol_version(Some(protocol));
        builder.max_protocol_version(Some(protocol));
    }
    builder
        .build()
        .map(NativeTlsConnector::from)
        .map_err(|error| FtpError::Protocol(format!("FTPS setup failed: {error}")))
}

/// Upgrades the control connection to explicit FTPS when requested.
fn upgrade_to_explicit_tls(
    stream: NativeTlsFtpStream,
    config: &FtpConfig,
) -> Result<NativeTlsFtpStream, FtpError> {
    if config.security_mode != FtpSecurityMode::Explicit {
        return Ok(stream);
    }
    stream
        .into_secure(build_tls_connector(config)?, &config.host)
        .map_err(|error| FtpError::Protocol(format!("FTPS handshake failed: {error}")))
}

fn tls_protocol(version: FtpTlsVersion) -> Option<Protocol> {
    match version {
        FtpTlsVersion::Auto => None,
        FtpTlsVersion::Ssl3 => Some(Protocol::Sslv3),
        FtpTlsVersion::Tls10 => Some(Protocol::Tlsv10),
        FtpTlsVersion::Tls11 => Some(Protocol::Tlsv11),
        FtpTlsVersion::Tls12 => Some(Protocol::Tlsv12),
        FtpTlsVersion::Tls13 => Some(Protocol::Tlsv13),
    }
}

fn resolve_socket_address(host: &str, port: u16) -> Result<SocketAddr, FtpError> {
    (host, port)
        .to_socket_addrs()
        .map_err(|_| FtpError::Connection)?
        .next()
        .ok_or(FtpError::Connection)
}

fn login_error(login: &FtpLogin, error: SuppaError) -> FtpError {
    if error.to_string().contains("530") {
        if matches!(login, FtpLogin::Anonymous { .. }) {
            FtpError::AnonymousLoginRejected
        } else {
            FtpError::AuthenticationFailed
        }
    } else {
        FtpError::Protocol(error.to_string())
    }
}
