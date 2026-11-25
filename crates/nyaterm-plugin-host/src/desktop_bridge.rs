//! Desktop endpoints resolve window/session ownership; no GPUI types cross this boundary.
use crate::tasks::{HostAdapter, TaskContext};
use futures::channel::mpsc as async_channel;
use nyaterm_core::plugins::extension::{HostRequest, TaskEvent};
use nyaterm_core::plugins::{ErrorCode, PluginError, PluginResult};
use std::sync::{Mutex, mpsc};

pub struct DesktopRequest {
    pub request: HostRequest,
    pub context: TaskContext,
    pub reply: mpsc::SyncSender<PluginResult<TaskEvent>>,
}

#[derive(Default)]
pub struct DesktopBridge {
    endpoints: Mutex<Vec<async_channel::Sender<DesktopRequest>>>,
}

impl DesktopBridge {
    pub fn register(&self) -> async_channel::Receiver<DesktopRequest> {
        let (sender, receiver) = async_channel::channel(16);
        let mut endpoints = self.endpoints.lock().unwrap();
        endpoints.retain(|endpoint| !endpoint.is_closed());
        endpoints.push(sender);
        receiver
    }
}

impl HostAdapter for DesktopBridge {
    fn execute(&self, request: HostRequest, context: &TaskContext) -> PluginResult<TaskEvent> {
        let endpoints = {
            let mut endpoints = self.endpoints.lock().unwrap();
            endpoints.retain(|endpoint| !endpoint.is_closed());
            endpoints.clone()
        };
        if endpoints.len() > 1
            && request.session().is_none()
            && matches!(
                request,
                HostRequest::Notify { .. } | HostRequest::ShowPanel { .. }
            )
        {
            return Err(PluginError::new(
                ErrorCode::InvalidResult,
                "A session target is required when multiple windows are open",
            ));
        }
        let mut sessions = Vec::<serde_json::Value>::new();
        for mut endpoint in endpoints {
            context.check(&request)?;
            let (reply, receiver) = mpsc::sync_channel(1);
            if endpoint
                .try_send(DesktopRequest {
                    request: request.clone(),
                    context: context.clone(),
                    reply,
                })
                .is_err()
            {
                continue;
            }
            let result = loop {
                context.check(&request)?;
                match receiver.recv_timeout(std::time::Duration::from_millis(25)) {
                    Ok(result) => break result,
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(_) => {
                        break Err(PluginError::new(
                            ErrorCode::NotFound,
                            "Extension target window closed",
                        ));
                    }
                }
            };
            context.check(&request)?;
            if matches!(request, HostRequest::Sessions) {
                if let Ok(TaskEvent::Completed { bytes, .. }) = result {
                    let values: Vec<serde_json::Value> =
                        serde_json::from_slice(&bytes).map_err(|_| {
                            PluginError::new(ErrorCode::InvalidResult, "Invalid session response")
                        })?;
                    sessions.extend(values);
                }
            } else if !matches!(&result, Err(error) if error.code == ErrorCode::NotFound) {
                return result;
            }
        }
        if matches!(request, HostRequest::Sessions) {
            return Ok(TaskEvent::Completed {
                bytes: serde_json::to_vec(&sessions).unwrap(),
                exit_code: None,
            });
        }
        Err(PluginError::new(
            ErrorCode::NotFound,
            "Extension target is no longer available",
        ))
    }
}
