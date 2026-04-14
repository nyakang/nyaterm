use serde::Deserialize;

use crate::error::AppResult;

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

#[tauri::command]
pub fn write_log(level: LogLevel, message: String, context: Option<String>) -> AppResult<()> {
    match (level, context.as_deref()) {
        (LogLevel::Debug, Some(context)) => {
            tracing::debug!(source = "frontend", context = %context, "{message}");
        }
        (LogLevel::Debug, None) => {
            tracing::debug!(source = "frontend", "{message}");
        }
        (LogLevel::Info, Some(context)) => {
            tracing::info!(source = "frontend", context = %context, "{message}");
        }
        (LogLevel::Info, None) => {
            tracing::info!(source = "frontend", "{message}");
        }
        (LogLevel::Warn, Some(context)) => {
            tracing::warn!(source = "frontend", context = %context, "{message}");
        }
        (LogLevel::Warn, None) => {
            tracing::warn!(source = "frontend", "{message}");
        }
        (LogLevel::Error, Some(context)) => {
            tracing::error!(source = "frontend", context = %context, "{message}");
        }
        (LogLevel::Error, None) => {
            tracing::error!(source = "frontend", "{message}");
        }
    }

    Ok(())
}
