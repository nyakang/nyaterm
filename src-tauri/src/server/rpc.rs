//! JSON-RPC style bridge over `POST /api/rpc`.
//!
//! Every handler directly calls the corresponding `#[tauri::command]`
//! function — the same code path the desktop webview uses. Managers come
//! from `tauri::State` exactly like in the generated Tauri dispatcher, so no
//! existing command logic is duplicated or modified.
//!
//! Argument keys follow the same camelCase convention the frontend already
//! uses for Tauri's IPC (`session_id` ← `sessionId`), implemented by
//! [`parse_args`].

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use serde_json::{Value, json};
use tauri::Manager;

use crate::error::AppResult;

/// Serialized error surface: matches what the desktop frontend receives from
/// a rejected `invoke()` (Tauri serializes `AppError` to its display string).
pub(crate) type HandlerOutput = Result<Value, String>;
pub(crate) type BoxFut<'a> = Pin<Box<dyn Future<Output = HandlerOutput> + Send + 'a>>;
pub(crate) type HandlerFn =
    Arc<dyn for<'a> Fn(&'a tauri::AppHandle, Value) -> BoxFut<'a> + Send + Sync>;

/// Shared server state handed to every axum handler.
#[derive(Clone)]
pub(crate) struct ServerState {
    pub app: tauri::AppHandle,
    pub auth: Arc<super::auth::AuthToken>,
    pub registry: Arc<HashMap<&'static str, HandlerFn>>,
    pub events: tokio::sync::broadcast::Sender<(String, Value)>,
    pub port: u16,
    pub download_dir: PathBuf,
    pub staging_dir: PathBuf,
    pub dist_dir: Option<PathBuf>,
}

/// Serializes an `AppResult` command result into the RPC response payload.
pub(crate) fn ok<T: serde::Serialize>(result: AppResult<T>) -> HandlerOutput {
    match result {
        Ok(value) => serde_json::to_value(value).map_err(|e| e.to_string()),
        Err(e) => Err(e.to_string()),
    }
}

/// Same as [`ok`] for commands returning `Result<T, String>`.
fn ok_str<T: serde::Serialize>(result: Result<T, String>) -> HandlerOutput {
    match result {
        Ok(value) => serde_json::to_value(value).map_err(|e| e.to_string()),
        Err(e) => Err(e),
    }
}

/// Same as [`ok`] for infallible commands.
fn ok_value<T: serde::Serialize>(value: T) -> HandlerOutput {
    serde_json::to_value(value).map_err(|e| e.to_string())
}

/// Extracts camelCase-keyed arguments into a typed struct, mirroring how
/// Tauri maps JS argument names onto Rust command parameters.
macro_rules! parse_args {
    ($value:expr, $($field:ident : $ty:ty),* $(,)?) => {{
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Args { $($field: $ty),* }
        serde_json::from_value::<Args>($value)
            .map_err(|e| format!("Invalid arguments: {e}"))
    }};
}

/// The hidden window lets commands that take a `WebviewWindow` (they only
/// read `window.label()`) run unchanged.
fn bridge_window(app: &tauri::AppHandle) -> Result<tauri::WebviewWindow, String> {
    use tauri::Manager;
    app.get_webview_window(super::BRIDGE_WINDOW_LABEL)
        .ok_or_else(|| "web bridge window is not available".to_string())
}

pub(crate) fn build_registry() -> Arc<HashMap<&'static str, HandlerFn>> {
    use crate::cmd;
    use crate::core::mcp::McpManager;
    use crate::core::monitoring::stats::RemoteStatsSampler;
    use crate::core::sftp as sftp_core;
    use crate::core::ssh::{
        HostKeyVerifyManager, PendingAuthManager, PendingSshAgentAuthManager, PendingSshAuthManager,
    };
    use crate::core::{CloudSyncManager, QuickCommandsStore, RecordingManager, SessionManager};
    use std::sync::Arc as StdArc;

    let mut handlers: HashMap<&'static str, HandlerFn> = HashMap::new();

    macro_rules! h {
        ($name:literal => |$app:ident, $args:ident| $body:expr) => {{
            #[allow(unused_variables, clippy::elidable_lifetime_names)]
            fn generated<'a>($app: &'a tauri::AppHandle, $args: serde_json::Value) -> BoxFut<'a> {
                Box::pin($body)
            }
            handlers.insert($name, StdArc::new(generated));
        }};
    }

    // ---- session management -------------------------------------------------
    h!("create_ssh_session" => |app, args| async move {
        let a = parse_args!(args,
            connection_id: String,
            create_request_id: Option<String>,
            startup_command: Option<cmd::session::StartupCommandPayload>,
            runtime_mode: Option<crate::config::SshRuntimeMode>)?;
        let window = bridge_window(app)?;
        ok(cmd::session::create_ssh_session(
            app.clone(),
            window,
            app.state::<StdArc<SessionManager>>(),
            app.state::<StdArc<RecordingManager>>(),
            a.connection_id,
            a.create_request_id,
            a.startup_command,
            a.runtime_mode,
        ).await)
    });

    h!("create_temporary_ssh_session" => |app, args| async move {
        let a = parse_args!(args,
            config: crate::core::ssh::SshConfig,
            create_request_id: Option<String>,
            startup_command: Option<cmd::session::StartupCommandPayload>)?;
        let window = bridge_window(app)?;
        ok(cmd::session::create_temporary_ssh_session(
            app.clone(),
            window,
            app.state::<StdArc<SessionManager>>(),
            app.state::<StdArc<RecordingManager>>(),
            a.config,
            a.create_request_id,
            a.startup_command,
        ).await)
    });

    h!("create_multiplexed_ssh_session" => |app, args| async move {
        let a = parse_args!(args,
            source_session_id: String,
            startup_command: Option<cmd::session::StartupCommandPayload>)?;
        ok(cmd::session::create_multiplexed_ssh_session(
            app.clone(),
            app.state::<StdArc<SessionManager>>(),
            app.state::<StdArc<RecordingManager>>(),
            a.source_session_id,
            a.startup_command,
        ).await)
    });

    h!("create_local_session" => |app, args| async move {
        let a = parse_args!(args,
            connection_id: Option<String>,
            create_request_id: Option<String>,
            working_dir: Option<String>)?;
        let window = bridge_window(app)?;
        ok(cmd::session::create_local_session(
            app.clone(),
            window,
            app.state::<StdArc<SessionManager>>(),
            app.state::<StdArc<RecordingManager>>(),
            a.connection_id,
            a.create_request_id,
            a.working_dir,
        ).await)
    });

    h!("create_telnet_session" => |app, args| async move {
        let a = parse_args!(args,
            connection_id: Option<String>,
            host: Option<String>,
            port: Option<u16>,
            name: Option<String>,
            create_request_id: Option<String>,
            startup_command: Option<cmd::session::StartupCommandPayload>)?;
        let window = bridge_window(app)?;
        ok(cmd::session::create_telnet_session(
            app.clone(),
            window,
            app.state::<StdArc<SessionManager>>(),
            app.state::<StdArc<RecordingManager>>(),
            a.connection_id,
            a.host,
            a.port,
            a.name,
            a.create_request_id,
            a.startup_command,
        ).await)
    });

    h!("create_serial_session" => |app, args| async move {
        let a = parse_args!(args,
            connection_id: Option<String>,
            port_name: Option<String>,
            baud_rate: Option<u32>,
            data_bits: Option<u8>,
            parity: Option<String>,
            stop_bits: Option<String>,
            name: Option<String>,
            create_request_id: Option<String>)?;
        let window = bridge_window(app)?;
        ok(cmd::session::create_serial_session(
            app.clone(),
            window,
            app.state::<StdArc<SessionManager>>(),
            app.state::<StdArc<RecordingManager>>(),
            a.connection_id,
            a.port_name,
            a.baud_rate,
            a.data_bits,
            a.parity,
            a.stop_bits,
            a.name,
            a.create_request_id,
        ).await)
    });

    h!("cancel_session_creation" => |app, args| async move {
        let a = parse_args!(args, create_request_id: String)?;
        ok(cmd::session::cancel_session_creation(app.state::<StdArc<SessionManager>>(), a.create_request_id).await)
    });

    h!("list_serial_ports" => |_app, _args| async move {
        ok(cmd::session::list_serial_ports())
    });

    h!("write_to_session" => |app, args| async move {
        let a = parse_args!(args,
            session_id: String,
            data: String,
            origin: Option<crate::core::InputOrigin>,
            sensitivity: Option<crate::core::InputSensitivity>)?;
        ok(cmd::session::write_to_session(app.state::<StdArc<SessionManager>>(), a.session_id, a.data, a.origin, a.sensitivity).await)
    });

    h!("set_session_output_paused" => |app, args| async move {
        let a = parse_args!(args, session_id: String, paused: bool)?;
        ok(cmd::session::set_session_output_paused(app.state::<StdArc<SessionManager>>(), a.session_id, a.paused).await)
    });

    h!("ack_session_output" => |app, args| async move {
        let a = parse_args!(args, session_id: String, bytes: usize)?;
        ok(cmd::session::ack_session_output(app.state::<StdArc<SessionManager>>(), a.session_id, a.bytes).await)
    });

    h!("resize_session" => |app, args| async move {
        let a = parse_args!(args, session_id: String, cols: u32, rows: u32)?;
        ok(cmd::session::resize_session(app.state::<StdArc<SessionManager>>(), a.session_id, a.cols, a.rows).await)
    });

    h!("attach_session" => |app, args| async move {
        let a = parse_args!(args, session_id: String)?;
        ok(cmd::session::attach_session(app.state::<StdArc<SessionManager>>(), a.session_id).await)
    });

    h!("detach_session_renderer" => |app, args| async move {
        let a = parse_args!(args, session_id: String)?;
        ok(cmd::session::detach_session_renderer(app.state::<StdArc<SessionManager>>(), a.session_id).await)
    });

    h!("close_session" => |app, args| async move {
        let a = parse_args!(args, session_id: String)?;
        ok(cmd::session::close_session(
            app.clone(),
            app.state::<StdArc<SessionManager>>(),
            app.state::<StdArc<RemoteStatsSampler>>(),
            a.session_id,
        ).await)
    });

    h!("list_sessions" => |app, _args| async move {
        ok(cmd::session::list_sessions(app.state::<StdArc<SessionManager>>()).await)
    });

    h!("get_session_cwd_presentation" => |app, args| async move {
        let a = parse_args!(args, session_id: String)?;
        ok(cmd::session::get_session_cwd_presentation(app.state::<StdArc<SessionManager>>(), a.session_id).await)
    });

    h!("add_command_history" => |app, args| async move {
        let a = parse_args!(args, session_id: String, command: String)?;
        ok(cmd::session::add_command_history(app.state::<StdArc<SessionManager>>(), a.session_id, a.command).await)
    });

    h!("register_command_submission" => |app, args| async move {
        let a = parse_args!(args, session_id: String, command: String)?;
        ok(cmd::session::register_command_submission(app.state::<StdArc<SessionManager>>(), a.session_id, a.command).await)
    });

    h!("register_command_confirmation_candidate" => |app, args| async move {
        let a = parse_args!(args, session_id: String, command: String)?;
        ok(cmd::session::register_command_confirmation_candidate(app.state::<StdArc<SessionManager>>(), a.session_id, a.command).await)
    });

    h!("get_command_history" => |app, _args| async move {
        ok(cmd::session::get_command_history(app.state::<StdArc<SessionManager>>()).await)
    });

    h!("delete_command_history" => |app, args| async move {
        let a = parse_args!(args, command: String)?;
        ok(cmd::session::delete_command_history(app.state::<StdArc<SessionManager>>(), a.command).await)
    });

    h!("fuzzy_search_history" => |app, args| async move {
        let a = parse_args!(args,
            pattern: String,
            limit: usize,
            min_command_length: Option<usize>,
            max_command_length: Option<usize>)?;
        ok(cmd::session::fuzzy_search_history(app.state::<StdArc<SessionManager>>(), a.pattern, a.limit, a.min_command_length, a.max_command_length).await)
    });

    h!("fuzzy_search_commands" => |app, args| async move {
        let a = parse_args!(args, pattern: String, limit: usize)?;
        ok(cmd::session::fuzzy_search_commands(app.state::<StdArc<QuickCommandsStore>>(), a.pattern, a.limit).await)
    });

    h!("fuzzy_search_candidates" => |app, args| async move {
        let a = parse_args!(args, pattern: String, items: Vec<crate::utils::fuzzy::FuzzySearchCandidate>, limit: usize)?;
        ok(cmd::session::fuzzy_search_candidates(a.pattern, a.items, a.limit).await)
    });

    h!("start_recording" => |app, args| async move {
        let a = parse_args!(args, request: cmd::session::StartRecordingRequest)?;
        ok(cmd::session::start_recording(
            app.clone(),
            app.state::<StdArc<SessionManager>>(),
            app.state::<StdArc<RecordingManager>>(),
            a.request,
        ).await)
    });

    h!("stop_recording" => |app, args| async move {
        let a = parse_args!(args, session_id: String)?;
        ok(cmd::session::stop_recording(app.state::<StdArc<RecordingManager>>(), a.session_id).await)
    });

    h!("is_recording" => |app, args| async move {
        let a = parse_args!(args, session_id: String)?;
        ok(cmd::session::is_recording(app.state::<StdArc<RecordingManager>>(), a.session_id).await)
    });

    h!("list_recording_sessions" => |app, _args| async move {
        ok(cmd::session::list_recording_sessions(app.state::<StdArc<RecordingManager>>()).await)
    });

    h!("get_recording_status" => |app, args| async move {
        let a = parse_args!(args, session_id: String)?;
        ok(cmd::session::get_recording_status(app.state::<StdArc<RecordingManager>>(), a.session_id).await)
    });

    h!("list_recording_statuses" => |app, _args| async move {
        ok(cmd::session::list_recording_statuses(app.state::<StdArc<RecordingManager>>()).await)
    });

    h!("terminal_history_search" => |app, args| async move {
        let a = parse_args!(args, request: crate::core::TerminalHistorySearchRequest)?;
        ok(cmd::session::terminal_history_search(app.state::<StdArc<RecordingManager>>(), a.request).await)
    });

    h!("set_recording_memory_limit" => |app, args| async move {
        let a = parse_args!(args, max_bytes: usize)?;
        ok(cmd::session::set_recording_memory_limit(app.state::<StdArc<RecordingManager>>(), a.max_bytes).await)
    });

    // Interactive auth flows (SSH keyboard-interactive / OTP / host key).
    h!("submit_otp_response" => |app, args| async move {
        let a = parse_args!(args, request_id: String, responses: Vec<String>)?;
        ok(cmd::session::submit_otp_response(app.state::<StdArc<PendingAuthManager>>(), a.request_id, a.responses).await)
    });

    h!("cancel_otp_request" => |app, args| async move {
        let a = parse_args!(args, request_id: String)?;
        ok(cmd::session::cancel_otp_request(app.state::<StdArc<PendingAuthManager>>(), a.request_id).await)
    });

    h!("submit_ssh_auth_response" => |app, args| async move {
        let a = parse_args!(args, request_id: String, response: crate::core::ssh::SshAuthResponse)?;
        ok(cmd::session::submit_ssh_auth_response(app.state::<StdArc<PendingSshAuthManager>>(), a.request_id, a.response).await)
    });

    h!("cancel_ssh_auth_request" => |app, args| async move {
        let a = parse_args!(args, request_id: String)?;
        ok(cmd::session::cancel_ssh_auth_request(app.state::<StdArc<PendingSshAuthManager>>(), a.request_id).await)
    });

    h!("respond_ssh_agent_auth" => |app, args| async move {
        let a = parse_args!(args, request_id: String, action: String)?;
        ok(cmd::session::respond_ssh_agent_auth(app.state::<StdArc<PendingSshAgentAuthManager>>(), a.request_id, a.action).await)
    });

    h!("cancel_ssh_agent_auth" => |app, args| async move {
        let a = parse_args!(args, request_id: String)?;
        ok(cmd::session::cancel_ssh_agent_auth(app.state::<StdArc<PendingSshAgentAuthManager>>(), a.request_id).await)
    });

    h!("respond_host_key_verify" => |app, args| async move {
        let a = parse_args!(args, request_id: String, accepted: bool)?;
        ok(cmd::session::respond_host_key_verify(app.state::<StdArc<HostKeyVerifyManager>>(), a.request_id, a.accepted).await)
    });

    // ---- SFTP ---------------------------------------------------------------
    h!("get_home_dir" => |app, args| async move {
        let a = parse_args!(args, session_id: String)?;
        ok(cmd::sftp::get_home_dir(app.state::<StdArc<SessionManager>>(), a.session_id).await)
    });

    h!("list_remote_dir" => |app, args| async move {
        let a = parse_args!(args, session_id: String, path: String, raw_path_token: Option<String>)?;
        ok(cmd::sftp::list_remote_dir(app.state::<StdArc<SessionManager>>(), a.session_id, a.path, a.raw_path_token).await)
    });

    h!("list_remote_child_directories" => |app, args| async move {
        let a = parse_args!(args, session_id: String, path: String, raw_path_token: Option<String>, show_hidden_files: bool)?;
        ok(cmd::sftp::list_remote_child_directories(app.state::<StdArc<SessionManager>>(), a.session_id, a.path, a.raw_path_token, a.show_hidden_files).await)
    });

    h!("delete_remote_file" => |app, args| async move {
        let a = parse_args!(args, session_id: String, path: String, raw_path_token: Option<String>)?;
        ok(cmd::sftp::delete_remote_file(app.state::<StdArc<SessionManager>>(), a.session_id, a.path, a.raw_path_token).await)
    });

    h!("rename_remote_file" => |app, args| async move {
        let a = parse_args!(args, old_path: String, new_path: String, old_raw_path_token: Option<String>, new_raw_path_token: Option<String>, session_id: String)?;
        ok(cmd::sftp::rename_remote_file(app.state::<StdArc<SessionManager>>(), a.session_id, a.old_path, a.new_path, a.old_raw_path_token, a.new_raw_path_token).await)
    });

    h!("sanitize_download_file_name" => |_app, args| async move {
        let a = parse_args!(args, name: String)?;
        ok_value(cmd::sftp::sanitize_download_file_name(a.name))
    });

    h!("get_file_properties" => |app, args| async move {
        let a = parse_args!(args, session_id: String, path: String, raw_path_token: Option<String>)?;
        ok(cmd::sftp::get_file_properties(app.state::<StdArc<SessionManager>>(), a.session_id, a.path, a.raw_path_token).await)
    });

    h!("read_remote_file_text" => |app, args| async move {
        let a = parse_args!(args, session_id: String, path: String, max_bytes: u64)?;
        ok(cmd::sftp::read_remote_file_text(app.state::<StdArc<SessionManager>>(), a.session_id, a.path, a.max_bytes).await)
    });

    h!("open_remote_file_text" => |app, args| async move {
        let a = parse_args!(args, session_id: String, path: String, max_bytes: u64)?;
        ok(cmd::sftp::open_remote_file_text(app.state::<StdArc<SessionManager>>(), a.session_id, a.path, a.max_bytes).await)
    });

    h!("read_remote_file_bytes" => |app, args| async move {
        let a = parse_args!(args, session_id: String, path: String, max_bytes: u64)?;
        ok(cmd::sftp::read_remote_file_bytes(app.state::<StdArc<SessionManager>>(), a.session_id, a.path, a.max_bytes).await)
    });

    h!("write_remote_file_text" => |app, args| async move {
        let a = parse_args!(args,
            session_id: String,
            path: String,
            content: String,
            expected_mtime: Option<u64>,
            expected_size: Option<u64>,
            expected_hash: Option<String>,
            force: Option<bool>)?;
        ok(cmd::sftp::write_remote_file_text(
            app.state::<StdArc<SessionManager>>(),
            a.session_id,
            a.path,
            a.content,
            a.expected_mtime,
            a.expected_size,
            a.expected_hash,
            a.force,
        ).await)
    });

    h!("create_remote_file" => |app, args| async move {
        let a = parse_args!(args, session_id: String, path: String, mode: Option<String>)?;
        ok(cmd::sftp::create_remote_file(app.state::<StdArc<SessionManager>>(), a.session_id, a.path, a.mode).await)
    });

    h!("create_remote_dir" => |app, args| async move {
        let a = parse_args!(args, session_id: String, path: String, mode: Option<String>)?;
        ok(cmd::sftp::create_remote_dir(app.state::<StdArc<SessionManager>>(), a.session_id, a.path, a.mode).await)
    });

    h!("create_remote_symlink" => |app, args| async move {
        let a = parse_args!(args, session_id: String, link_path: String, target_path: String)?;
        ok(cmd::sftp::create_remote_symlink(app.state::<StdArc<SessionManager>>(), a.session_id, a.link_path, a.target_path).await)
    });

    h!("update_remote_symlink_target" => |app, args| async move {
        let a = parse_args!(args, session_id: String, path: String, raw_path_token: Option<String>, target_path: String)?;
        ok(cmd::sftp::update_remote_symlink_target(app.state::<StdArc<SessionManager>>(), a.session_id, a.path, a.raw_path_token, a.target_path).await)
    });

    h!("chmod_remote_file" => |app, args| async move {
        let a = parse_args!(args, session_id: String, path: String, mode: String)?;
        ok(cmd::sftp::chmod_remote_file(app.state::<StdArc<SessionManager>>(), a.session_id, a.path, a.mode).await)
    });

    h!("update_remote_file_attributes" => |app, args| async move {
        let a = parse_args!(args, session_id: String, path: String, raw_path_token: Option<String>, update: sftp_core::RemoteFileAttributeUpdate)?;
        ok(cmd::sftp::update_remote_file_attributes(app.state::<StdArc<SessionManager>>(), a.session_id, a.path, a.raw_path_token, a.update).await)
    });

    h!("copy_file_entry" => |app, args| async move {
        let a = parse_args!(args, request: sftp_core::CopyFileEntryRequest)?;
        ok(cmd::sftp::copy_file_entry(app.clone(), app.state::<StdArc<SessionManager>>(), a.request).await)
    });

    h!("pause_transfer" => |app, args| async move {
        let a = parse_args!(args, transfer_id: String)?;
        ok(cmd::sftp::pause_transfer(app.clone(), a.transfer_id).await)
    });

    h!("resume_transfer" => |app, args| async move {
        let a = parse_args!(args, transfer_id: String)?;
        ok(cmd::sftp::resume_transfer(app.clone(), a.transfer_id).await)
    });

    h!("cancel_transfer" => |app, args| async move {
        let a = parse_args!(args, transfer_id: String)?;
        ok(cmd::sftp::cancel_transfer(app.clone(), a.transfer_id).await)
    });

    h!("respond_transfer_duplicate" => |app, args| async move {
        let a = parse_args!(args, request_id: String, action: String)?;
        ok(cmd::sftp::respond_transfer_duplicate(app.state::<StdArc<crate::core::sftp::TransferDuplicateManager>>(), a.request_id, a.action).await)
    });

    // Web-only variants of the file transfer commands: the frontend dialog
    // shim produces `__web_upload__` / `__web_downloads__` marker paths that
    // are resolved by super::files against server-side staging directories.
    h!("download_remote_file" => |app, args| async move {
        let a = parse_args!(args, session_id: String, remote_path: String, local_path: String, transfer_id: Option<String>)?;
        super::files::download_remote_file_web(app.clone(), a.session_id, a.remote_path, a.local_path, a.transfer_id).await
    });

    h!("upload_local_file" => |app, args| async move {
        let a = parse_args!(args, session_id: String, local_path: String, remote_path: String, transfer_id: Option<String>, duplicate_strategy_override: Option<String>)?;
        super::files::upload_local_file_web(app.clone(), a.session_id, a.local_path, a.remote_path, a.transfer_id, a.duplicate_strategy_override).await
    });

    // ---- connections / credentials / keys / passwords -----------------------
    h!("get_saved_connections" => |app, _args| async move {
        ok(cmd::connection::get_saved_connections(app.clone()))
    });

    h!("get_supported_ssh_algorithms" => |_app, _args| async move {
        ok_value(cmd::connection::get_supported_ssh_algorithms())
    });

    h!("get_ssh_agent_forwarding_identities" => |app, args| async move {
        let a = parse_args!(args, forwarding_config: crate::config::SshAgentForwardingConfig)?;
        ok(cmd::connection::get_ssh_agent_forwarding_identities(app.clone(), a.forwarding_config).await)
    });

    h!("get_connection_custom_icons" => |app, _args| async move {
        ok(cmd::connection::get_connection_custom_icons(app.clone()))
    });

    h!("delete_connection_custom_icon" => |app, args| async move {
        let a = parse_args!(args, id: String)?;
        ok(cmd::connection::delete_connection_custom_icon(app.clone(), a.id))
    });

    h!("import_connection_icon" => |app, args| async move {
        let a = parse_args!(args, path: String)?;
        let path = super::files::resolve_staged_path_for(app, &a.path)?;
        ok(cmd::connection::import_connection_icon(app.clone(), path))
    });

    h!("update_connection_icon" => |app, args| async move {
        let a = parse_args!(args, connection_id: String, icon: Option<String>, icon_auto_detect: bool)?;
        ok(cmd::connection::update_connection_icon(app.clone(), a.connection_id, a.icon, a.icon_auto_detect))
    });

    h!("update_connection_asset_from_monitoring" => |app, args| async move {
        let a = parse_args!(args, connection_id: String, asset_patch: crate::config::AssetMetadata)?;
        ok(cmd::connection::update_connection_asset_from_monitoring(app.clone(), a.connection_id, a.asset_patch))
    });

    h!("save_connection" => |app, args| async move {
        let a = parse_args!(args, connection: crate::config::SavedConnection)?;
        ok(cmd::connection::save_connection(app.clone(), a.connection))
    });

    h!("delete_connection" => |app, args| async move {
        let a = parse_args!(args, id: String)?;
        ok(cmd::connection::delete_connection(app.clone(), a.id))
    });

    h!("get_connection_password_value" => |app, args| async move {
        let a = parse_args!(args, id: String)?;
        ok(cmd::connection::get_connection_password_value(app.clone(), a.id))
    });

    h!("reorder_items" => |app, args| async move {
        let a = parse_args!(args, connections: Vec<crate::cmd::connection::SortOrderUpdate>, groups: Vec<crate::cmd::connection::SortOrderUpdate>)?;
        ok(cmd::connection::reorder_items(app.clone(), a.connections, a.groups))
    });

    h!("get_ssh_keys" => |app, _args| async move {
        ok(cmd::connection::get_ssh_keys(app.clone()))
    });

    h!("get_ssh_key_passphrase" => |app, args| async move {
        let a = parse_args!(args, id: String)?;
        ok(cmd::connection::get_ssh_key_passphrase(app.clone(), a.id))
    });

    h!("get_ssh_key_private_key" => |app, args| async move {
        let a = parse_args!(args, id: String)?;
        ok(cmd::connection::get_ssh_key_private_key(app.clone(), a.id))
    });

    h!("save_ssh_key" => |app, args| async move {
        let a = parse_args!(args, key: crate::config::SshKey)?;
        ok(cmd::connection::save_ssh_key(app.clone(), a.key))
    });

    h!("delete_ssh_key" => |app, args| async move {
        let a = parse_args!(args, id: String)?;
        ok(cmd::connection::delete_ssh_key(app.clone(), a.id))
    });

    h!("get_groups" => |app, _args| async move {
        ok(cmd::connection::get_groups(app.clone()))
    });

    h!("save_group" => |app, args| async move {
        let a = parse_args!(args, group: crate::config::Group)?;
        ok(cmd::connection::save_group(app.clone(), a.group))
    });

    h!("delete_group" => |app, args| async move {
        let a = parse_args!(args, id: String)?;
        ok(cmd::connection::delete_group(app.clone(), a.id))
    });

    h!("clear_all_connections" => |app, _args| async move {
        ok(cmd::connection::clear_all_connections(app.clone()))
    });

    h!("get_quick_commands" => |app, _args| async move {
        ok(cmd::connection::get_quick_commands(app.state::<StdArc<QuickCommandsStore>>()))
    });

    h!("save_quick_commands" => |app, args| async move {
        let a = parse_args!(args, config: crate::config::QuickCommandsConfig)?;
        ok(cmd::connection::save_quick_commands(app.clone(), app.state::<StdArc<QuickCommandsStore>>(), a.config))
    });

    h!("upsert_quick_command" => |app, args| async move {
        let a = parse_args!(args, command: crate::config::QuickCommand, new_category: Option<crate::config::QuickCommandCategory>)?;
        ok(cmd::connection::upsert_quick_command(app.clone(), app.state::<StdArc<QuickCommandsStore>>(), a.command, a.new_category))
    });

    h!("increment_quick_command_use_count" => |app, args| async move {
        let a = parse_args!(args, id: String)?;
        ok(cmd::connection::increment_quick_command_use_count(app.clone(), app.state::<StdArc<QuickCommandsStore>>(), a.id))
    });

    h!("import_quick_commands" => |app, args| async move {
        let a = parse_args!(args, file_path: String, source: crate::core::QuickCommandsImportSource)?;
        let file_path = super::files::resolve_staged_path_for(app, &a.file_path)?;
        ok(cmd::connection::import_quick_commands(app.clone(), app.state::<StdArc<QuickCommandsStore>>(), file_path, a.source))
    });

    h!("get_saved_passwords" => |app, _args| async move {
        ok(cmd::connection::get_saved_passwords(app.clone()))
    });

    h!("get_saved_password_value" => |app, args| async move {
        let a = parse_args!(args, id: String)?;
        ok(cmd::connection::get_saved_password_value(app.clone(), a.id))
    });

    h!("save_password" => |app, args| async move {
        let a = parse_args!(args, entry: crate::config::SavedPassword)?;
        ok(cmd::connection::save_password(app.clone(), a.entry))
    });

    h!("delete_password" => |app, args| async move {
        let a = parse_args!(args, id: String)?;
        ok(cmd::connection::delete_password(app.clone(), a.id))
    });

    h!("get_saved_credentials" => |app, _args| async move {
        ok(cmd::credential::get_saved_credentials(app.clone()))
    });

    h!("get_saved_credential_password" => |app, args| async move {
        let a = parse_args!(args, id: String)?;
        ok(cmd::credential::get_saved_credential_password(app.clone(), a.id))
    });

    h!("save_credential" => |app, args| async move {
        let a = parse_args!(args, entry: crate::config::SavedCredential)?;
        ok(cmd::credential::save_credential(app.clone(), a.entry))
    });

    h!("delete_credential" => |app, args| async move {
        let a = parse_args!(args, id: String)?;
        ok(cmd::credential::delete_credential(app.clone(), a.id))
    });

    h!("reorder_credentials" => |app, args| async move {
        let a = parse_args!(args, updates: Vec<crate::cmd::credential::CredentialSortOrderUpdate>)?;
        ok(cmd::credential::reorder_credentials(app.clone(), a.updates))
    });

    // ---- settings / otp / tunnels / proxies ---------------------------------
    h!("get_app_settings" => |app, _args| async move {
        ok(cmd::settings::get_app_settings(app.clone()))
    });

    h!("save_app_settings" => |app, args| async move {
        let a = parse_args!(args,
            settings: crate::config::AppSettings,
            allow_master_password_change: Option<bool>,
            owner_window_label: Option<String>)?;
        ok(cmd::settings::save_app_settings(
            app.clone(),
            app.state::<StdArc<CloudSyncManager>>(),
            app.state::<StdArc<McpManager>>(),
            a.settings,
            a.allow_master_password_change,
            a.owner_window_label,
        ).await)
    });

    h!("save_app_language" => |app, args| async move {
        let a = parse_args!(args, language: String)?;
        ok(cmd::settings::save_app_language(app.clone(), a.language))
    });

    h!("save_app_ui_settings" => |app, args| async move {
        let a = parse_args!(args, ui: crate::config::UiConfig)?;
        ok(cmd::settings::save_app_ui_settings(a.ui))
    });

    h!("verify_master_password" => |app, args| async move {
        let a = parse_args!(args, password: String)?;
        ok(cmd::settings::verify_master_password(app.clone(), a.password))
    });

    h!("get_otp_entries" => |app, _args| async move {
        ok(cmd::otp::get_otp_entries(app.clone()))
    });

    h!("get_otp_secret_value" => |app, args| async move {
        let a = parse_args!(args, id: String)?;
        ok(cmd::otp::get_otp_secret_value(app.clone(), a.id))
    });

    h!("save_otp_entry" => |app, args| async move {
        let a = parse_args!(args, entry: crate::config::OtpEntry)?;
        ok(cmd::otp::save_otp_entry(app.clone(), a.entry))
    });

    h!("delete_otp_entry" => |app, args| async move {
        let a = parse_args!(args, id: String)?;
        ok(cmd::otp::delete_otp_entry(app.clone(), a.id))
    });

    h!("generate_otp_code" => |app, args| async move {
        let a = parse_args!(args, id: String)?;
        ok(cmd::otp::generate_otp_code(app.clone(), a.id))
    });

    h!("get_tunnels" => |app, _args| async move {
        ok(cmd::tunnel::get_tunnels(app.clone()).await)
    });

    h!("get_tunnel_runtime_states" => |app, _args| async move {
        ok(cmd::tunnel::get_tunnel_runtime_states(app.clone(), app.state::<StdArc<crate::core::ssh::TunnelManager>>()).await)
    });

    h!("get_tunnel_groups" => |app, _args| async move {
        ok(cmd::tunnel::get_tunnel_groups(app.clone()))
    });

    h!("save_tunnel" => |app, args| async move {
        let a = parse_args!(args, tunnel: crate::config::TunnelConfig)?;
        ok(cmd::tunnel::save_tunnel(app.clone(), app.state::<StdArc<crate::core::ssh::TunnelManager>>(), a.tunnel).await)
    });

    h!("save_tunnel_group" => |app, args| async move {
        let a = parse_args!(args, group: crate::config::TunnelGroup)?;
        ok(cmd::tunnel::save_tunnel_group(app.clone(), a.group))
    });

    h!("set_tunnel_group" => |app, args| async move {
        let a = parse_args!(args, tunnel_id: String, group_id: Option<String>)?;
        ok(cmd::tunnel::set_tunnel_group(app.clone(), a.tunnel_id, a.group_id))
    });

    h!("delete_tunnel" => |app, args| async move {
        let a = parse_args!(args, tunnel_id: String)?;
        ok(cmd::tunnel::delete_tunnel(app.clone(), app.state::<StdArc<crate::core::ssh::TunnelManager>>(), a.tunnel_id).await)
    });

    h!("delete_tunnel_group" => |app, args| async move {
        let a = parse_args!(args, group_id: String)?;
        ok(cmd::tunnel::delete_tunnel_group(app.clone(), app.state::<StdArc<crate::core::ssh::TunnelManager>>(), a.group_id).await)
    });

    h!("open_tunnel" => |app, args| async move {
        let a = parse_args!(args, tunnel_id: String)?;
        ok(cmd::tunnel::open_tunnel(app.clone(), app.state::<StdArc<crate::core::ssh::TunnelManager>>(), a.tunnel_id).await)
    });

    h!("close_tunnel" => |app, args| async move {
        let a = parse_args!(args, tunnel_id: String)?;
        ok(cmd::tunnel::close_tunnel(app.clone(), app.state::<StdArc<crate::core::ssh::TunnelManager>>(), a.tunnel_id).await)
    });

    h!("get_proxies" => |app, _args| async move {
        ok(cmd::proxy::get_proxies(app.clone()))
    });

    h!("get_proxy_groups" => |app, _args| async move {
        ok(cmd::proxy::get_proxy_groups(app.clone()))
    });

    h!("save_proxy" => |app, args| async move {
        let a = parse_args!(args, proxy: crate::config::ProxyConfig)?;
        ok(cmd::proxy::save_proxy(app.clone(), a.proxy))
    });

    h!("save_proxy_group" => |app, args| async move {
        let a = parse_args!(args, group: crate::config::ProxyGroup)?;
        ok(cmd::proxy::save_proxy_group(app.clone(), a.group))
    });

    h!("set_proxy_group" => |app, args| async move {
        let a = parse_args!(args, proxy_id: String, group_id: Option<String>)?;
        ok(cmd::proxy::set_proxy_group(app.clone(), a.proxy_id, a.group_id))
    });

    h!("delete_proxy" => |app, args| async move {
        let a = parse_args!(args, proxy_id: String)?;
        ok(cmd::proxy::delete_proxy(app.clone(), a.proxy_id))
    });

    h!("delete_proxy_group" => |app, args| async move {
        let a = parse_args!(args, group_id: String)?;
        ok(cmd::proxy::delete_proxy_group(app.clone(), a.group_id))
    });

    h!("get_proxy_password" => |app, args| async move {
        let a = parse_args!(args, proxy_id: String)?;
        ok(cmd::proxy::get_proxy_password(app.clone(), a.proxy_id))
    });

    // ---- ssh config / notes / misc ------------------------------------------
    h!("list_ssh_config_hosts" => |_app, _args| async move {
        ok(cmd::ssh_config::list_ssh_config_hosts())
    });

    h!("get_ssh_config" => |_app, _args| async move {
        ok(cmd::ssh_config::get_ssh_config())
    });

    h!("resolve_ssh_host" => |_app, args| async move {
        let a = parse_args!(args, alias: String)?;
        ok(cmd::ssh_config::resolve_ssh_host(a.alias))
    });

    h!("import_ssh_config_hosts" => |app, _args| async move {
        ok(cmd::ssh_config::import_ssh_config_hosts(app.clone()))
    });

    h!("list_note_tree" => |_app, _args| async move {
        ok(cmd::note::list_note_tree())
    });

    h!("get_note" => |_app, args| async move {
        let a = parse_args!(args, note_id: String)?;
        ok(cmd::note::get_note(a.note_id))
    });

    h!("create_note_folder" => |app, args| async move {
        let a = parse_args!(args, parent_id: Option<String>, name: Option<String>)?;
        ok(cmd::note::create_note_folder(app.clone(), a.parent_id, a.name))
    });

    h!("create_note" => |app, args| async move {
        let a = parse_args!(args, parent_id: Option<String>, title: Option<String>, markdown: Option<String>)?;
        ok(cmd::note::create_note(app.clone(), a.parent_id, a.title, a.markdown))
    });

    h!("update_note" => |app, args| async move {
        let a = parse_args!(args, note_id: String, title: String, markdown: String, expected_revision: u64, force: Option<bool>)?;
        ok(cmd::note::update_note(app.clone(), a.note_id, a.title, a.markdown, a.expected_revision, a.force))
    });

    h!("rename_note_node" => |app, args| async move {
        let a = parse_args!(args, node_kind: String, node_id: String, name: String)?;
        ok(cmd::note::rename_note_node(app.clone(), a.node_kind, a.node_id, a.name))
    });

    h!("move_note_node" => |app, args| async move {
        let a = parse_args!(args, node_kind: String, node_id: String, parent_id: Option<String>, sort_order: i64)?;
        ok(cmd::note::move_note_node(app.clone(), a.node_kind, a.node_id, a.parent_id, a.sort_order))
    });

    h!("delete_note_node" => |app, args| async move {
        let a = parse_args!(args, node_kind: String, node_id: String)?;
        ok(cmd::note::delete_note_node(app.clone(), a.node_kind, a.node_id))
    });

    h!("read_clipboard_text" => |_app, _args| async move {
        ok_value(cmd::clipboard::read_clipboard_text().await)
    });

    h!("write_clipboard_text" => |_app, args| async move {
        let a = parse_args!(args, text: String)?;
        ok_str(cmd::clipboard::write_clipboard_text(a.text).await)
    });

    h!("append_frontend_logs" => |_app, args| async move {
        let a = parse_args!(args, entries: Vec<crate::observability::FrontendLogEntry>)?;
        ok(cmd::log::append_frontend_logs(a.entries))
    });

    h!("get_remote_stats" => |app, args| async move {
        let a = parse_args!(args, session_id: String)?;
        ok(cmd::stats::get_remote_stats(app.state::<StdArc<SessionManager>>(), app.state::<StdArc<RemoteStatsSampler>>(), a.session_id).await)
    });

    h!("get_terminal_cwd" => |app, args| async move {
        let a = parse_args!(args, session_id: String)?;
        ok(cmd::stats::get_terminal_cwd(app.state::<StdArc<SessionManager>>(), a.session_id).await)
    });

    h!("try_get_terminal_cwd" => |app, args| async move {
        let a = parse_args!(args, session_id: String)?;
        ok(cmd::stats::try_get_terminal_cwd(app.state::<StdArc<SessionManager>>(), a.session_id).await)
    });

    h!("get_remote_processes" => |app, args| async move {
        let a = parse_args!(args, session_id: String)?;
        ok(cmd::process::get_remote_processes(app.state::<StdArc<SessionManager>>(), a.session_id).await)
    });

    h!("signal_remote_process" => |app, args| async move {
        let a = parse_args!(args, session_id: String, pid: u32, signal: String)?;
        ok(cmd::process::signal_remote_process(app.state::<StdArc<SessionManager>>(), a.session_id, a.pid, a.signal).await)
    });

    h!("get_remote_gpu_overview" => |app, args| async move {
        let a = parse_args!(args, session_id: String)?;
        ok(cmd::gpu::get_remote_gpu_overview(app.state::<StdArc<SessionManager>>(), a.session_id).await)
    });

    h!("get_remote_ascend_npu_overview" => |app, args| async move {
        let a = parse_args!(args, session_id: String)?;
        ok(cmd::ascend_npu::get_remote_ascend_npu_overview(app.state::<StdArc<SessionManager>>(), a.session_id).await)
    });

    h!("get_remote_docker_overview" => |app, args| async move {
        let a = parse_args!(args, session_id: String)?;
        ok(cmd::docker::get_remote_docker_overview(app.clone(), app.state::<StdArc<SessionManager>>(), app.state::<StdArc<crate::cmd::docker::DockerSudoManager>>(), a.session_id).await)
    });

    h!("get_app_runtime_info" => |app, _args| async move {
        ok_value(cmd::app::get_app_runtime_info(app.state::<crate::runtime::AppRuntime>()))
    });

    h!("get_support_info" => |app, _args| async move {
        ok_value(cmd::app::get_support_info(app.state::<crate::runtime::AppRuntime>()))
    });

    h!("get_app_lock_state" => |app, _args| async move {
        ok_value(cmd::app::get_app_lock_state(app.state::<crate::cmd::app::AppLockState>()))
    });

    h!("set_app_lock_state" => |app, args| async move {
        let a = parse_args!(args, locked: bool)?;
        ok_value(cmd::app::set_app_lock_state(app.clone(), app.state::<crate::cmd::app::AppLockState>(), a.locked))
    });

    h!("get_external_mcp_status" => |app, _args| async move {
        ok_str(cmd::mcp::get_external_mcp_status(app.state::<StdArc<McpManager>>()).await)
    });

    h!("notify_mcp_session_restore_complete" => |app, args| async move {
        let a = parse_args!(args, owner_window_label: String)?;
        ok(cmd::mcp::notify_mcp_session_restore_complete(app.clone(), app.state::<StdArc<McpManager>>(), a.owner_window_label).await)
    });

    h!("report_mcp_active_session" => |app, args| async move {
        let a = parse_args!(args, session_id: Option<String>)?;
        let window = bridge_window(app)?;
        ok(cmd::mcp::report_mcp_active_session(window, app.state::<StdArc<McpManager>>(), a.session_id).await)
    });

    h!("get_external_mcp_client_configs" => |app, _args| async move {
        ok(cmd::mcp::get_external_mcp_client_configs(app.state::<StdArc<McpManager>>()))
    });

    h!("respond_external_mcp_approval" => |app, args| async move {
        let a = parse_args!(args, request_id: String, decision: String)?;
        ok(cmd::mcp::respond_external_mcp_approval(app.state::<StdArc<McpManager>>(), a.request_id, a.decision).await)
    });

    h!("translate_text" => |app, args| async move {
        let a = parse_args!(args, provider: String, text: String, target_language: String)?;
        ok(cmd::translate::translate_text(app.clone(), a.provider, a.text, a.target_language).await)
    });

    // Cloud sync runtime controls.
    h!("test_cloud_sync_connection" => |app, _args| async move {
        ok(cmd::cloud_sync::test_cloud_sync_connection(app.state::<StdArc<CloudSyncManager>>()).await)
    });

    h!("get_cloud_sync_status" => |app, _args| async move {
        ok(cmd::cloud_sync::get_cloud_sync_status(app.state::<StdArc<CloudSyncManager>>()).await)
    });

    h!("sync_push_now" => |app, _args| async move {
        ok(cmd::cloud_sync::sync_push_now(app.state::<StdArc<CloudSyncManager>>()).await)
    });

    h!("sync_pull_now" => |app, _args| async move {
        ok(cmd::cloud_sync::sync_pull_now(app.state::<StdArc<CloudSyncManager>>()).await)
    });

    h!("resolve_cloud_sync_conflict" => |app, args| async move {
        let a = parse_args!(args, action: String)?;
        ok(cmd::cloud_sync::resolve_cloud_sync_conflict(app.state::<StdArc<CloudSyncManager>>(), a.action).await)
    });

    h!("list_cloud_sync_history" => |app, _args| async move {
        ok(cmd::cloud_sync::list_cloud_sync_history(app.state::<StdArc<CloudSyncManager>>()).await)
    });

    h!("begin_github_gist_device_flow" => |_app, _args| async move {
        ok(cmd::cloud_sync::begin_github_gist_device_flow().await)
    });

    h!("poll_github_gist_device_flow" => |_app, args| async move {
        let a = parse_args!(args, flow_id: String, existing_gist_id: Option<String>)?;
        ok(cmd::cloud_sync::poll_github_gist_device_flow(a.flow_id, a.existing_gist_id).await)
    });

    h!("cancel_github_gist_device_flow" => |_app, args| async move {
        let a = parse_args!(args, flow_id: String)?;
        ok(cmd::cloud_sync::cancel_github_gist_device_flow(a.flow_id).await)
    });

    h!("detect_codex_cli" => |app, _args| async move {
        ok(cmd::ai::detect_codex_cli(app.clone()).await)
    });

    h!("get_codex_account_status" => |app, _args| async move {
        ok(cmd::ai::get_codex_account_status(app.clone()).await)
    });

    h!("detect_claude_code_cli" => |app, _args| async move {
        ok(cmd::ai::detect_claude_code_cli(app.clone()).await)
    });

    h!("get_claude_code_account_status" => |app, _args| async move {
        ok(cmd::ai::get_claude_code_account_status(app.clone()).await)
    });

    h!("list_ai_model_names" => |app, _args| async move {
        ok(cmd::ai::list_ai_model_names(app.clone()).await)
    });

    h!("refresh_ai_model_settings" => |app, args| async move {
        let a = parse_args!(args, ai_settings: crate::config::AiSettings)?;
        ok(cmd::ai::refresh_ai_model_settings(app.clone(), a.ai_settings).await)
    });

    h!("logout_codex" => |app, _args| async move {
        ok(cmd::ai::logout_codex(app.clone()).await)
    });

    h!("get_ai_sessions" => |app, _args| async move {
        ok(cmd::ai::get_ai_sessions(app.clone()))
    });

    h!("get_ai_messages" => |app, args| async move {
        let a = parse_args!(args, session_id: String)?;
        ok(cmd::ai::get_ai_messages(app.clone(), a.session_id))
    });

    h!("clear_ai_history" => |app, _args| async move {
        ok(cmd::ai::clear_ai_history(app.clone()))
    });

    h!("append_ai_audit" => |app, args| async move {
        let a = parse_args!(args, request: crate::core::ai::AppendAiAuditRequest)?;
        ok(cmd::ai::append_ai_audit(app.clone(), a.request))
    });

    h!("get_ai_audit_logs" => |app, _args| async move {
        ok(cmd::ai::get_ai_audit_logs(app.clone(), None))
    });

    Arc::new(handlers)
}

async fn rpc_endpoint(
    axum::extract::State(state): axum::extract::State<ServerState>,
    Json(request): Json<RpcRequest>,
) -> Response {
    let Some(handler) = state.registry.get(request.cmd.as_str()) else {
        return Json(json!({
            "ok": false,
            "error": format!("Unknown command: {}", request.cmd),
        }))
        .into_response();
    };

    let app = state.app.clone();
    let result = handler(&app, request.args).await;
    match result {
        Ok(data) => Json(json!({ "ok": true, "data": data })).into_response(),
        Err(error) => Json(json!({ "ok": false, "error": error })).into_response(),
    }
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RpcRequest {
    cmd: String,
    #[serde(default)]
    args: Value,
}

async fn health() -> Response {
    Json(json!({
        "ok": true,
        "name": "NyaTerm",
        "version": env!("CARGO_PKG_VERSION"),
    }))
    .into_response()
}

pub(crate) async fn serve(
    state: ServerState,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use axum::middleware;
    use axum::routing::{get, post};

    let protected = Router::new()
        .route("/api/rpc", post(rpc_endpoint))
        .route("/api/ws", get(super::ws::ws_endpoint))
        .route("/api/sftp/staging", post(super::files::stage_upload))
        .route("/api/sftp/staging/{name}", get(super::files::fetch_staged))
        .route(
            "/api/sftp/temp/{name}",
            get(super::files::fetch_temp_download),
        )
        .layer(middleware::from_fn_with_state(
            state.clone(),
            super::auth::auth_middleware,
        ))
        .with_state(state.clone());

    let public = Router::new().route("/api/health", get(health));

    let router = public
        .merge(protected)
        .fallback(super::static_files::fallback)
        .with_state(state.clone())
        // Bearer-token auth carries no ambient credentials, so a permissive
        // CORS policy is safe and enables hosting the frontend on another
        // origin (e.g. a CDN) if desired.
        .layer(tower_http::cors::CorsLayer::permissive());

    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], state.port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, router).await?;
    Ok(())
}
