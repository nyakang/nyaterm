//! Session ownership and lifecycle.
#[cfg(windows)]
use crate::local_conpty;
use crate::local_shell::LocalShellPathStyle;
use crate::session_event_queue::SessionEventQueue;
use crate::ssh_services::SshSessionServices;
use crate::ssh_worker::{SshCommand, TmuxCommandSender, run_ssh_worker};
use crate::telnet_input::TelnetInputState;
use crate::transport_io::telnet_naws::TelnetNawsWriter;
use crate::transport_io::{
    QueuedTransportWriter, TelnetAutoLoginState, TelnetWriterLifecycle, TelnetWriterPolicy,
    WriteCapacity, WriterFlushPolicy, open_serial_port, spawn_serial_reader_thread,
    spawn_tcp_reader_thread,
};
use crate::transport_io::{
    cancel_tmux_pane_output, spawn_reader_thread, telnet_wire_size, tmux_modem_unsupported,
};
use crate::{
    EnvironmentSnapshot, LocalPtyTransport, LocalSessionConfig, SerialSessionConfig,
    SerialTransport, SessionDrain, SessionError, SessionEvent, SessionEventConsumerId, SessionInfo,
    SessionKind, ShellEnvironmentCache, SshChannelTransport, SshMultiplexHandle, SshSessionConfig,
    TelnetSessionConfig, TelnetTransport, TerminalTransport, TmuxPaneTransport, connection_attempt,
    local_shell, local_shell_integration, open_ssh_multiplex_handle, ssh_mailbox, tmux,
    validate_ssh_algorithm_preferences, writer_mailbox, zmodem,
};
use nyaterm_core::runtime::reaper::Reaper;
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::collections::HashMap;
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

pub struct SessionManager {
    /// Arc-shared so the SSH worker can register tmux pane sessions while
    /// control mode is active, without owning the whole manager.
    pub(crate) sessions: Arc<Mutex<HashMap<String, ManagedSession>>>,
    pub(crate) event_queue: SessionEventQueue,
    pub(crate) next_event_consumer_id: AtomicU64,
    pub(crate) shell_environment: Arc<ShellEnvironmentCache>,
    pub(crate) ssh_services: SshSessionServices,
    pub(crate) reaper: Mutex<Option<Reaper<ManagedSession>>>,
}

pub(crate) enum ManagedSession {
    Local(LocalPtyTransport),
    Ssh(SshChannelTransport),
    Tcp(Box<TelnetTransport>),
    Serial(SerialTransport),
    /// Virtual session projecting one tmux pane of a control-mode SSH channel.
    TmuxPane(TmuxPaneTransport),
}

pub type SharedSessionManager = Arc<SessionManager>;

pub(crate) fn local_pty_size(cols: u16, rows: u16, pixel_width: u16, pixel_height: u16) -> PtySize {
    PtySize {
        rows,
        cols,
        pixel_width,
        pixel_height,
    }
}

pub(crate) fn configure_environment(
    command: &mut CommandBuilder,
    shell_snapshot: Option<&EnvironmentSnapshot>,
) {
    if let Some(shell_snapshot) = shell_snapshot {
        // Clear portable-pty's inherited process environment only after a shell
        // query succeeds and can be represented completely. This prevents
        // variables removed by the shell (especially stale SSH agent paths)
        // from leaking through, while preserving non-UTF-8 values that may be
        // omitted by the inherited fallback snapshot.
        if shell_snapshot.replaces_inherited_environment() {
            command.env_clear();
        }
        for (variable, value) in shell_snapshot.iter() {
            command.env(variable, value.as_str());
        }
    }
    command.env("TERM", "xterm-256color");
    if cfg!(target_os = "macos") {
        command.env("LANG", utf8_env_or("LANG", "en_US.UTF-8"));
        command.env("LC_CTYPE", utf8_env_or("LC_CTYPE", "UTF-8"));
    }
}

pub(crate) fn utf8_env_or(name: &str, fallback: &str) -> String {
    std::env::var(name)
        .ok()
        .filter(|value| {
            let normalized = value.to_ascii_lowercase().replace('_', "-");
            normalized.contains("utf-8") || normalized.contains("utf8")
        })
        .unwrap_or_else(|| fallback.to_string())
}

#[cfg(windows)]
pub(crate) fn default_local_shell_path() -> String {
    std::env::var("COMSPEC")
        .ok()
        .filter(|path| !path.trim().is_empty())
        .unwrap_or_else(|| "cmd.exe".to_string())
}

#[cfg(not(windows))]
pub(crate) fn default_local_shell_path() -> String {
    std::env::var("SHELL")
        .ok()
        .filter(|path| !path.trim().is_empty())
        .unwrap_or_else(|| "/bin/sh".to_string())
}

#[cfg(not(windows))]
pub(crate) fn should_use_interactive_login_args(program: &str) -> bool {
    let name = program
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(program)
        .to_ascii_lowercase();
    matches!(name.as_str(), "bash" | "zsh" | "fish")
}

impl SessionManager {
    pub fn new() -> Self {
        Self::with_ssh_services(SshSessionServices::default())
    }

    pub fn with_ssh_services(ssh_services: SshSessionServices) -> Self {
        Self {
            ssh_services,
            sessions: Arc::new(Mutex::new(HashMap::new())),
            event_queue: SessionEventQueue::new(),
            next_event_consumer_id: AtomicU64::new(1),
            shell_environment: ShellEnvironmentCache::global(),
            reaper: Mutex::new(None),
        }
    }

    pub(crate) fn ensure_reaper(&self) -> Result<(), SessionError> {
        let mut reaper = self.reaper.lock().map_err(|_| SessionError::LockPoisoned)?;
        if reaper.is_none() {
            *reaper = Some(
                Reaper::new("transport-reaper", 64, |mut session: ManagedSession| {
                    session.close()
                })
                .map_err(SessionError::Worker)?,
            );
        }
        Ok(())
    }

    /// Release local resources on the reaper. Virtual tmux sessions have no
    /// blocking destructor and never imply a remote kill.
    pub fn close_detached(&self, session_id: &str) -> Result<(), SessionError> {
        {
            let mut sessions = self
                .sessions
                .lock()
                .map_err(|_| SessionError::LockPoisoned)?;
            if matches!(sessions.get(session_id), Some(ManagedSession::TmuxPane(_))) {
                sessions.remove(session_id);
                self.event_queue.cancel_session(session_id);
                return Ok(());
            }
        }
        self.ensure_reaper()?;
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| SessionError::LockPoisoned)?;
        let reaper = self.reaper.lock().map_err(|_| SessionError::LockPoisoned)?;
        reaper
            .as_ref()
            .ok_or_else(|| SessionError::Worker(std::io::Error::other("reaper unavailable")))?
            .try_submit_with(|| {
                let mut session = sessions.remove(session_id)?;
                if let ManagedSession::Ssh(ssh) = &session {
                    self.event_queue.cancel_session(session_id);
                    cancel_tmux_pane_output(&self.event_queue, &sessions, &ssh.tmux_tx);
                }
                // Clear pending input before waiting behind other cleanup jobs.
                // Native cancellation and joins remain on the reaper worker.
                match &session {
                    ManagedSession::Local(local) => local.writer.command_tx.stop(),
                    ManagedSession::Tcp(tcp) => tcp.writer.command_tx.stop(),
                    ManagedSession::Serial(serial) => serial.writer.command_tx.stop(),
                    ManagedSession::Ssh(ssh) => ssh.command_tx.close(),
                    ManagedSession::TmuxPane(_) => {}
                }
                if let ManagedSession::Local(local) = &mut session {
                    local.cleanup_consumer = self.event_queue.consumer_for(session_id);
                }
                self.event_queue.cancel_session(session_id);
                Some(session)
            })
            .map_err(SessionError::Worker)
    }

    /// Allocate an independent event consumer. Sessions are invisible to the
    /// consumer until explicitly assigned, which lets multiple desktop
    /// workspaces share one transport manager without stealing each other's
    /// output.
    /// Place an in-process presentation fence after all output already queued.
    pub fn enqueue_output_boundary(&self, session_id: &str, token: &str) {
        self.event_queue.push(SessionEvent::OutputBoundary {
            session_id: session_id.into(),
            token: token.into(),
        });
    }

    pub fn register_event_consumer(&self) -> SessionEventConsumerId {
        SessionEventConsumerId(self.next_event_consumer_id.fetch_add(1, Ordering::Relaxed))
    }

    pub fn assign_session_event_consumer(
        &self,
        session_id: &str,
        consumer_id: SessionEventConsumerId,
    ) {
        self.event_queue.assign_consumer(session_id, consumer_id);
    }

    pub fn clear_session_event_consumer(
        &self,
        session_id: &str,
        consumer_id: SessionEventConsumerId,
    ) {
        self.event_queue.clear_consumer(session_id, consumer_id);
    }

    /// Return the runtime-only shell environment cache shared by SSH tasks.
    pub fn shell_environment(&self) -> Arc<ShellEnvironmentCache> {
        Arc::clone(&self.shell_environment)
    }

    /// Namespace of the resolved launcher, including Windows Terminal aliases.
    pub fn local_shell_path_style(&self, session_id: &str) -> Option<LocalShellPathStyle> {
        let sessions = self.sessions.lock().ok()?;
        match sessions.get(session_id)? {
            ManagedSession::Local(session) => Some(session.cwd_path_style),
            _ => None,
        }
    }

    /// The WSL distribution a local WSL session targets, used to map WSL-only
    /// working directories onto the `\\wsl.localhost` namespace.
    pub fn local_wsl_distro(&self, session_id: &str) -> Option<String> {
        let sessions = self.sessions.lock().ok()?;
        match sessions.get(session_id)? {
            ManagedSession::Local(session) => session.wsl_distro.clone(),
            _ => None,
        }
    }

    pub fn create_local_session(
        &self,
        config: LocalSessionConfig,
    ) -> Result<SessionInfo, SessionError> {
        let shell = local_shell::ResolvedLocalShell::resolve(&config);
        self.create_local_session_with_shell(config, shell)
    }

    pub(crate) fn create_local_session_with_shell(
        &self,
        config: LocalSessionConfig,
        shell: local_shell::ResolvedLocalShell,
    ) -> Result<SessionInfo, SessionError> {
        shell
            .validate_working_directory(config.working_dir.as_deref())
            .map_err(|error| SessionError::Spawn(error.into()))?;
        self.ensure_reaper()?;
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| SessionError::LockPoisoned)?;
        #[cfg(windows)]
        local_conpty::ensure_supported()?;
        let session_id = uuid::Uuid::new_v4().to_string();
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(local_pty_size(
                config.cols,
                config.rows,
                config.pixel_width,
                config.pixel_height,
            ))
            .map_err(SessionError::OpenPty)?;

        let prepared = local_shell_integration::LocalShellIntegration::prepare(&shell, &session_id);
        let (launch, mut integration) = match prepared {
            Some((launch, integration)) => (launch, Some(integration)),
            None => (
                local_shell::ResolvedLocalShell {
                    program: shell.program.clone(),
                    args: shell.args.clone(),
                },
                None,
            ),
        };
        let cwd_path_style = integration
            .as_ref()
            .map(|integration| integration.path_style)
            .unwrap_or_else(|| LocalShellPathStyle::from_program(&shell.program));
        let wsl_distro = local_shell_integration::wsl_distro(&shell);
        let dialect = if shell.program.to_ascii_lowercase().contains("wsl") && integration.is_some()
        {
            shell
                .args
                .iter()
                .map(|arg| nyaterm_core::completion::types::ShellDialect::from_program(arg))
                .find(|kind| *kind != nyaterm_core::completion::types::ShellDialect::Unknown)
                .unwrap_or(nyaterm_core::completion::types::ShellDialect::Unknown)
        } else {
            nyaterm_core::completion::types::ShellDialect::from_program(&shell.program)
        };
        let user = std::env::var("USERNAME")
            .or_else(|_| std::env::var("USER"))
            .unwrap_or_default();
        let scope = nyaterm_core::completion::types::CompletionScope::new(&[
            "local",
            &user,
            &shell.program,
            wsl_distro.as_deref().unwrap_or(""),
            &format!("{dialect:?}"),
        ]);
        let command_context = nyaterm_core::completion::types::ShellCommandContext {
            scope,
            dialect,
            command_events: integration.is_some(),
        };
        if let Some(integration) = &mut integration {
            integration.completion_distro = wsl_distro.clone();
        }
        let startup_files = integration
            .as_mut()
            .and_then(|integration| integration.startup_files.take());
        let mut command = launch.command(config.working_dir.as_deref());
        // Startup asynchronously warms the complete shell environment. Copy
        // values from the shared snapshot here without starting another shell.
        // If a terminal is created before warm-up finishes, retain portable-pty's
        // inherited environment as a non-blocking fallback.
        let shell_snapshot = self
            .shell_environment
            .snapshot()
            .ok()
            .flatten()
            .filter(|snapshot| shell.matches_environment(snapshot));
        configure_environment(&mut command, shell_snapshot.as_deref());

        let reader = pair
            .master
            .try_clone_reader()
            .map_err(SessionError::CloneReader)?;
        let (writer, cancellation) = pair
            .master
            .take_cancellable_writer()
            .map_err(SessionError::TakeWriter)?;
        let mut child = pair
            .slave
            .spawn_command(command)
            .map_err(SessionError::Spawn)?;
        drop(pair.slave);

        let info = SessionInfo {
            id: session_id.clone(),
            name: config.name,
            kind: SessionKind::LocalPty,
            working_dir: config.working_dir.clone(),
            cols: config.cols,
            rows: config.rows,
        };
        let mut writer = match QueuedTransportWriter::spawn(
            session_id.clone(),
            writer,
            WriterFlushPolicy::AfterWrite,
            self.event_queue.clone(),
        ) {
            Ok(mut writer) => {
                writer.cancellation = Some(cancellation);
                writer
            }
            Err(error) => {
                let killed = child.kill().is_ok();
                // No reader worker exists. Close its pipe before ConPTY teardown
                // so flushing buffered output cannot wait for a missing drainer.
                drop(reader);
                drop(pair.master);
                if killed {
                    let _ = child.wait();
                }
                return Err(SessionError::Worker(error));
            }
        };
        self.event_queue.push(SessionEvent::CommandContext {
            session_id: session_id.clone(),
            context: command_context,
        });
        let reader_thread = match spawn_reader_thread(
            session_id.clone(),
            reader,
            self.event_queue.clone(),
            integration,
        ) {
            Ok(reader) => reader,
            Err(error) => {
                writer.stop();
                let killed = child.kill().is_ok();
                drop(pair.master);
                writer.close();
                if killed {
                    let _ = child.wait();
                }
                return Err(SessionError::Worker(error));
            }
        };
        let session = LocalPtyTransport {
            info: info.clone(),
            cwd_path_style,
            wsl_distro,
            startup_files,
            master: Some(pair.master),
            writer,
            child,
            reader_thread: Some(reader_thread),
            event_queue: self.event_queue.clone(),
            cleanup_consumer: None,
        };

        sessions.insert(session_id, ManagedSession::Local(session));

        Ok(info)
    }

    pub fn create_telnet_session(
        &self,
        config: TelnetSessionConfig,
    ) -> Result<SessionInfo, SessionError> {
        self.create_telnet_session_with_attempt(config, Default::default())
    }

    pub fn create_telnet_session_with_attempt(
        &self,
        config: TelnetSessionConfig,
        attempt: connection_attempt::ConnectionAttempt,
    ) -> Result<SessionInfo, SessionError> {
        self.ensure_reaper()?;
        let session_id = uuid::Uuid::new_v4().to_string();
        let addr = format!("{}:{}", config.host, config.port);
        let connect = || -> Result<TcpStream, std::io::Error> {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            let stream = runtime
                .block_on(attempt.until_cancelled(tokio::net::TcpStream::connect((
                    config.host.as_str(),
                    config.port,
                ))))
                .map_err(std::io::Error::other)??;
            let stream = stream.into_std()?;
            stream.set_nonblocking(false)?;
            Ok(stream)
        };
        let (stream, route) = if config.proxy.is_some() || config.proxy_jump.is_some() {
            let (stream, route) = crate::network_route::NetworkRoute::connect_with_attempt(
                crate::network_route::NetworkRouteConfig {
                    host: config.host.clone(),
                    port: config.port,
                    proxy: config.proxy.clone(),
                    jump: config.proxy_jump.as_deref().cloned(),
                },
                attempt.clone(),
            )
            .map_err(|source| SessionError::ConnectRoute {
                addr: addr.clone(),
                source,
            })?;
            (stream, Some(route))
        } else {
            (
                connect().map_err(|source| SessionError::ConnectTcp {
                    addr: addr.clone(),
                    source,
                })?,
                None,
            )
        };
        stream.set_nodelay(true).ok();
        stream
            .set_read_timeout(Some(Duration::from_millis(250)))
            .ok();

        let writer_stream = stream
            .try_clone()
            .map_err(|source| SessionError::CloneTcp {
                session_id: session_id.clone(),
                source,
            })?;
        let reader_stream = stream
            .try_clone()
            .map_err(|source| SessionError::CloneTcp {
                session_id: session_id.clone(),
                source,
            })?;
        let shutdown_stream = stream
            .try_clone()
            .map_err(|source| SessionError::CloneTcp {
                session_id: session_id.clone(),
                source,
            })?;
        let lifecycle = Arc::new(TelnetWriterLifecycle {
            stopped: AtomicBool::new(false),
            stream: shutdown_stream,
        });

        let info = SessionInfo {
            id: session_id.clone(),
            name: config.name.clone(),
            kind: if config.raw_tcp {
                SessionKind::RawTcp
            } else {
                SessionKind::Telnet
            },
            working_dir: None,
            cols: config.cols,
            rows: config.rows,
        };

        // Fail before spawning either worker if session registration is unavailable.
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| SessionError::LockPoisoned)?;
        let auto_login =
            TelnetAutoLoginState::new(&config).map(|state| Arc::new(Mutex::new(state)));
        let mut writer = QueuedTransportWriter::spawn_with_telnet_policy(
            session_id.clone(),
            writer_stream,
            if config.force_character_at_a_time {
                WriterFlushPolicy::EachByte
            } else {
                WriterFlushPolicy::AfterWrite
            },
            self.event_queue.clone(),
            Some(TelnetWriterPolicy {
                idle_interval: (!config.raw_tcp && config.keep_alive_interval_secs != 0)
                    .then(|| Duration::from_secs(u64::from(config.keep_alive_interval_secs))),
                lifecycle: lifecycle.clone(),
            }),
        )
        .map_err(SessionError::Worker)?;
        let naws = TelnetNawsWriter::new(writer.command_tx.clone(), &config);
        naws.offer().map_err(|error| {
            lifecycle.stop();
            writer.close();
            if let Some(route) = &route {
                route.close();
            }
            SessionError::Worker(error)
        })?;
        let reader_thread = spawn_tcp_reader_thread(
            session_id.clone(),
            reader_stream,
            writer.command_tx.clone(),
            lifecycle.clone(),
            config.clone(),
            naws.clone(),
            auto_login.clone(),
            self.event_queue.clone(),
        )
        .map_err(|error| {
            lifecycle.stop();
            writer.close();
            SessionError::Worker(error)
        })?;

        let session = TelnetTransport {
            route,
            info: info.clone(),
            writer,
            reader_stream: stream,
            backspace_as_bs: nyaterm_core::terminal::connection_input::BackspaceMode::parse(
                &config.backspace_mode,
            )
                == nyaterm_core::terminal::connection_input::BackspaceMode::Backspace,
            input: TelnetInputState::default(),
            auto_login,
            naws,
            event_queue: self.event_queue.clone(),
            config,
            reader_thread: Some(reader_thread),
        };

        sessions.insert(session_id, ManagedSession::Tcp(Box::new(session)));

        Ok(info)
    }

    pub fn create_ssh_session(
        &self,
        config: SshSessionConfig,
    ) -> Result<SessionInfo, SessionError> {
        let multiplex =
            if config.runtime_mode == nyaterm_core::models::ssh_runtime::SshRuntimeMode::Sftp {
                Some(open_ssh_multiplex_handle(config.clone()).map_err(|source| {
                    SessionError::CreateSsh {
                        addr: format!("{}:{}", config.host, config.port),
                        source,
                    }
                })?)
            } else {
                None
            };
        self.create_ssh_session_inner(config, multiplex)
    }

    pub fn create_ssh_session_with_multiplex(
        &self,
        mut config: SshSessionConfig,
        multiplex: SshMultiplexHandle,
    ) -> Result<SessionInfo, SessionError> {
        config
            .apply_runtime_policy()
            .map_err(|message| SessionError::CreateSsh {
                addr: format!("{}:{}", config.host, config.port),
                source: anyhow::Error::msg(message),
            })?;
        multiplex
            .ensure_matches_config(&config)
            .map_err(|source| SessionError::CreateSsh {
                addr: format!("{}:{}", config.host, config.port),
                source,
            })?;
        self.create_ssh_session_inner(config, Some(multiplex))
    }

    pub(crate) fn create_ssh_session_inner(
        &self,
        mut config: SshSessionConfig,
        multiplex: Option<SshMultiplexHandle>,
    ) -> Result<SessionInfo, SessionError> {
        config
            .apply_runtime_policy()
            .map_err(|message| SessionError::CreateSsh {
                addr: format!("{}:{}", config.host, config.port),
                source: anyhow::Error::msg(message),
            })?;
        self.ensure_reaper()?;
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| SessionError::LockPoisoned)?;
        let session_id = uuid::Uuid::new_v4().to_string();
        let addr = format!("{}:{}", config.host, config.port);
        validate_ssh_algorithm_preferences(config.ssh_algorithms.as_ref()).map_err(|source| {
            SessionError::CreateSsh {
                addr: addr.clone(),
                source: source.into(),
            }
        })?;
        let (command_tx, command_rx) = ssh_mailbox::channel();
        let (tmux_tx, tmux_cmd_rx) = tmux::mailbox::channel();
        let tmux_active = Arc::new(AtomicBool::new(false));
        let worker_tmux_active = tmux_active.clone();
        // The control session builds pane transports sharing this sender so
        // every command lands on one ordered control channel.
        let worker_tmux_tx = tmux_tx.clone();
        let (ready_tx, ready_rx) = mpsc::channel();
        let event_queue = self.event_queue.clone();
        let attempt = multiplex
            .as_ref()
            .map(|handle| handle.inner.attempt.clone())
            .unwrap_or_else(|| config.attempt.clone());
        let worker_config = config.clone();
        let worker_session_id = session_id.clone();
        let shell_environment = self.shell_environment();
        let ssh_services = self.ssh_services.clone();
        let worker_sessions = self.sessions.clone();
        let worker_thread =
            nyaterm_core::runtime::worker::spawn(format!("ssh-worker-{session_id}"), move || {
                run_ssh_worker(
                    worker_session_id,
                    worker_config,
                    command_rx,
                    ready_tx,
                    event_queue,
                    multiplex,
                    shell_environment,
                    ssh_services,
                    tmux_cmd_rx,
                    worker_tmux_tx,
                    worker_tmux_active,
                    worker_sessions,
                );
            })
            .map_err(SessionError::Worker)?;

        match ready_rx.recv() {
            Ok(Ok(())) => {}
            Ok(Err(message)) => {
                let _ = worker_thread.join();
                return Err(SessionError::CreateSsh {
                    addr,
                    source: anyhow::anyhow!(message),
                });
            }
            Err(error) => {
                let _ = worker_thread.join();
                return Err(SessionError::CreateSsh {
                    addr,
                    source: anyhow::anyhow!("SSH worker exited before readiness: {error}"),
                });
            }
        }

        let info = SessionInfo {
            id: session_id.clone(),
            name: config.name,
            kind: SessionKind::Ssh,
            working_dir: None,
            cols: config.cols,
            rows: config.rows,
        };
        let session = SshChannelTransport {
            info: info.clone(),
            shell_enabled: config.runtime_mode
                != nyaterm_core::models::ssh_runtime::SshRuntimeMode::Sftp,
            attempt,
            command_tx,
            tmux_tx,
            tmux_active,
            backspace_as_bs: nyaterm_core::terminal::connection_input::BackspaceMode::parse(
                &config.backspace_mode,
            )
                == nyaterm_core::terminal::connection_input::BackspaceMode::Backspace,
            worker_thread: Some(worker_thread),
        };

        sessions.insert(session_id, ManagedSession::Ssh(session));

        Ok(info)
    }

    pub fn create_serial_session(
        &self,
        config: SerialSessionConfig,
    ) -> Result<SessionInfo, SessionError> {
        self.ensure_reaper()?;
        let session_id = uuid::Uuid::new_v4().to_string();
        let port = open_serial_port(&config).map_err(|source| SessionError::OpenSerial {
            port_name: config.port_name.clone(),
            source,
        })?;
        let reader = port
            .try_clone()
            .map_err(|source| SessionError::CloneSerial {
                session_id: session_id.clone(),
                source,
            })?;

        let info = SessionInfo {
            id: session_id.clone(),
            name: config.name,
            kind: SessionKind::Serial,
            working_dir: None,
            cols: 80,
            rows: 24,
        };
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| SessionError::LockPoisoned)?;
        let mut writer = QueuedTransportWriter::spawn(
            session_id.clone(),
            port,
            WriterFlushPolicy::serial(),
            self.event_queue.clone(),
        )
        .map_err(SessionError::Worker)?;
        let stop_reader = Arc::new(AtomicBool::new(false));
        let reader_thread = spawn_serial_reader_thread(
            session_id.clone(),
            reader,
            stop_reader.clone(),
            self.event_queue.clone(),
        )
        .map_err(|error| {
            writer.close();
            SessionError::Worker(error)
        })?;
        let session = SerialTransport {
            flow_control: config.flow_control,
            info: info.clone(),
            writer,
            backspace_as_bs: nyaterm_core::terminal::connection_input::BackspaceMode::parse(
                &config.backspace_mode,
            )
                == nyaterm_core::terminal::connection_input::BackspaceMode::Backspace,
            stop_reader,
            reader_thread: Some(reader_thread),
        };

        sessions.insert(session_id, ManagedSession::Serial(session));

        Ok(info)
    }

    pub fn list_serial_ports(&self) -> Result<Vec<String>, SessionError> {
        let mut ports = serialport::available_ports()
            .map_err(|source| SessionError::OpenSerial {
                port_name: "<list>".to_string(),
                source,
            })?
            .into_iter()
            .map(|port| port.port_name)
            .collect::<Vec<_>>();
        ports.sort_unstable();
        Ok(ports)
    }

    pub fn list_sessions(&self) -> Result<Vec<SessionInfo>, SessionError> {
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| SessionError::LockPoisoned)?
            .values()
            .map(ManagedSession::info)
            .collect::<Vec<_>>();
        sessions.sort_by(|left, right| left.name.cmp(&right.name).then(left.id.cmp(&right.id)));
        Ok(sessions)
    }

    pub fn write(&self, session_id: &str, data: &[u8]) -> Result<(), SessionError> {
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| SessionError::LockPoisoned)?;
        let session = sessions
            .get_mut(session_id)
            .ok_or_else(|| SessionError::NotFound(session_id.to_string()))?;
        session.write(data).map_err(|source| SessionError::Write {
            session_id: session_id.to_string(),
            source,
        })
    }

    /// Software flow control may consume XON/XOFF in binary transfer payloads.
    pub fn supports_modem_transfer(&self, session_id: &str) -> bool {
        self.sessions.lock().is_ok_and(|sessions| {
            // Output already queued by an exited session still needs protocol filtering.
            // Session existence is checked by the eventual writer, not this policy gate.
            match sessions.get(session_id) {
                Some(ManagedSession::TmuxPane(_)) => false,
                Some(ManagedSession::Ssh(ssh)) if ssh.tmux_active.load(Ordering::Acquire) => false,
                Some(ManagedSession::Serial(serial)) => {
                    serial.flow_control
                        != nyaterm_core::models::connection::SerialFlowControl::Software
                }
                _ => true,
            }
        })
    }

    pub(crate) fn write_queue_peak_bytes(&self, session_id: &str) -> usize {
        let Ok(sessions) = self.sessions.lock() else {
            return 0;
        };
        match sessions.get(session_id) {
            Some(ManagedSession::Local(s)) => s.writer.command_tx.peak_bytes(),
            Some(ManagedSession::Tcp(s)) => s.writer.command_tx.peak_bytes(),
            Some(ManagedSession::Serial(s)) => s.writer.command_tx.peak_bytes(),
            Some(ManagedSession::Ssh(s)) => s.command_tx.peak_bytes(),
            Some(ManagedSession::TmuxPane(pane)) => pane.tmux_tx.peak_bytes(),
            None => 0,
        }
    }

    pub fn zmodem_transfer_options(&self, session_id: &str) -> zmodem::ZmodemTransferOptions {
        let serial = self.sessions.lock().is_ok_and(|sessions| {
            matches!(sessions.get(session_id), Some(ManagedSession::Serial(_)))
        });
        if serial {
            zmodem::ZmodemTransferOptions {
                buffered_files: true,
                ..Default::default()
            }
        } else {
            zmodem::ZmodemTransferOptions::reliable_transport()
        }
    }

    /// Send protocol bytes without transport text-input transformations.
    /// Background transfer path: preserve each frame until accepted, with a
    /// bounded wait between attempts and cancellation independent of the FIFO.
    pub fn write_raw_cancellable(
        &self,
        session_id: &str,
        data: &[u8],
        cancelled: &AtomicBool,
    ) -> Result<(), SessionError> {
        self.write_raw_cancellable_measured(session_id, data, cancelled)
            .map(|_| ())
    }

    pub(crate) fn write_raw_cancellable_measured(
        &self,
        session_id: &str,
        data: &[u8],
        cancelled: &AtomicBool,
    ) -> Result<Duration, SessionError> {
        let wire_bytes = {
            let sessions = self
                .sessions
                .lock()
                .map_err(|_| SessionError::LockPoisoned)?;
            match sessions.get(session_id) {
                Some(ManagedSession::TmuxPane(_)) => {
                    return Err(SessionError::Write {
                        session_id: session_id.into(),
                        source: tmux_modem_unsupported().into(),
                    });
                }
                Some(ManagedSession::Ssh(ssh)) if ssh.tmux_active.load(Ordering::Acquire) => {
                    return Err(SessionError::Write {
                        session_id: session_id.into(),
                        source: tmux_modem_unsupported().into(),
                    });
                }
                Some(ManagedSession::Tcp(session)) => {
                    telnet_wire_size(data, session.config.raw_tcp)
                }
                _ => data.len(),
            }
        };
        if wire_bytes > writer_mailbox::BYTE_LIMIT {
            return Err(SessionError::Write {
                session_id: session_id.to_owned(),
                source: std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "transfer frame exceeds input queue capacity",
                )
                .into(),
            });
        }
        let mut waited = Duration::ZERO;
        loop {
            if cancelled.load(Ordering::Acquire) {
                return Err(SessionError::Write {
                    session_id: session_id.into(),
                    source: std::io::Error::from(std::io::ErrorKind::Interrupted).into(),
                });
            }
            match self.write_raw(session_id, data) {
                Err(error) if error.is_backpressure() => {
                    let target = {
                        let sessions = self
                            .sessions
                            .lock()
                            .map_err(|_| SessionError::LockPoisoned)?;
                        match sessions.get(session_id) {
                            Some(ManagedSession::Local(s)) => {
                                WriteCapacity::Native(s.writer.command_tx.clone())
                            }
                            Some(ManagedSession::Tcp(s)) => {
                                WriteCapacity::Native(s.writer.command_tx.clone())
                            }
                            Some(ManagedSession::Serial(s)) => {
                                WriteCapacity::Native(s.writer.command_tx.clone())
                            }
                            Some(ManagedSession::Ssh(s)) => {
                                WriteCapacity::Ssh(s.command_tx.clone())
                            }
                            Some(ManagedSession::TmuxPane(_)) => {
                                return Err(SessionError::Write {
                                    session_id: session_id.into(),
                                    source: tmux_modem_unsupported().into(),
                                });
                            }
                            None => return Err(SessionError::NotFound(session_id.into())),
                        }
                    };
                    let start = Instant::now();
                    target
                        .wait(wire_bytes, cancelled)
                        .map_err(|source| SessionError::Write {
                            session_id: session_id.into(),
                            source: source.into(),
                        })?;
                    waited += start.elapsed();
                }
                result => return result.map(|()| waited),
            }
        }
    }

    pub fn write_raw(&self, session_id: &str, data: &[u8]) -> Result<(), SessionError> {
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| SessionError::LockPoisoned)?;
        let session = sessions
            .get_mut(session_id)
            .ok_or_else(|| SessionError::NotFound(session_id.to_string()))?;
        session
            .write_raw(data)
            .map_err(|source| SessionError::Write {
                session_id: session_id.to_string(),
                source,
            })
    }

    pub fn resize(&self, session_id: &str, cols: u16, rows: u16) -> Result<(), SessionError> {
        self.resize_with_pixels(session_id, cols, rows, 0, 0)
    }

    /// Resize the live session, including total pixel dimensions when known.
    /// Pixel size is used by local PTY masters and SSH `window-change` / `request_pty`.
    pub fn resize_with_pixels(
        &self,
        session_id: &str,
        cols: u16,
        rows: u16,
        pixel_width: u16,
        pixel_height: u16,
    ) -> Result<(), SessionError> {
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| SessionError::LockPoisoned)?;
        let session = sessions
            .get_mut(session_id)
            .ok_or_else(|| SessionError::NotFound(session_id.to_string()))?;
        session
            .resize(cols, rows, pixel_width, pixel_height)
            .map_err(|source| SessionError::Resize {
                session_id: session_id.to_string(),
                source,
            })
    }

    pub fn close(&self, session_id: &str) -> Result<(), SessionError> {
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| SessionError::LockPoisoned)?;
        let mut session = sessions
            .remove(session_id)
            .ok_or_else(|| SessionError::NotFound(session_id.to_string()))?;
        if let ManagedSession::Ssh(ssh) = &session {
            self.event_queue.cancel_session(session_id);
            cancel_tmux_pane_output(&self.event_queue, &sessions, &ssh.tmux_tx);
        }
        drop(sessions);
        if let ManagedSession::Local(local) = &mut session {
            local.cleanup_consumer = self.event_queue.consumer_for(session_id);
        }
        self.event_queue.cancel_session(session_id);
        session.close();
        Ok(())
    }

    /// Explicitly terminate a remote tmux pane. Keep the projection until the
    /// server confirms its removal, and coalesce repeated close gestures.
    pub fn tmux_close_pane(&self, session_id: &str) -> Result<(), SessionError> {
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| SessionError::LockPoisoned)?;
        let Some(ManagedSession::TmuxPane(pane)) = sessions.get_mut(session_id) else {
            return Err(SessionError::NotFound(session_id.into()));
        };
        if !pane.close_requested {
            pane.tmux_tx
                .send(SshCommand::TmuxPaneClose {
                    pane: pane.pane.clone(),
                })
                .map_err(|source| SessionError::Write {
                    session_id: session_id.into(),
                    source: source.into(),
                })?;
            pane.close_requested = true;
        }
        Ok(())
    }

    /// Queue one tmux control-channel command line (`select-window`,
    /// `new-window`, `kill-pane`, ...). Refuses while the session is a plain
    /// SSH shell so commands can never be typed into it.
    pub fn tmux_send_command(&self, session_id: &str, line: &str) -> Result<(), SessionError> {
        let sessions = self
            .sessions
            .lock()
            .map_err(|_| SessionError::LockPoisoned)?;
        let Some(ManagedSession::Ssh(session)) = sessions.get(session_id) else {
            return Err(SessionError::Write {
                session_id: session_id.into(),
                source: anyhow::anyhow!("session is not an SSH session in tmux control mode"),
            });
        };
        if !session.tmux_active.load(Ordering::Acquire) {
            return Err(SessionError::Write {
                session_id: session_id.into(),
                source: anyhow::anyhow!("session is not in tmux control mode"),
            });
        }
        session
            .tmux_tx
            .send(SshCommand::TmuxCommand {
                line: line.to_string(),
            })
            .map_err(|source| SessionError::Write {
                session_id: session_id.into(),
                source: source.into(),
            })
    }

    /// Leave tmux control mode; the remote tmux session keeps running and the
    /// channel returns to the interactive shell.
    pub fn tmux_detach(&self, session_id: &str) -> Result<(), SessionError> {
        let sessions = self
            .sessions
            .lock()
            .map_err(|_| SessionError::LockPoisoned)?;
        let Some(ManagedSession::Ssh(session)) = sessions.get(session_id) else {
            return Err(SessionError::Write {
                session_id: session_id.into(),
                source: anyhow::anyhow!("session is not an SSH session in tmux control mode"),
            });
        };
        if !session.tmux_active.load(Ordering::Acquire) {
            return Err(SessionError::Write {
                session_id: session_id.into(),
                source: anyhow::anyhow!("session is not in tmux control mode"),
            });
        }
        session
            .tmux_tx
            .send(SshCommand::TmuxDetach)
            .map_err(|source| SessionError::Write {
                session_id: session_id.into(),
                source: source.into(),
            })
    }

    /// Register a projected tmux pane as a virtual session so `write`,
    /// `resize` and `close` route through the control channel.
    pub(crate) fn register_tmux_pane(
        sessions: &Arc<Mutex<HashMap<String, ManagedSession>>>,
        event_queue: &SessionEventQueue,
        host_id: &str,
        info: SessionInfo,
        pane: String,
        tmux_tx: TmuxCommandSender,
    ) -> Result<(), SessionError> {
        let mut sessions = sessions.lock().map_err(|_| SessionError::LockPoisoned)?;
        // Host cancellation, pane registration and reopening its output must
        // share the registry lock. Otherwise a late layout response could
        // reopen a cancelled pane queue while close is joining its producer.
        if event_queue.session_cancelled(host_id) {
            return Err(SessionError::NotFound(host_id.into()));
        }
        event_queue.resume_session(&info.id);
        sessions.insert(
            info.id.clone(),
            ManagedSession::TmuxPane(TmuxPaneTransport {
                close_requested: false,
                info,
                pane,
                tmux_tx,
            }),
        );
        Ok(())
    }

    pub(crate) fn remove_tmux_pane(
        sessions: &Arc<Mutex<HashMap<String, ManagedSession>>>,
        event_queue: &SessionEventQueue,
        session_id: &str,
    ) {
        if let Ok(mut sessions) = sessions.lock() {
            sessions.remove(session_id);
        }
        event_queue.cancel_session(session_id);
    }

    pub fn try_recv_event(&self) -> Result<Option<SessionEvent>, SessionError> {
        Ok(self.event_queue.drain(1).events.into_iter().next())
    }

    pub fn drain_events(&self, max_events: usize) -> Result<SessionDrain, SessionError> {
        Ok(self.event_queue.drain(max_events))
    }

    pub fn drain_events_with_output_budget(
        &self,
        max_events: usize,
        max_output_bytes: usize,
    ) -> Result<SessionDrain, SessionError> {
        Ok(self
            .event_queue
            .drain_with_output_budget(max_events, Some(max_output_bytes)))
    }

    /// Like [`Self::drain_events_with_output_budget`], but parks up to `timeout`
    /// waiting for the first event instead of returning an empty drain.
    ///
    /// Only for a dedicated consumer thread — the UI tick path must keep using
    /// the non-blocking variants.
    pub fn drain_events_blocking_with_output_budget(
        &self,
        max_events: usize,
        max_output_bytes: usize,
        timeout: Duration,
    ) -> Result<SessionDrain, SessionError> {
        Ok(self.event_queue.drain_blocking_with_output_budget(
            max_events,
            Some(max_output_bytes),
            timeout,
        ))
    }

    /// Wait without removing events so a caller can acquire its handoff lock
    /// before draining. Readiness does not reserve events: the subsequent drain
    /// must check current consumer ownership again.
    pub fn wait_for_events_for_consumer(
        &self,
        consumer_id: SessionEventConsumerId,
        timeout: Duration,
    ) -> bool {
        self.event_queue
            .wait_for_events_for_consumer(consumer_id, timeout)
    }

    pub fn drain_events_blocking_for_consumer_with_output_budget(
        &self,
        consumer_id: SessionEventConsumerId,
        max_events: usize,
        max_output_bytes: usize,
        timeout: Duration,
    ) -> Result<SessionDrain, SessionError> {
        Ok(self
            .event_queue
            .drain_blocking_for_consumer_with_output_budget(
                consumer_id,
                max_events,
                Some(max_output_bytes),
                timeout,
            ))
    }
}

impl ManagedSession {
    pub(crate) fn info(&self) -> SessionInfo {
        match self {
            Self::Local(session) => session.info.clone(),
            Self::Ssh(session) => session.info.clone(),
            Self::Tcp(session) => session.info.clone(),
            Self::Serial(session) => session.info.clone(),
            Self::TmuxPane(pane) => pane.info.clone(),
        }
    }

    pub(crate) fn write(&mut self, data: &[u8]) -> anyhow::Result<()> {
        match self {
            Self::Local(session) => session.write(data),
            Self::Tcp(session) => session.write(data),
            Self::Ssh(session) => session.write(data),
            Self::Serial(session) => session.write(data),
            Self::TmuxPane(pane) => pane.write(data),
        }
    }

    pub(crate) fn write_raw(&mut self, data: &[u8]) -> anyhow::Result<()> {
        match self {
            Self::Local(session) => session.write_raw(data),
            Self::Tcp(session) => session.write_raw(data),
            Self::Ssh(session) => session.write_raw(data),
            Self::Serial(session) => session.write_raw(data),
            Self::TmuxPane(pane) => pane.write_raw(data),
        }
    }

    pub(crate) fn resize(
        &mut self,
        cols: u16,
        rows: u16,
        pixel_width: u16,
        pixel_height: u16,
    ) -> anyhow::Result<()> {
        match self {
            Self::Local(session) => session.resize(cols, rows, pixel_width, pixel_height),
            Self::Tcp(session) => session.resize(cols, rows, pixel_width, pixel_height),
            Self::Ssh(session) => session.resize(cols, rows, pixel_width, pixel_height),
            Self::Serial(session) => session.resize(cols, rows, pixel_width, pixel_height),
            Self::TmuxPane(pane) => pane.resize(cols, rows, pixel_width, pixel_height),
        }
    }

    pub(crate) fn close(&mut self) {
        match self {
            Self::Local(session) => {
                let _ = session.close();
            }
            Self::Tcp(session) => {
                let _ = session.close();
            }
            Self::Ssh(session) => {
                let _ = session.close();
            }
            Self::Serial(session) => {
                let _ = session.close();
            }
            Self::TmuxPane(pane) => {
                let _ = pane.close();
            }
        }
    }
}

impl Drop for SessionManager {
    fn drop(&mut self) {
        self.event_queue.close();
        let sessions = {
            let mut sessions = match self.sessions.lock() {
                Ok(sessions) => sessions,
                Err(poisoned) => poisoned.into_inner(),
            };
            std::mem::take(&mut *sessions)
        };
        // SSH teardown removes its projections from this same registry. Never
        // retain the registry guard while joining a worker. Closing the queue
        // above first wakes producers blocked on any pane's output budget.
        for (_, mut session) in sessions {
            session.close();
        }
    }
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}
