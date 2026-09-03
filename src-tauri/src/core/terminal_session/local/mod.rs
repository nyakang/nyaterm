//! Local PTY (pseudo-terminal) session creation and management.
//!
//! Spawns the user's shell (PowerShell on Windows, $SHELL elsewhere) and bridges I/O to Tauri.

use crate::config::AiExecutionProfile;
use crate::core::SessionOutputCoalescer;
use crate::core::capture::OutputCaptureProcessor;
use crate::core::recording::{InputOrigin, RecordingManager};
use crate::core::replace_cwd_state;
use crate::core::session::{
    DynamicTitleCapabilities, SessionCommand, SessionCommandReceiver, SessionCommandSender,
    SessionHandle, SessionInfo, SessionManager, SessionReadyHook, SessionType, SharedCwd,
    StartupInjectionAttempt, StartupInputBarrier, session_command_channel,
};
use crate::core::ssh::osc::{CwdPayloadEvent, OscStripper, build_ready_marker};
use crate::core::terminal_session::{TerminalOutputDecoder, encode_terminal_input};
use crate::core::zmodem::{
    ZmodemAction, ZmodemDetectResult, ZmodemDetector, ZmodemDirection, ZmodemEvent, ZmodemTransfer,
    start_zmodem_transfer,
};
use crate::error::AppResult;
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex as StdMutex,
    atomic::{AtomicBool, Ordering},
    mpsc as std_mpsc,
};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::mpsc;

include!("config.rs");
include!("shell.rs");
include!("windows_terminal.rs");
include!("args.rs");
include!("environment.rs");
include!("cwd.rs");
include!("startup.rs");
include!("session.rs");
include!("tests.rs");
