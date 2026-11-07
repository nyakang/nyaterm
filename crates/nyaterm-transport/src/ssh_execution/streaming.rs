//! Auxiliary exec channels with explicit exit evidence and bounded event delivery.
use crate::ssh_channel::SshChannelConnection;
use crate::{SshMultiplexHandle, SshSessionConfig};
use russh::{ChannelMsg, Sig};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub enum SshExecEvent {
    Started,
    Eof,
    Closed,
    Output { data: Vec<u8>, stderr: bool },
    OutputDropped { bytes: usize },
    InterruptSent,
    ExitStatus(u32),
    ExitSignal { interrupted: bool },
    Finished { confirmed: bool, interrupted: bool },
    TimedOut,
    CancelledBeforeStart,
    Failed,
}

pub async fn stream_ssh_command(
    config: SshSessionConfig,
    multiplex: Option<SshMultiplexHandle>,
    command: Vec<u8>,
    timeout: Duration,
    interrupt: CancellationToken,
    events: mpsc::Sender<SshExecEvent>,
) {
    let deadline = tokio::time::Instant::now() + timeout;
    let connection = tokio::select! {
        biased;
        _=events.closed()=>return,
        _=interrupt.cancelled()=>{let _=send_event(&events, SshExecEvent::CancelledBeforeStart).await;return;}
        result=tokio::time::timeout_at(deadline,async {
            config.ensure_shell_available()?;
            SshChannelConnection::connect(&config,multiplex.as_ref()).await
        })=>match result {
            Ok(Ok(connection))=>connection,
            Ok(Err(_))=>{let _=send_event(&events, SshExecEvent::Failed).await;return;}
            Err(_)=>{let _=send_event(&events, SshExecEvent::TimedOut).await;return;}
        }
    };
    let opened = tokio::select! {
        biased;
        _ = events.closed() => None,
        _ = interrupt.cancelled() => None,
        result = tokio::time::timeout_at(deadline, connection.open_session()) => Some(result),
    };
    let mut channel = match opened {
        Some(Ok(Ok(channel))) => channel,
        _ => {
            let event = if interrupt.is_cancelled() {
                SshExecEvent::CancelledBeforeStart
            } else if tokio::time::Instant::now() >= deadline {
                SshExecEvent::TimedOut
            } else {
                SshExecEvent::Failed
            };
            let _ = send_event(&events, event).await;
            let _ =
                tokio::time::timeout(Duration::from_secs(2), connection.close("exec open failed"))
                    .await;
            return;
        }
    };
    if interrupt.is_cancelled() {
        let _ = tokio::time::timeout(Duration::from_secs(2), async {
            let _ = channel.close().await;
            let _ = connection.close("exec cancelled before start").await;
        })
        .await;
        let _ = send_event(&events, SshExecEvent::CancelledBeforeStart).await;
        return;
    }
    if !matches!(
        tokio::time::timeout_at(deadline, channel.exec(true, command)).await,
        Ok(Ok(()))
    ) {
        let _ = tokio::time::timeout(Duration::from_secs(2), async {
            let _ = channel.close().await;
            let _ = connection.close("exec request failed").await;
        })
        .await;
        let _ = send_event(&events, SshExecEvent::Failed).await;
        return;
    }
    // Noninteractive jobs cannot wait for additional stdin supplied by the caller.
    let _ = tokio::time::timeout_at(deadline, channel.eof()).await;
    let mut confirmed = false;
    let mut eof_seen = false;
    let mut interrupted = false;
    let mut stop_sent = false;
    let mut dropped = 0usize;
    loop {
        let message = tokio::select! {
            biased;
            _=events.closed()=>break,
            _=tokio::time::sleep_until(deadline)=>{
                if dropped > 0 { let _ = send_event(&events, SshExecEvent::OutputDropped { bytes: dropped }).await; }
                let _=send_event(&events, SshExecEvent::TimedOut).await;break;
            }
            _=interrupt.cancelled(),if !stop_sent=>{
                stop_sent=true;
                if matches!(tokio::time::timeout_at(deadline, channel.signal(Sig::INT)).await, Ok(Ok(()))) {let _=send_event(&events, SshExecEvent::InterruptSent).await;}
                continue;
            }
            message=channel.wait()=>message,
        };
        match message {
            Some(ChannelMsg::Success) => {
                config.attempt.record_shell_reply(true);
                let _ = send_event(&events, SshExecEvent::Started).await;
            }
            Some(ChannelMsg::Failure) => {
                config.attempt.record_shell_reply(false);
                let _ = send_event(&events, SshExecEvent::Failed).await;
                break;
            }
            Some(ChannelMsg::Data { data }) | Some(ChannelMsg::ExtendedData { data, ext: 0 }) => {
                emit_output(&events, data.to_vec(), false, &mut dropped);
            }
            Some(ChannelMsg::ExtendedData { data, .. }) => {
                emit_output(&events, data.to_vec(), true, &mut dropped)
            }
            Some(ChannelMsg::ExitStatus { exit_status }) => {
                confirmed = true;
                let _ = send_event(&events, SshExecEvent::ExitStatus(exit_status)).await;
            }
            Some(ChannelMsg::ExitSignal { signal_name, .. }) => {
                confirmed = true;
                interrupted = stop_sent && matches!(signal_name, Sig::INT);
                let _ = send_event(&events, SshExecEvent::ExitSignal { interrupted }).await;
            }
            Some(ChannelMsg::Eof) => {
                eof_seen = true;
                let _ = send_event(&events, SshExecEvent::Eof).await;
            } // EOF is not exit evidence.
            Some(ChannelMsg::Close) | None => {
                let _ = send_event(&events, SshExecEvent::Closed).await;
                if dropped > 0 {
                    let _ =
                        send_event(&events, SshExecEvent::OutputDropped { bytes: dropped }).await;
                }
                let _ = send_event(
                    &events,
                    SshExecEvent::Finished {
                        confirmed,
                        interrupted,
                    },
                )
                .await;
                break;
            }
            _ => {}
        }
        if eof_seen && confirmed {
            if dropped > 0 {
                let _ = send_event(&events, SshExecEvent::OutputDropped { bytes: dropped }).await;
            }
            let _ = send_event(
                &events,
                SshExecEvent::Finished {
                    confirmed,
                    interrupted,
                },
            )
            .await;
            break;
        }
    }
    let _ = tokio::time::timeout(Duration::from_secs(2), async {
        let _ = channel.close().await;
        let _ = connection.close("exec observer finished").await;
    })
    .await;
}
fn emit_output(
    events: &mpsc::Sender<SshExecEvent>,
    data: Vec<u8>,
    stderr: bool,
    dropped: &mut usize,
) {
    if *dropped > 0 {
        if events
            .try_send(SshExecEvent::OutputDropped { bytes: *dropped })
            .is_err()
        {
            *dropped = dropped.saturating_add(data.len());
            return;
        }
        *dropped = 0;
    }
    let len = data.len();
    if events
        .try_send(SshExecEvent::Output { data, stderr })
        .is_err()
    {
        *dropped = dropped.saturating_add(len);
    }
}

async fn send_event(events: &mpsc::Sender<SshExecEvent>, event: SshExecEvent) -> Result<(), ()> {
    tokio::time::timeout(Duration::from_secs(1), events.send(event))
        .await
        .map_err(|_| ())?
        .map_err(|_| ())
}
