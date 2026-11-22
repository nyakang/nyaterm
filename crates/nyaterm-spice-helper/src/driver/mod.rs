//! One-session helper executable driver with bounded independent pipe workers.

mod pipe;

use std::io;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::{Receiver, RecvTimeoutError, TryRecvError};
use std::time::{Duration, Instant};

use nyaterm_remote_desktop::spice::ipc::{encode_cursor, encode_frame};
use nyaterm_remote_desktop::spice::protocol::{
    SPICE_PROTOCOL_VERSION, SpiceControl, SpiceErrorKind, SpiceSessionState,
};

use crate::driver::pipe::{Outbox, start_reader};
use crate::native::{NativeClient, NativeError, NativeEvent, NativeRuntime};

type Inbox = Receiver<io::Result<Option<SpiceControl>>>;

pub fn run_stdio() -> bool {
    #[cfg(target_os = "linux")]
    {
        // A small kernel pipe forces repeated producer/consumer wakeups for
        // each frame. Best-effort tuning is confined to our output descriptor;
        // non-pipe output still receives the ordinary protocol validation.
        let _ = crate::pipe_capacity::configure_frame_pipe(&io::stdout());
    }
    let Ok((outbox, writer)) = Outbox::start(io::stdout()) else {
        return false;
    };
    let result = catch_unwind(AssertUnwindSafe(|| {
        let inbox = start_reader(io::stdin()).map_err(|_| SpiceErrorKind::HelperCrash)?;
        run(inbox, &outbox)
    }))
    .unwrap_or(Err(SpiceErrorKind::HelperCrash));
    let success = result.is_ok();
    outbox.finish(result, writer);
    success
}

fn next(inbox: &Inbox, timeout: Duration) -> Result<Option<SpiceControl>, SpiceErrorKind> {
    match inbox.recv_timeout(timeout) {
        Ok(Ok(command)) => Ok(command),
        Err(RecvTimeoutError::Timeout) => Err(SpiceErrorKind::Timeout),
        Err(RecvTimeoutError::Disconnected) => Ok(None),
        Ok(Err(_)) => Err(SpiceErrorKind::Protocol),
    }
}

fn run(inbox: Inbox, outbox: &Outbox) -> Result<(), SpiceErrorKind> {
    match next(&inbox, Duration::from_secs(10))? {
        Some(SpiceControl::Hello {
            version: SPICE_PROTOCOL_VERSION,
        }) => {}
        None => return Ok(()),
        _ => return Err(SpiceErrorKind::Protocol),
    }
    outbox.control(SpiceControl::HelloAck {
        version: SPICE_PROTOCOL_VERSION,
    })?;
    let config = loop {
        match next(&inbox, Duration::from_secs(60))? {
            Some(SpiceControl::Connect { config }) => break config,
            Some(SpiceControl::Ping { sequence }) => {
                outbox.control(SpiceControl::Pong { sequence })?
            }
            Some(SpiceControl::Disconnect) | None => {
                outbox.control(SpiceControl::State {
                    state: SpiceSessionState::Disconnected,
                })?;
                return Ok(());
            }
            _ => return Err(SpiceErrorKind::Protocol),
        }
    };
    outbox.control(SpiceControl::State {
        state: SpiceSessionState::Connecting,
    })?;
    let runtime = NativeRuntime::load().map_err(|_| SpiceErrorKind::RuntimeMissing)?;
    let mut client = NativeClient::connect(&runtime, *config).map_err(|error| match error {
        NativeError::InvalidField(_) | NativeError::Configuration(_) => {
            SpiceErrorKind::Configuration
        }
        _ => SpiceErrorKind::Connection,
    })?;
    let mut connecting = Some(Instant::now());
    loop {
        if outbox.failed() {
            return Err(SpiceErrorKind::HelperCrash);
        }
        for _ in 0..32 {
            let command = match inbox.try_recv() {
                Ok(Ok(Some(command))) => command,
                Ok(Ok(None)) | Err(TryRecvError::Disconnected) => return Ok(()),
                Ok(Err(_)) => return Err(SpiceErrorKind::Protocol),
                Err(TryRecvError::Empty) => break,
            };
            if matches!(command, SpiceControl::Disconnect) {
                drop(client);
                outbox.control(SpiceControl::State {
                    state: SpiceSessionState::Disconnected,
                })?;
                return Ok(());
            }
            if let SpiceControl::Ping { sequence } = command {
                outbox.control(SpiceControl::Pong { sequence })?;
                continue;
            }
            // Client-originated command failures are recoverable policy/device
            // errors. Wrong-direction controls terminate the IPC session.
            if let Err(error) = dispatch(&mut client, command) {
                if error == SpiceErrorKind::Protocol {
                    return Err(error);
                }
                outbox.control(SpiceControl::Error {
                    error,
                    fatal: false,
                })?;
            }
        }
        if connecting.is_some_and(|start| start.elapsed() >= Duration::from_secs(90)) {
            return Err(SpiceErrorKind::Timeout);
        }
        let Some(event) = client.poll_with_frames(outbox.can_frame()) else {
            if client.failed() {
                return Err(SpiceErrorKind::Protocol);
            }
            std::thread::sleep(Duration::from_millis(2));
            continue;
        };
        match event {
            NativeEvent::Control(control) => {
                match &control {
                    SpiceControl::State {
                        state: SpiceSessionState::Connected,
                    } => connecting = None,
                    SpiceControl::State {
                        state: SpiceSessionState::Migrating,
                    } => connecting = Some(Instant::now()),
                    SpiceControl::State {
                        state: SpiceSessionState::Disconnected,
                    } => {
                        outbox.control(control)?;
                        return Ok(());
                    }
                    SpiceControl::Error { error, fatal: true } => return Err(*error),
                    _ => {}
                }
                outbox.control(control)?;
            }
            NativeEvent::Frame { channel, event } => {
                outbox.send(encode_frame(channel, event).map_err(|_| SpiceErrorKind::Protocol)?)?;
            }
            NativeEvent::Cursor { channel, event } => {
                outbox
                    .send(encode_cursor(channel, &event).map_err(|_| SpiceErrorKind::Protocol)?)?;
            }
        }
    }
}

fn dispatch(client: &mut NativeClient<'_>, command: SpiceControl) -> Result<(), SpiceErrorKind> {
    match command {
        SpiceControl::Input { events } => client.send_input(&events),
        SpiceControl::Clipboard { text } => return client.set_clipboard(text),
        SpiceControl::Resize { monitors } => return client.resize(&monitors),
        SpiceControl::SetVisible { visible } => client.set_visible(visible),
        SpiceControl::SetMuted { muted } => client.set_muted(muted),
        SpiceControl::Refresh => client.refresh(),
        SpiceControl::SendFiles { transfer_id, paths } => {
            return client.send_files(transfer_id, &paths);
        }
        SpiceControl::CancelTransfer { transfer_id } => client.cancel_transfer(transfer_id),
        _ => return Err(SpiceErrorKind::Protocol),
    }
    Ok(())
}
