use crate::features::NyaTermApp;
use crate::features::formatting::recent_terminal_output;
use crate::features::runtime_jobs::await_blocking_job;
use gpui::Context;
use nyaterm_core::plugins::extension::{HostRequest, TaskEvent};
use nyaterm_core::plugins::{ErrorCode, PluginError, PluginResult};
use nyaterm_plugin_host::desktop_bridge::DesktopRequest;
use nyaterm_plugin_host::tasks::MAX_TASK_BYTES;

fn unavailable() -> PluginError {
    PluginError::new(
        ErrorCode::NotFound,
        "Extension session is no longer available",
    )
}

fn complete(bytes: Vec<u8>) -> PluginResult<TaskEvent> {
    Ok(TaskEvent::Completed {
        bytes,
        exit_code: None,
    })
}

impl NyaTermApp {
    pub(super) fn dispatch_extension_host(
        &mut self,
        message: DesktopRequest,
        cx: &mut Context<Self>,
    ) {
        let DesktopRequest {
            request,
            context,
            reply,
        } = message;
        if let Err(error) = context.check(&request) {
            let _ = reply.send(Err(error));
            return;
        }
        // Transport assigns a new UUID for every connection, including reconnects.
        // Generation zero denotes that UUID instance; no active-tab lookup is used.
        if let Some(session) = request.session()
            && (session.generation != 0
                || self.session.metadata(&session.id).is_none_or(|metadata| {
                    metadata.disconnected || metadata.source_connection_id != session.connection_id
                }))
        {
            let _ = reply.send(Err(unavailable()));
            return;
        }
        match &request {
            HostRequest::Sessions => {
                let values: Vec<_> = self.session.session_order().iter().filter(|id| self.session.metadata(id).is_some_and(|metadata| !metadata.disconnected)).filter_map(|id| {
                    self.session.session_info(id).map(|info| serde_json::json!({
                        "id": id, "generation": 0, "name": info.name, "kind": format!("{:?}", info.kind),
                        "connection_id": self.session.metadata(id).and_then(|metadata| metadata.source_connection_id.clone()),
                    }))
                }).collect();
                let _ = reply.send(complete(serde_json::to_vec(&values).unwrap()));
            }
            HostRequest::TerminalSnapshot { session, lines } => {
                let output = recent_terminal_output(
                    self.terminal_buffer_tail_for_session(&session.id),
                    (*lines as usize).clamp(1, 500),
                );
                let _ = reply.send(complete(output.into_bytes()));
            }
            HostRequest::Notify { message, .. } => {
                self.shell
                    .set_status(message.chars().take(1024).collect::<String>());
                cx.notify();
                let _ = reply.send(complete(Vec::new()));
            }
            HostRequest::ShowPanel {
                session,
                id,
                title,
                elements,
            } => {
                let key = nyaterm_core::plugins::management::PluginKey::Wasm(context.plugin_id());
                // Updating an existing tool must not steal focus from another tab or input.
                let detail = self
                    .plugins
                    .details
                    .get(&key)
                    .cloned()
                    .unwrap_or_else(|| self.open_plugin_details(key, 3, cx));
                let result = detail.update(cx, |panel, cx| {
                    panel.show_tool_panel(
                        context.plugin_id(),
                        session.clone(),
                        id.clone(),
                        title.clone(),
                        elements.clone(),
                        cx,
                    )
                });
                let _ = reply.send(result.and_then(|_| complete(Vec::new())));
            }
            HostRequest::TerminalWrite { session, bytes } => {
                if bytes.len() > MAX_TASK_BYTES {
                    let _ = reply.send(Err(PluginError::new(
                        ErrorCode::PackageLimit,
                        "Terminal input exceeds its limit",
                    )));
                    return;
                }
                let manager = self.session.manager_handle();
                let id = session.id.clone();
                let bytes = bytes.clone();
                let task = self
                    .blocking_jobs
                    .submit_task("extension-terminal-write", move |_| {
                        context.check(&request)?;
                        manager.write(&id, &bytes).map_err(|_| unavailable())?;
                        complete(Vec::new())
                    });
                cx.spawn(async move |_, _| {
                    let result = await_blocking_job(task)
                        .await
                        .unwrap_or_else(|_| Err(unavailable()));
                    let _ = reply.send(result);
                })
                .detach();
            }
            HostRequest::RemoteExecute { session, command } => {
                let Some(config) = self
                    .session
                    .metadata(&session.id)
                    .and_then(|m| m.ssh_config.clone())
                else {
                    let _ = reply.send(Err(PluginError::new(
                        ErrorCode::Incompatible,
                        "This session has no independent SSH execution channel",
                    )));
                    return;
                };
                let Some(lease) = self.session.ssh_connection_for_session(&session.id) else {
                    let _ = reply.send(Err(unavailable()));
                    return;
                };
                let command = command.as_bytes().to_vec();
                let manager = self.session.manager_handle();
                let id = session.id.clone();
                let task = self.blocking_jobs.submit_task("extension-ssh-execute", move |_| {
                    context.check(&request)?;
                    nyaterm_transport::ssh_execution::run_ssh_exec_operation(async move {
                        use nyaterm_transport::ssh_execution::streaming::{stream_ssh_command, SshExecEvent};
                        let cancel = tokio_util::sync::CancellationToken::new();
                        let (events, mut receiver) = tokio::sync::mpsc::channel(16);
                        let worker = tokio::spawn(stream_ssh_command(config, Some(lease.handle()), command,
                            std::time::Duration::from_secs(300), cancel.clone(), events));
                        let mut output = Vec::new();
                        let mut exit_code = None;
                        let mut failure = None;
                        let mut tick = tokio::time::interval(std::time::Duration::from_millis(25));
                        loop {
                            tokio::select! {
                                _ = tick.tick() => {
                                    if context.check(&request).is_err() || lease.is_closed()
                                        || !manager.list_sessions().is_ok_and(|sessions| sessions.iter().any(|s| s.id == id)) {
                                        failure = Some(PluginError::new(ErrorCode::Cancelled, "Extension execution cancelled"));
                                        cancel.cancel(); break;
                                    }
                                }
                                event = receiver.recv() => match event {
                                    Some(SshExecEvent::Output { data, .. }) => {
                                        if output.len() + data.len() > MAX_TASK_BYTES {
                                            failure = Some(PluginError::new(ErrorCode::PackageLimit, "Extension execution output exceeded its limit"));
                                            cancel.cancel(); break;
                                        }
                                        if let Err(error) = context.output(&request, &data) {
                                            failure = Some(error); cancel.cancel(); break;
                                        }
                                        output.extend(data);
                                    }
                                    Some(SshExecEvent::ExitStatus(code)) => exit_code = i32::try_from(code).ok(),
                                    Some(SshExecEvent::Failed | SshExecEvent::FailedBeforeStart | SshExecEvent::TimedOut | SshExecEvent::OutputDropped { .. }) => {
                                        failure = Some(PluginError::new(ErrorCode::Io, "Extension SSH execution failed")); cancel.cancel(); break;
                                    }
                                    None => break,
                                    _ => {}
                                }
                            }
                        }
                        drop(receiver);
                        let _ = worker.await;
                        drop(lease);
                        Ok(if let Some(error) = failure { Err(error) } else if exit_code.is_none() {
                            Err(PluginError::new(ErrorCode::Io, "SSH command ended without an exit status"))
                        } else { Ok(TaskEvent::Completed { bytes: output, exit_code }) })
                    }).map_err(|_| PluginError::new(ErrorCode::Io, "Cannot run extension SSH task"))?
                });
                cx.spawn(async move |_, _| {
                    let _ = reply.send(
                        await_blocking_job(task)
                            .await
                            .unwrap_or_else(|_| Err(unavailable())),
                    );
                })
                .detach();
            }
            HostRequest::RemoteRead { session, .. } | HostRequest::RemoteWrite { session, .. } => {
                let Some(config) = self
                    .session
                    .metadata(&session.id)
                    .and_then(|m| m.ssh_config.clone())
                else {
                    let _ = reply.send(Err(PluginError::new(
                        ErrorCode::Incompatible,
                        "Remote files require SSH",
                    )));
                    return;
                };
                let service = match self.remote_file_service_for_session(
                    &session.id,
                    config,
                    crate::features::session::remote_files::FileAccessPurpose::Automation,
                ) {
                    Ok(service) => service,
                    Err(_) => {
                        let _ = reply.send(Err(unavailable()));
                        return;
                    }
                };
                let manager = self.session.manager_handle();
                let id = session.id.clone();
                let task = self
                    .blocking_jobs
                    .submit_task("extension-remote-file", move |_| {
                        context.check(&request)?;
                        let checked_request = request.clone();
                        let cancelled: std::sync::Arc<dyn Fn() -> bool + Send + Sync> =
                            std::sync::Arc::new(move || {
                                context.check(&checked_request).is_err()
                                    || !manager.list_sessions().is_ok_and(|sessions| {
                                        sessions.iter().any(|session| session.id == id)
                                    })
                            });
                        let result = match request {
                            HostRequest::RemoteRead {
                                path,
                                offset,
                                length,
                                ..
                            } => service.read_chunk(&path, offset, length, cancelled),
                            HostRequest::RemoteWrite {
                                path,
                                offset,
                                bytes,
                                ..
                            } => service
                                .write_chunk(&path, offset, bytes, cancelled)
                                .map(|_| Vec::new()),
                            _ => unreachable!(),
                        };
                        complete(result.map_err(|_| {
                            PluginError::new(
                                ErrorCode::Io,
                                "Extension remote file operation failed",
                            )
                        })?)
                    });
                cx.spawn(async move |_, _| {
                    let _ = reply.send(
                        await_blocking_job(task)
                            .await
                            .unwrap_or_else(|_| Err(unavailable())),
                    );
                })
                .detach();
            }
            _ => {
                let _ = reply.send(Err(PluginError::new(
                    ErrorCode::Incompatible,
                    "This desktop operation is not supported",
                )));
            }
        }
    }
}
