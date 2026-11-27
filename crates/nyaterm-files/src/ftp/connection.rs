use std::net::{Shutdown, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use nyaterm_core::character_encoding::CharacterEncoding;
use suppaftp::native_tls::{Protocol, TlsConnector};
use suppaftp::types::{FileType, FormatControl, Mode};
use suppaftp::{NativeTlsConnector, NativeTlsFtpStream, Status, TransferStream};

type NativeTlsStream = <NativeTlsConnector as suppaftp::TlsConnector>::Stream;

use crate::ftp::config::{
    FtpConfig, FtpFilenameEncodingMode, FtpLogin, FtpPassiveMode, FtpSecurityMode, FtpTlsVersion,
};
use crate::ftp::encoding::FtpEncoding;
use crate::ftp::error::FtpError;
use crate::ftp::io::{Operation, SessionIo, connect_host, connect_socket};
use crate::ftp::pool::{Connection, Pool, PooledConnection};
use crate::ftp::service::FtpFileService;
use crate::sftp::SftpTransferControl;

pub(super) struct Lease<'a> {
    // Stop and join the watcher before returning the connection to the pool. Otherwise
    // its teardown could clear a subsequent operation's monitored data socket.
    pub(super) watch: Operation,
    connection: PooledConnection<'a>,
}

impl<'a> Lease<'a> {
    pub(super) fn new(
        connection: PooledConnection<'a>,
        control: &SftpTransferControl,
        deadline: Option<Instant>,
    ) -> Result<Self, FtpError> {
        let task = control.clone();
        let paused = control.clone();
        let watch = Operation::start(
            connection.io.clone(),
            move || task.is_cancelled() || deadline.is_some_and(|at| Instant::now() >= at),
            move || paused.is_paused(),
        )?;
        Ok(Self { watch, connection })
    }

    pub(super) fn invalidate(&self) {
        self.connection.io.close();
    }

    pub(super) fn command<T>(
        &mut self,
        command: impl FnOnce(&mut NativeTlsFtpStream) -> suppaftp::FtpResult<T>,
    ) -> Result<T, FtpError> {
        self.watch.check()?;
        let result = command(&mut self.connection.stream);
        self.watch.check()?;
        self.watch.progress();
        match result.map_err(FtpError::from) {
            Err(error @ (FtpError::Connection | FtpError::Timeout | FtpError::Tls)) => {
                self.connection.io.close();
                Err(error)
            }
            result => result,
        }
    }

    pub(super) fn complete<T>(
        &mut self,
        data: TransferStream<NativeTlsStream>,
        result: Result<T, FtpError>,
    ) -> Result<T, FtpError> {
        let result = match self.watch.check() {
            Ok(()) => result,
            Err(error) => Err(error),
        };
        match result {
            Ok(value) => {
                // The monitor owns a duplicate socket. Plain FTP needs an
                // explicit EOF; TLS finish sends close_notify before half-close.
                if !self.connection.secure {
                    let _ = data.get_ref().get_ref().shutdown(Shutdown::Write);
                }
                self.command(|_| data.finish())?;
                Ok(value)
            }
            Err(error) => {
                self.watch.cleanup();
                let _ = data.get_ref().get_ref().shutdown(Shutdown::Both);
                // A completion reply can race ABOR. A NOOP fence must be the next
                // reply before any cleanup command may mutate server paths.
                if self.connection.stream.abort_with_fence(data).is_err() {
                    self.connection.io.close();
                }
                Err(error)
            }
        }
    }
}

impl FtpFileService {
    pub(super) fn connect_inner(
        config: FtpConfig,
        login: FtpLogin,
        local_encoding: CharacterEncoding,
        cancelled: impl Fn() -> bool + Send + 'static,
    ) -> Result<Self, FtpError> {
        let tls = if config.security_mode == FtpSecurityMode::None {
            None
        } else {
            Some(tls_connector(&config)?)
        };
        Self::connect_with_tls(config, login, local_encoding, cancelled, tls)
    }

    pub(super) fn connect_with_tls(
        config: FtpConfig,
        login: FtpLogin,
        local_encoding: CharacterEncoding,
        cancelled: impl Fn() -> bool + Send + 'static,
        mut tls: Option<NativeTlsConnector>,
    ) -> Result<Self, FtpError> {
        if config.connect_timeout.is_zero() {
            return Err(FtpError::UnsupportedSettings);
        }
        if config.filename_encoding_mode == FtpFilenameEncodingMode::Custom {
            config.resolve_filename_encoding(local_encoding, false)?;
        }
        let socket = connect_host(
            config.host.clone(),
            config.port,
            config.connect_timeout,
            &cancelled,
        )?;
        let peer = socket.peer_addr().map_err(|_| FtpError::Connection)?;
        let io = SessionIo::new(&socket)?;
        let watch = Operation::start_with_timeout(
            Arc::clone(&io),
            cancelled,
            || false,
            config.connect_timeout,
        )?;
        let connected = (|| {
            let mut stream = if config.security_mode == FtpSecurityMode::Implicit {
                NativeTlsFtpStream::connect_secure_implicit_with_stream(
                    socket,
                    tls.take().ok_or(FtpError::UnsupportedSettings)?,
                    &config.host,
                )
            } else {
                NativeTlsFtpStream::connect_with_stream(socket)
            }
            .map_err(FtpError::from)?;
            watch.check()?;
            stream.set_mode(match config.passive_mode {
                FtpPassiveMode::Epsv => Mode::ExtendedPassive,
                FtpPassiveMode::Auto if peer.is_ipv6() => Mode::ExtendedPassive,
                _ => Mode::Passive,
            });
            let data_io = Arc::clone(&io);
            let timeout = config.connect_timeout;
            stream = stream.passive_stream_builder(move |advertised: SocketAddr| {
                let target = SocketAddr::new(peer.ip(), advertised.port());
                let socket = connect_socket(vec![target], timeout, || data_io.interrupted())
                    .map_err(|_| {
                        suppaftp::FtpError::ConnectionError(std::io::Error::new(
                            std::io::ErrorKind::ConnectionAborted,
                            "data connection failed",
                        ))
                    })?;
                data_io
                    .set_data(&socket)
                    .map_err(|_| suppaftp::FtpError::BadResponse)?;
                Ok(socket)
            });
            if config.security_mode == FtpSecurityMode::Explicit {
                stream = stream
                    .into_secure(
                        tls.take().ok_or(FtpError::UnsupportedSettings)?,
                        &config.host,
                    )
                    .map_err(FtpError::from)?;
            }
            let password = match &login {
                FtpLogin::Anonymous { .. } => "",
                FtpLogin::Password { password, .. } => password.expose_secret(),
            };
            stream.login(login.username(), password).map_err(|error| {
            if matches!(error, suppaftp::FtpError::UnexpectedResponse(ref response) if response.status as u32 == 530) {
                if matches!(login, FtpLogin::Anonymous { .. }) { FtpError::AnonymousLoginRejected } else { FtpError::AuthenticationFailed }
            } else { FtpError::from(error) }
        })?;
            if config.security_mode == FtpSecurityMode::Implicit {
                stream
                    .custom_command("PBSZ 0", &[Status::CommandOk])
                    .map_err(FtpError::from)?;
                stream
                    .custom_command("PROT P", &[Status::CommandOk])
                    .map_err(FtpError::from)?;
            }
            let utf8 = config.filename_encoding_mode == FtpFilenameEncodingMode::Auto
                && stream
                    .feat()
                    .map(|features| features.keys().any(|key| key.eq_ignore_ascii_case("UTF8")))
                    .unwrap_or(false);
            let encoding = config.resolve_filename_encoding(local_encoding, utf8)?;
            stream.set_encoding(FtpEncoding(encoding));
            if encoding == CharacterEncoding::Utf8 {
                let _ = stream.opts("UTF8", Some("ON"));
            }
            stream
                .transfer_type(if config.ascii_mode {
                    FileType::Ascii(FormatControl::Default)
                } else {
                    FileType::Binary
                })
                .map_err(FtpError::from)?;
            let mut home = stream.pwd().map_err(FtpError::from)?;
            if !config.initial_directory.is_empty() {
                stream
                    .cwd(&config.initial_directory)
                    .map_err(FtpError::from)?;
                home = stream.pwd().map_err(FtpError::from)?;
            }
            Ok::<_, FtpError>((stream, encoding, home))
        })();
        watch.check()?;
        let (stream, encoding, home) = connected?;
        watch.check()?;
        drop(watch);
        Ok(Self {
            pool: Arc::new(Pool::new(Connection {
                stream,
                io,
                secure: config.security_mode != FtpSecurityMode::None,
                last_used: Instant::now(),
            })),
            config,
            encoding,
            home,
            peer,
            username: login.username().to_owned(),
            mutations: Mutex::new(()),
        })
    }

    pub(super) fn lock_stream(&self, control: &SftpTransferControl) -> Result<Lease<'_>, FtpError> {
        self.lock_stream_until(control, None)
    }

    pub(super) fn lock_stream_until(
        &self,
        control: &SftpTransferControl,
        deadline: Option<std::time::Instant>,
    ) -> Result<Lease<'_>, FtpError> {
        Lease::new(
            self.pool.acquire(false, control, deadline)?,
            control,
            deadline,
        )
    }

    pub(super) fn lock_transfer(
        &self,
        control: &SftpTransferControl,
    ) -> Result<Lease<'_>, FtpError> {
        Lease::new(self.pool.acquire(true, control, None)?, control, None)
    }
}

fn tls_connector(config: &FtpConfig) -> Result<NativeTlsConnector, FtpError> {
    let mut builder = TlsConnector::builder();
    let protocol = match config.tls_version {
        FtpTlsVersion::Auto => None,
        FtpTlsVersion::Ssl3 => Some(Protocol::Sslv3),
        FtpTlsVersion::Tls10 => Some(Protocol::Tlsv10),
        FtpTlsVersion::Tls11 => Some(Protocol::Tlsv11),
        FtpTlsVersion::Tls12 => Some(Protocol::Tlsv12),
        FtpTlsVersion::Tls13 => Some(Protocol::Tlsv13),
    };
    if let Some(protocol) = protocol {
        builder.min_protocol_version(Some(protocol));
        builder.max_protocol_version(Some(protocol));
    }
    builder
        .build()
        .map(NativeTlsConnector::from)
        .map_err(|_| FtpError::UnsupportedSettings)
}
