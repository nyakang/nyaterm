//! Serial port session: opens a serial device and bridges I/O to the session manager.

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
    ZmodemAction, ZmodemDetectResult, ZmodemDetector, ZmodemDirection, ZmodemEvent, ZmodemTransfer,
    start_zmodem_transfer,
};
use crate::core::{RecordingManager, SessionOutputCoalescer};
use crate::error::{AppError, AppResult};
use crate::observability::{StructuredLog, StructuredLogLevel, log_event, log_rate_limited};
use serialport::{DataBits, FlowControl, Parity, SerialPort, StopBits};
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

include!("config.rs");
include!("port.rs");
include!("manager.rs");
include!("session.rs");
