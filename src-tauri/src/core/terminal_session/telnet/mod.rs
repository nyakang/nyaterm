//! Telnet session: raw TCP with basic IAC negotiation, bridged to the session manager.

use crate::config::AiExecutionProfile;
use crate::core::capture::OutputCaptureProcessor;
use crate::core::input::remap_del_to_bs;
use crate::core::session::{
    DynamicTitleCapabilities, SessionCommand, SessionCommandReceiver, SessionCommandSender,
    SessionHandle, SessionInfo, SessionManager, SessionReadyHook, SessionType, SharedCwd,
    session_command_channel,
};
use crate::core::terminal_session::{TerminalOutputDecoder, encode_terminal_input};
use crate::core::zmodem::{
    ZmodemAction, ZmodemDetectResult, ZmodemDetector, ZmodemDirection, ZmodemDownloadOoDrain,
    ZmodemEvent, ZmodemTransfer, ZmodemUploadDrain, start_zmodem_transfer,
};
use crate::core::{RecordingManager, SessionOutputCoalescer};
use crate::error::AppResult;
use crate::observability::{StructuredLog, StructuredLogLevel, log_event, log_rate_limited};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{Mutex as TokioMutex, mpsc};

const IAC: u8 = 255;
const WILL: u8 = 251;
const WONT: u8 = 252;
const DO: u8 = 253;
const DONT: u8 = 254;
const SB: u8 = 250;
const SE: u8 = 240;

const OPT_ECHO: u8 = 1;
const OPT_SUPPRESS_GO_AHEAD: u8 = 3;
const OPT_NAWS: u8 = 31;

include!("types.rs");
include!("negotiation.rs");
include!("line_editor.rs");
include!("auto_login.rs");
include!("tests.rs");
include!("manager.rs");
include!("session.rs");
