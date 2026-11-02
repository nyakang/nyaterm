#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
mod control;
#[cfg(any(windows, test))]
mod display;
#[cfg(windows)]
mod events;

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, TryRecvError};
use std::time::{Duration, Instant};

use nyaterm_remote_desktop::activex::{
    HostCommand, HostEvent, NativeRdpConnectionEvent, decode, encode,
};
use nyaterm_remote_desktop::{PROTOCOL_VERSION, RdpError, RdpErrorKind, read_packet, write_packet};

fn send(event: &HostEvent) -> io::Result<()> {
    write_packet(&mut io::stdout().lock(), &encode(event)?)
}

fn failure(kind: RdpErrorKind, message: &str) -> HostEvent {
    HostEvent::Connection(NativeRdpConnectionEvent::Failed(RdpError::new(
        kind, message,
    )))
}

fn main() {
    let result = std::panic::catch_unwind(run);
    if !matches!(result, Ok(Ok(()))) {
        let _ = send(&failure(RdpErrorKind::HelperCrashed, "ActiveX host failed"));
        std::process::exit(1);
    }
}

#[cfg(windows)]
struct OleApartment;
#[cfg(windows)]
impl OleApartment {
    fn new() -> io::Result<Self> {
        unsafe { windows::Win32::System::Ole::OleInitialize(None) }
            .map_err(|_| io::Error::other("Cannot initialize the ActiveX STA"))?;
        Ok(Self)
    }
}
#[cfg(windows)]
impl Drop for OleApartment {
    fn drop(&mut self) {
        unsafe {
            windows::Win32::System::Ole::OleUninitialize();
        }
    }
}

fn run() -> io::Result<()> {
    // IPC reads never touch COM. The main thread owns the STA and every HWND.
    let (tx, rx) = mpsc::sync_channel(32);
    let input_closed = Arc::new(AtomicBool::new(false));
    let reader_closed = Arc::clone(&input_closed);
    // EOF is also the parent-death signal. Keep this independent of the STA so
    // an unresponsive COM call cannot leave an orphan holding credentials.
    std::thread::Builder::new()
        .name("activex-parent-watch".into())
        .spawn(move || {
            while !input_closed.load(Ordering::Acquire) {
                std::thread::sleep(Duration::from_millis(100));
            }
            std::thread::sleep(Duration::from_secs(1));
            std::process::exit(0);
        })?;
    std::thread::Builder::new()
        .name("activex-stdin".into())
        .spawn(move || {
            let mut input = io::stdin().lock();
            while let Ok(Some(packet)) = read_packet(&mut input) {
                let Ok(command) = decode::<HostCommand>(&packet) else {
                    break;
                };
                if tx.try_send(command).is_err() {
                    break;
                }
            }
            reader_closed.store(true, Ordering::Release);
        })?;
    #[cfg(windows)]
    let _apartment = OleApartment::new()?;
    #[cfg(windows)]
    let mut control: Option<control::NativeRdpControl> = None;
    let mut parent_pid = None;
    let started = Instant::now();
    let mut last_details = Instant::now();
    loop {
        #[cfg(windows)]
        unsafe {
            use windows::Win32::UI::WindowsAndMessaging::{
                DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, TranslateMessage,
            };
            let mut message = MSG::default();
            // Keep the IPC clock responsive even under a continuous paint stream.
            for _ in 0..64 {
                if !PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                    break;
                }
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        match rx.try_recv() {
            Ok(HostCommand::Hello {
                version,
                parent_pid: pid,
            }) if parent_pid.is_none() && version == PROTOCOL_VERSION => {
                parent_pid = Some(pid);
                send(&HostEvent::Hello {
                    version: PROTOCOL_VERSION,
                })?;
            }
            Ok(HostCommand::Hello { .. }) => {
                send(&failure(
                    RdpErrorKind::Protocol,
                    "ActiveX IPC protocol mismatch",
                ))?;
                return Ok(());
            }
            Ok(HostCommand::Close) if parent_pid.is_some() => {
                #[cfg(windows)]
                drop(control.take());
                send(&HostEvent::Closed)?;
                return Ok(());
            }
            Ok(command) if parent_pid.is_some() => match command {
                HostCommand::Ping => send(&HostEvent::Pong)?,
                HostCommand::Prepare {
                    parent,
                    rect,
                    config,
                } => {
                    #[cfg(windows)]
                    {
                        if control.is_some() {
                            return Err(io::Error::other("Duplicate ActiveX connect"));
                        }
                        match control::NativeRdpControl::create(
                            parent,
                            parent_pid.unwrap(),
                            rect,
                            &config,
                        ) {
                            Ok(native) => {
                                send(&HostEvent::Prepared {
                                    child: native.window_handle(),
                                })?;
                                control = Some(native);
                            }
                            Err(_) => {
                                send(&failure(
                                    RdpErrorKind::Negotiation,
                                    "Cannot initialize or connect the ActiveX control",
                                ))?;
                                return Ok(());
                            }
                        }
                    }
                    #[cfg(not(windows))]
                    {
                        let _ = (parent, rect, config);
                        send(&failure(
                            RdpErrorKind::Unsupported,
                            "ActiveX is only available on Windows",
                        ))?;
                        return Ok(());
                    }
                }
                HostCommand::Start =>
                {
                    #[cfg(windows)]
                    if let Some(control) = &mut control {
                        control.start().map_err(io::Error::other)?;
                    }
                }
                HostCommand::Viewport { rect, visible } => {
                    #[cfg(windows)]
                    if let Some(control) = &mut control {
                        control.viewport(rect, visible).map_err(io::Error::other)?;
                    }
                    #[cfg(not(windows))]
                    let _ = (rect, visible);
                }
                HostCommand::Focus =>
                {
                    #[cfg(windows)]
                    if let Some(control) = &control {
                        control.focus();
                    }
                }
                _ => return Err(io::Error::other("Unexpected ActiveX command")),
            },
            Ok(_) => return Err(io::Error::other("ActiveX handshake required")),
            Err(TryRecvError::Disconnected) => return Ok(()),
            Err(TryRecvError::Empty) => {}
        }
        if parent_pid.is_none() && started.elapsed() > Duration::from_secs(5) {
            return Ok(());
        }
        #[cfg(windows)]
        if let Some(control) = &mut control {
            for event in control.tick().map_err(io::Error::other)? {
                send(&HostEvent::Connection(event))?;
            }
            if last_details.elapsed() >= Duration::from_secs(1) {
                send(&HostEvent::Details(control.details()))?;
                send(&HostEvent::PointerInside(control.pointer_inside()))?;
                last_details = Instant::now();
            }
        }
        #[cfg(not(windows))]
        let _ = &mut last_details;
        std::thread::sleep(Duration::from_millis(10));
    }
}
