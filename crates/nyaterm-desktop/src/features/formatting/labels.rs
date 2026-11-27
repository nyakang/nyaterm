use gpui::rgb;
use nyaterm_core::{CloudSyncError, CloudSyncSettings, TunnelConfig};
pub(in crate::features) use nyaterm_remote_monitor_gpui::formatting::docker_compose_project_key;
pub(in crate::features) use nyaterm_remote_monitor_gpui::formatting::docker_state_color;
pub(in crate::features) use nyaterm_remote_monitor_gpui::formatting::format_uptime;
use nyaterm_transport::{SessionKind, SshTunnelMode, TelnetEnterMode};
pub(in crate::features) use nyaterm_ui::presentation::compact_id;

use crate::theme::ThemePalette;

pub(in crate::features) fn format_rate(bytes_per_sec: f64) -> String {
    if bytes_per_sec >= 1024. * 1024. {
        format!("{:.1} MiB/s", bytes_per_sec / 1024. / 1024.)
    } else if bytes_per_sec >= 1024. {
        format!("{:.1} KiB/s", bytes_per_sec / 1024.)
    } else {
        format!("{bytes_per_sec:.0} B/s")
    }
}

pub(in crate::features) fn session_kind_label(kind: SessionKind) -> std::borrow::Cow<'static, str> {
    match kind {
        SessionKind::LocalPty => "local".into(),
        SessionKind::Ssh => "ssh".into(),
        SessionKind::Mosh => "mosh".into(),
        SessionKind::Telnet => "telnet".into(),
        SessionKind::RawTcp => "raw tcp".into(),
        SessionKind::Serial => "serial".into(),
        SessionKind::Rdp => "rdp".into(),
        SessionKind::Vnc => "vnc".into(),
        SessionKind::Spice => "spice".into(),
        SessionKind::AdbShell => rust_i18n::t!("android.shell"),
        SessionKind::AdbFiles => rust_i18n::t!("android.files"),
        SessionKind::AdbScreen => rust_i18n::t!("android.screen"),
        SessionKind::Ftp => "FTP".into(),
    }
}

pub(in crate::features) fn cloud_sync_history_status(error: &CloudSyncError) -> &'static str {
    match error {
        CloudSyncError::Conflict(_) => "conflict",
        _ => "failed",
    }
}

pub(in crate::features) fn configured_cloud_sync_provider(settings: &CloudSyncSettings) -> String {
    let provider = settings.provider.trim();
    if provider.is_empty() {
        "local_directory".to_string()
    } else {
        provider.to_string()
    }
}

pub(in crate::features) fn none_if_blank(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

/// The last `max_lines` of a raw output tail as plain text for AI and MCP.
///
/// Control sequences are dropped and a carriage return keeps only what was
/// written after it, so progress bars read as their final state.
pub(in crate::features) fn recent_terminal_output(output: &str, max_lines: usize) -> String {
    let plain = nyaterm_core::strip_terminal_control_sequences(output);
    let lines = plain
        .lines()
        .map(|line| {
            line.rsplit('\r')
                .next()
                .unwrap_or_default()
                .chars()
                .filter(|ch| *ch == '\t' || !ch.is_control())
                .collect::<String>()
        })
        .collect::<Vec<_>>();
    let start = lines.len().saturating_sub(max_lines);
    lines[start..].join("\n")
}

pub(in crate::features) fn format_cloud_provider(provider: &str) -> String {
    match provider.trim() {
        "" => "Unknown".to_string(),
        "local_directory" => "Local directory".to_string(),
        "webdav" => "WebDAV".to_string(),
        "s3" => "S3".to_string(),
        "gitee_snippet" => "Gitee snippet".to_string(),
        "github_gist" => "GitHub gist".to_string(),
        "aliyun_drive" => "Aliyun Drive".to_string(),
        "google_drive" => "Google Drive".to_string(),
        "onedrive" => "OneDrive".to_string(),
        other => other.to_string(),
    }
}

pub(in crate::features) fn format_history_timestamp_ms(timestamp_ms: u64) -> String {
    if timestamp_ms == 0 {
        return "never".to_string();
    }
    // Timestamps are UTC epoch milliseconds; render them as local wall-clock like the
    // terminal gutter formatter, otherwise sync rows display UTC and shift by the
    // machine's UTC offset.
    let secs = (timestamp_ms / 1000) as i64;
    let offset = time::UtcOffset::current_local_offset().unwrap_or(time::UtcOffset::UTC);
    let datetime = time::OffsetDateTime::from_unix_timestamp(secs)
        .map(|value| value.to_offset(offset))
        .unwrap_or_else(|_| time::OffsetDateTime::UNIX_EPOCH.to_offset(offset));
    format!(
        "{:02}:{:02}:{:02}",
        datetime.hour(),
        datetime.minute(),
        datetime.second()
    )
}

/// AiMessage.created_at is RFC3339 UTC; display local wall-clock like
/// `format_history_timestamp_ms`, keeping the date for messages older than today.
pub(in crate::features) fn format_ai_message_time(created_at: &str) -> Option<String> {
    let parsed = time::OffsetDateTime::parse(
        created_at.trim(),
        &time::format_description::well_known::Rfc3339,
    )
    .ok()?;
    let offset = time::UtcOffset::current_local_offset().unwrap_or(time::UtcOffset::UTC);
    let datetime = parsed.to_offset(offset);
    let date = datetime.date();
    let today = time::OffsetDateTime::now_utc().to_offset(offset).date();
    let clock = format!("{:02}:{:02}", datetime.hour(), datetime.minute());
    Some(if date == today {
        clock
    } else if date.year() == today.year() {
        format!("{:02}-{:02} {clock}", date.month() as u8, date.day())
    } else {
        format!(
            "{:04}-{:02}-{:02} {clock}",
            date.year(),
            date.month() as u8,
            date.day()
        )
    })
}

pub(in crate::features) fn format_duration_ms(duration_ms: Option<u64>) -> Option<String> {
    let value = duration_ms?;
    if value < 1000 {
        Some(format!("{value} ms"))
    } else if value < 60_000 {
        Some(format!("{:.1} s", value as f64 / 1000.0))
    } else {
        let minutes = value / 60_000;
        let seconds = (value % 60_000) as f64 / 1000.0;
        Some(format!("{minutes}m {seconds:.0}s"))
    }
}

pub(in crate::features) fn cloud_sync_status_dot_color(
    palette: ThemePalette,
    status: &str,
) -> gpui::Rgba {
    match status {
        "running" => rgb(palette.link),
        "success" => rgb(palette.success),
        "failed" => rgb(palette.danger),
        "conflict" => rgb(palette.warning),
        "disabled" => rgb(palette.text_dimmed),
        _ => rgb(palette.text_muted),
    }
}

pub(in crate::features) fn normalize_startup_command(value: &str) -> String {
    let mut command = value.trim().replace("\r\n", "\n").replace('\r', "\n");
    if !command.ends_with('\n') {
        command.push('\n');
    }
    command
}

pub(in crate::features) fn short_id(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

pub(in crate::features) fn status_label(status: &str) -> &'static str {
    if status.starts_with("running") {
        "session running"
    } else if status.contains("failed") || status.contains("error") {
        "session attention"
    } else {
        "session ready"
    }
}

pub(in crate::features) fn non_empty_string(value: String) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

pub(in crate::features) fn parse_telnet_enter_mode(value: &str) -> TelnetEnterMode {
    match value {
        "crlf" => TelnetEnterMode::Crlf,
        "lf" => TelnetEnterMode::Lf,
        _ => TelnetEnterMode::Cr,
    }
}

pub(in crate::features) fn download_file_name_from_remote_path(remote_path: &str) -> String {
    remote_path
        .trim()
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty() && *name != ".")
        .unwrap_or("nyaterm-download.bin")
        .to_string()
}

pub(in crate::features) fn tunnel_mode(tunnel: &TunnelConfig) -> Option<SshTunnelMode> {
    match tunnel.tunnel_type.as_str() {
        "local" => Some(SshTunnelMode::Local),
        "remote" => Some(SshTunnelMode::Remote),
        "dynamic" => Some(SshTunnelMode::Dynamic),
        _ => None,
    }
}

pub(in crate::features) fn tunnel_name(tunnel: &TunnelConfig) -> String {
    if tunnel.name.trim().is_empty() {
        tunnel.id.clone()
    } else {
        tunnel.name.clone()
    }
}

pub(in crate::features) fn tunnel_endpoint(tunnel: &TunnelConfig, listen: &str) -> String {
    match tunnel.tunnel_type.as_str() {
        "dynamic" => format!("{listen} SOCKS5"),
        "remote" => format!(
            "remote {} -> {}:{}",
            tunnel.listen_port, tunnel.target_host, tunnel.target_port
        ),
        _ => format!("{listen} -> {}:{}", tunnel.target_host, tunnel.target_port),
    }
}

pub(in crate::features) use nyaterm_file_browser_gpui::formatting::format_permissions_octal;

pub(in crate::features) fn trim_terminal_output_to(output: &mut String, max_bytes: usize) {
    if max_bytes == 0 || output.len() <= max_bytes {
        return;
    }
    let drain_to = output
        .char_indices()
        .find_map(|(index, _)| (index >= output.len() - max_bytes).then_some(index))
        .unwrap_or(0);
    output.drain(..drain_to);
}

pub(in crate::features) fn format_last_used_ms(last_used_at_ms: Option<u64>) -> String {
    let Some(ms) = last_used_at_ms.filter(|value| *value > 0) else {
        return "never".to_string();
    };
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(ms);
    if now_ms < ms {
        return "just now".to_string();
    }
    let secs = (now_ms - ms) / 1000;
    if secs < 60 {
        "just now".to_string()
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86_400 {
        format!("{}h ago", secs / 3600)
    } else if secs < 86_400 * 30 {
        format!("{}d ago", secs / 86_400)
    } else {
        format!("{}mo ago", secs / (86_400 * 30))
    }
}

#[cfg(test)]
mod tests {
    use super::{format_history_timestamp_ms, recent_terminal_output};

    #[test]
    fn recent_output_is_plain_text_tail() {
        let raw = "\x1b]0;title\x07old\r\n\x1b[32mok\x1b[0m\r\n10%\r50%\r100%\r\nuser@host:~$ \x07";
        assert_eq!(recent_terminal_output(raw, 3), "ok\n100%\nuser@host:~$ ");
        assert_eq!(recent_terminal_output(raw, 1), "user@host:~$ ");
    }

    #[test]
    fn history_timestamp_is_local_wall_clock_not_utc() {
        assert_eq!(format_history_timestamp_ms(0), "never");

        let timestamp_ms = 1_600_000_000_000;
        let offset = time::UtcOffset::current_local_offset().unwrap_or(time::UtcOffset::UTC);
        let datetime = time::OffsetDateTime::from_unix_timestamp((timestamp_ms / 1000) as i64)
            .expect("valid timestamp")
            .to_offset(offset);
        let expected = format!(
            "{:02}:{:02}:{:02}",
            datetime.hour(),
            datetime.minute(),
            datetime.second()
        );
        assert_eq!(
            format_history_timestamp_ms(timestamp_ms),
            expected,
            "history timestamps must honor the local UTC offset"
        );
    }
}
