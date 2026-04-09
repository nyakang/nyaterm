use crate::config;
use crate::error::AppResult;
use crate::ssh::TunnelManager;
use std::sync::Arc;
use tauri::Manager;

#[tauri::command]
pub async fn get_tunnels(app: tauri::AppHandle) -> AppResult<Vec<config::TunnelConfig>> {
    let mut tunnels = config::load_tunnels(&app)?;
    let tunnel_mgr = app.state::<Arc<TunnelManager>>();
    for tunnel in &mut tunnels {
        tunnel.is_open = tunnel_mgr.is_open(&tunnel.id).await;
    }
    Ok(tunnels)
}

#[tauri::command]
pub async fn save_tunnel(
    app: tauri::AppHandle,
    tunnel_mgr: tauri::State<'_, Arc<TunnelManager>>,
    tunnel: config::TunnelConfig,
) -> AppResult<()> {
    if tunnel_mgr.is_open(&tunnel.id).await {
        tunnel_mgr.close(&tunnel.id).await;
    }
    let mut tunnels = config::load_tunnels(&app)?;
    if let Some(existing) = tunnels.iter_mut().find(|t| t.id == tunnel.id) {
        *existing = tunnel;
    } else {
        tunnels.push(tunnel);
    }
    config::save_tunnels(&app, &tunnels)
}

#[tauri::command]
pub async fn delete_tunnel(
    app: tauri::AppHandle,
    tunnel_mgr: tauri::State<'_, Arc<TunnelManager>>,
    tunnel_id: String,
) -> AppResult<()> {
    if tunnel_mgr.is_open(&tunnel_id).await {
        tunnel_mgr.close(&tunnel_id).await;
    }
    let mut tunnels = config::load_tunnels(&app)?;
    tunnels.retain(|t| t.id != tunnel_id);
    config::save_tunnels(&app, &tunnels)
}

#[tauri::command]
pub async fn open_tunnel(
    app: tauri::AppHandle,
    tunnel_mgr: tauri::State<'_, Arc<TunnelManager>>,
    tunnel_id: String,
) -> AppResult<()> {
    let tunnels = config::load_tunnels(&app)?;
    let tunnel = tunnels.iter().find(|t| t.id == tunnel_id).ok_or_else(|| {
        crate::error::AppError::Config(format!("Tunnel '{}' not found", tunnel_id))
    })?;
    tunnel_mgr.open(tunnel, &app).await
}

#[tauri::command]
pub async fn close_tunnel(
    tunnel_mgr: tauri::State<'_, Arc<TunnelManager>>,
    tunnel_id: String,
) -> AppResult<()> {
    tunnel_mgr.close(&tunnel_id).await;
    Ok(())
}
