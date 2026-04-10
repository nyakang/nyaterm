//! Tauri app entry point: state construction, plugin registration, and command routing.

mod app;
mod cmd;
mod config;
mod core;
mod error;
mod utils;

use std::sync::Arc;

use crate::core::ssh::{PendingAuthManager, TunnelManager};
use crate::core::{RecordingManager, SessionManager};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let session_manager = Arc::new(SessionManager::new());
    let tunnel_manager = Arc::new(TunnelManager::new());
    let recording_manager = Arc::new(RecordingManager::new());
    let pending_auth_manager = Arc::new(PendingAuthManager::new());

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(session_manager.clone())
        .manage(tunnel_manager.clone())
        .manage(recording_manager.clone())
        .manage(pending_auth_manager.clone())
        .setup(move |a| app::setup(a, session_manager))
        .on_window_event(app::on_window_event)
        .invoke_handler(tauri::generate_handler![
            cmd::settings::get_system_fonts,
            cmd::session::create_ssh_session,
            cmd::session::create_local_session,
            cmd::session::write_to_session,
            cmd::session::resize_session,
            cmd::session::attach_session,
            cmd::session::close_session,
            cmd::session::list_sessions,
            cmd::session::add_command_history,
            cmd::session::get_command_history,
            cmd::session::fuzzy_search_history,
            cmd::session::fuzzy_search_commands,
            cmd::session::start_recording,
            cmd::session::stop_recording,
            cmd::session::is_recording,
            cmd::session::submit_otp_response,
            cmd::session::cancel_otp_request,
            cmd::sftp::get_home_dir,
            cmd::sftp::list_remote_dir,
            cmd::sftp::delete_remote_file,
            cmd::sftp::rename_remote_file,
            cmd::sftp::download_remote_file,
            cmd::sftp::upload_local_file,
            cmd::sftp::get_file_properties,
            cmd::sftp::create_remote_file,
            cmd::sftp::create_remote_dir,
            cmd::sftp::create_remote_symlink,
            cmd::sftp::chmod_remote_file,
            cmd::sftp::download_remote_directory,
            cmd::sftp::upload_local_directory,
            cmd::connection::get_saved_connections,
            cmd::connection::save_connection,
            cmd::connection::delete_connection,
            cmd::connection::reorder_items,
            cmd::connection::get_ssh_keys,
            cmd::connection::get_ssh_key_passphrase,
            cmd::connection::save_ssh_key,
            cmd::connection::delete_ssh_key,
            cmd::connection::get_groups,
            cmd::connection::save_group,
            cmd::connection::delete_group,
            cmd::connection::clear_all_connections,
            cmd::connection::get_quick_commands,
            cmd::connection::save_quick_commands,
            cmd::connection::get_saved_passwords,
            cmd::connection::get_saved_password_value,
            cmd::connection::save_password,
            cmd::connection::delete_password,
            cmd::settings::get_app_settings,
            cmd::settings::save_app_settings,
            cmd::settings::verify_lock_password,
            cmd::watcher::start_file_watch,
            cmd::watcher::stop_file_watch,
            cmd::translate::translate_text,
            cmd::importer::import_sessions,
            cmd::stats::get_remote_stats,
            cmd::stats::get_terminal_cwd,
            cmd::tunnel::get_tunnels,
            cmd::tunnel::save_tunnel,
            cmd::tunnel::delete_tunnel,
            cmd::tunnel::open_tunnel,
            cmd::tunnel::close_tunnel,
            cmd::proxy::get_proxies,
            cmd::proxy::save_proxy,
            cmd::proxy::delete_proxy,
            cmd::proxy::get_proxy_password,
            cmd::otp::get_otp_entries,
            cmd::otp::get_otp_secret_value,
            cmd::otp::save_otp_entry,
            cmd::otp::delete_otp_entry,
            cmd::otp::generate_otp_code,
            cmd::otp::import_otp_from_qr,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
