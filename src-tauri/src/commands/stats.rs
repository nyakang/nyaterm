use crate::error::{AppError, AppResult};
use crate::session::SessionManager;
use crate::ssh::SshHandler;
use russh::client;
use std::sync::Arc;

#[derive(serde::Serialize)]
pub struct RemoteStats {
    pub cpu_percent: f64,
    pub mem_used_mb: u64,
    pub mem_total_mb: u64,
}

#[tauri::command]
pub async fn get_remote_stats(
    state: tauri::State<'_, Arc<SessionManager>>,
    session_id: String,
) -> AppResult<RemoteStats> {
    use russh::ChannelMsg;

    let handle = {
        let sessions = state.sessions.lock().await;
        let session = sessions
            .get(&session_id)
            .ok_or_else(|| AppError::SessionNotFound(format!("Session '{}' not found", session_id)))?;

        session
            .ssh_handle
            .as_ref()
            .ok_or_else(|| AppError::Config("Not an SSH session".to_string()))?
            .clone()
            .downcast::<client::Handle<SshHandler>>()
            .map_err(|_| AppError::Config("Failed to get SSH handle".to_string()))?
    };

    let mut channel = handle
        .channel_open_session()
        .await
        .map_err(|e| AppError::Channel(format!("Failed to open channel: {}", e)))?;

    let cmd = "awk '/^cpu /{idle=$5;total=0;for(i=2;i<=NF;i++)total+=$i;printf \"%.1f\",(1-idle/total)*100}' /proc/stat; \
               awk '/MemTotal/{t=$2}/MemAvailable/{a=$2}END{printf \" %d %d\",(t-a)/1024,t/1024}' /proc/meminfo";

    channel
        .exec(true, cmd)
        .await
        .map_err(|e| AppError::Channel(format!("Failed to execute stats command: {}", e)))?;

    let mut output = String::new();
    loop {
        match channel.wait().await {
            Some(ChannelMsg::Data { ref data }) => {
                output.push_str(&String::from_utf8_lossy(data));
            }
            Some(ChannelMsg::Eof) | None => break,
            _ => {}
        }
    }

    let parts: Vec<&str> = output.trim().split_whitespace().collect();
    if parts.len() < 3 {
        return Err(AppError::Config(format!(
            "Unexpected stats output: '{}'",
            output.trim()
        )));
    }

    Ok(RemoteStats {
        cpu_percent: parts[0].parse::<f64>().unwrap_or(0.0),
        mem_used_mb: parts[1].parse::<u64>().unwrap_or(0),
        mem_total_mb: parts[2].parse::<u64>().unwrap_or(0),
    })
}

#[tauri::command]
pub fn get_system_fonts() -> Vec<String> {
    use font_kit::source::SystemSource;
    if let Ok(mut families) = SystemSource::new().all_families() {
        families.sort();
        families.dedup();
        return families;
    }
    Vec::new()
}
