use crate::core::ssh_config::{SshConfig, SshConfigEntry};
use crate::error::AppResult;

/// Lists all concrete host entries from `~/.ssh/config` with resolved ProxyJump chains.
#[tauri::command]
pub fn list_ssh_config_hosts() -> AppResult<Vec<SshConfigEntry>> {
    let config = SshConfig::load_default()?;
    config.to_entries()
}

/// Returns the raw parsed `Host` blocks from `~/.ssh/config`.
#[tauri::command]
pub fn get_ssh_config() -> AppResult<SshConfig> {
    SshConfig::load_default()
}

/// Resolves a single host alias into a fully-resolved entry with hops.
#[tauri::command]
pub fn resolve_ssh_host(alias: String) -> AppResult<SshConfigEntry> {
    let config = SshConfig::load_default()?;
    config.resolve(&alias)
}

/// Imports all concrete SSH config hosts as saved connections.
/// Returns the number of connections imported (skips duplicates by name).
#[tauri::command]
pub fn import_ssh_config_hosts(app: tauri::AppHandle) -> AppResult<usize> {
    let count = crate::core::ssh_config::import_ssh_config_connections(&app)?;
    tauri::async_runtime::spawn(async move {
        crate::core::cloud_sync::notify_config_changed(&app).await;
    });
    Ok(count)
}
