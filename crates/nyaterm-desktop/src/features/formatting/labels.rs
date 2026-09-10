use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui::rgb;
use nyaterm_core::{
    AppSettingsSummary, CloudSyncError, CloudSyncHistoryEntry, CloudSyncSettings, RiskLevel,
    TunnelConfig,
};
use nyaterm_transport::{SessionKind, SshTunnelMode, TelnetEnterMode, safe_recording_name};

use crate::features::runtime_jobs::AiAgentStepStatus;
use crate::theme::ThemePalette;

pub(in crate::features) fn ai_agent_step_status_style(
    status: AiAgentStepStatus,
) -> (&'static str, u32, u32) {
    match status {
        AiAgentStepStatus::Planning => ("planning", 0x93c5fd, 0x17233a),
        AiAgentStepStatus::Tool => ("tool", 0xc4b5fd, 0x2b2142),
        AiAgentStepStatus::NeedsApproval => ("review", 0xfacc15, 0x3a2f14),
        AiAgentStepStatus::Running => ("running", 0x6ee7b7, 0x12342a),
        AiAgentStepStatus::Completed => ("done", 0x86efac, 0x12301f),
        AiAgentStepStatus::Failed => ("failed", 0xfca5a5, 0x3a1717),
        AiAgentStepStatus::Cancelled => ("cancelled", 0xcbd5e1, 0x273244),
    }
}

pub(in crate::features) fn format_rate(bytes_per_sec: f64) -> String {
    if bytes_per_sec >= 1024. * 1024. {
        format!("{:.1} MiB/s", bytes_per_sec / 1024. / 1024.)
    } else if bytes_per_sec >= 1024. {
        format!("{:.1} KiB/s", bytes_per_sec / 1024.)
    } else {
        format!("{bytes_per_sec:.0} B/s")
    }
}

pub(in crate::features) fn format_uptime(seconds: u64) -> String {
    let days = seconds / 86_400;
    let hours = (seconds % 86_400) / 3_600;
    let minutes = (seconds % 3_600) / 60;
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{minutes}m")
    }
}

pub(in crate::features) fn risk_label(risk: Option<&RiskLevel>) -> &'static str {
    match risk {
        Some(RiskLevel::Low) => "Low",
        Some(RiskLevel::Medium) => "Medium",
        Some(RiskLevel::High) => "High",
        Some(RiskLevel::Critical) => "Critical",
        None => "Unrated",
    }
}

pub(in crate::features) fn recording_file_path(
    settings: &AppSettingsSummary,
    config_dir: &std::path::Path,
    session_name: &str,
) -> PathBuf {
    let base_dir = if settings.recording_path.trim().is_empty() {
        config_dir.join("recordings")
    } else {
        PathBuf::from(settings.recording_path.trim())
    };
    let timestamp_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    base_dir.join(format!(
        "recording-{}-{timestamp_ms}.log",
        safe_recording_name(session_name)
    ))
}

pub(in crate::features) fn docker_state_rank(state: &str) -> u8 {
    match state.trim().to_ascii_lowercase().as_str() {
        "running" => 0,
        "restarting" | "paused" => 1,
        "created" => 2,
        "exited" | "dead" => 3,
        _ => 4,
    }
}

pub(in crate::features) fn docker_state_label(state: &str) -> &'static str {
    match state.trim().to_ascii_lowercase().as_str() {
        "running" => "running",
        "restarting" => "restart",
        "paused" => "paused",
        "created" => "created",
        "exited" => "exited",
        "dead" => "dead",
        _ => "unknown",
    }
}

pub(in crate::features) fn docker_state_color(palette: ThemePalette, state: &str) -> gpui::Hsla {
    match state.trim().to_ascii_lowercase().as_str() {
        "running" => rgb(palette.success).into(),
        "restarting" | "paused" => rgb(palette.warning).into(),
        "created" => rgb(palette.link).into(),
        "exited" | "dead" => rgb(palette.danger).into(),
        _ => rgb(palette.text_muted).into(),
    }
}

pub(in crate::features) fn docker_compose_project_key(
    project_name: &str,
    config_files: Option<&str>,
) -> String {
    format!(
        "{}\n{}",
        project_name.trim(),
        config_files.unwrap_or_default().trim()
    )
}

pub(in crate::features) fn session_kind_label(kind: SessionKind) -> &'static str {
    match kind {
        SessionKind::LocalPty => "local",
        SessionKind::Ssh => "ssh",
        SessionKind::Telnet => "telnet",
        SessionKind::RawTcp => "raw tcp",
        SessionKind::Serial => "serial",
        SessionKind::Rdp => "rdp",
        SessionKind::Vnc => "vnc",
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

pub(in crate::features) fn recent_terminal_output(output: &str, max_lines: usize) -> String {
    let lines = output.lines().collect::<Vec<_>>();
    let start = lines.len().saturating_sub(max_lines);
    lines[start..].join("\n")
}

pub(in crate::features) fn compact_id(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.chars().count() <= 12 {
        trimmed.to_string()
    } else {
        let prefix: String = trimmed.chars().take(8).collect();
        let suffix: String = trimmed
            .chars()
            .rev()
            .take(4)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        format!("{prefix}..{suffix}")
    }
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

#[derive(Debug, Clone)]
enum TerminalTimestampToken {
    Year4,
    Year2,
    Month2,
    Month,
    Day2,
    Day,
    Hour2,
    Hour,
    Minute2,
    Minute,
    Second2,
    Second,
    Millis3,
    Millis2,
    Millis1,
    Literal(char),
}

#[derive(Debug, Clone)]
pub(in crate::features) struct TerminalTimestampFormatter {
    tokens: Vec<TerminalTimestampToken>,
    width_chars: usize,
    offset: time::UtcOffset,
}

impl TerminalTimestampFormatter {
    pub(in crate::features) fn new(format: &str) -> Self {
        let format = normalized_terminal_timestamp_format(format);
        let tokens = parse_terminal_timestamp_tokens(format);
        let width_chars = terminal_timestamp_tokens_width(&tokens);
        Self {
            tokens,
            width_chars,
            offset: time::UtcOffset::current_local_offset().unwrap_or(time::UtcOffset::UTC),
        }
    }

    pub(in crate::features) fn width_chars(&self) -> usize {
        self.width_chars
    }

    pub(in crate::features) fn format(&self, timestamp_ms: u64) -> String {
        let secs = (timestamp_ms / 1000) as i64;
        let millis = timestamp_ms % 1000;
        let datetime = time::OffsetDateTime::from_unix_timestamp(secs)
            .ok()
            .map(|datetime| datetime.to_offset(self.offset));
        let Some(datetime) = datetime else {
            return " ".repeat(self.width_chars);
        };

        render_terminal_timestamp_tokens(&self.tokens, datetime, millis)
    }
}

pub(in crate::features) fn terminal_timestamp_format_width_chars(format: &str) -> usize {
    let tokens = parse_terminal_timestamp_tokens(normalized_terminal_timestamp_format(format));
    terminal_timestamp_tokens_width(&tokens)
}

fn normalized_terminal_timestamp_format(format: &str) -> &str {
    let trimmed = format.trim();
    if trimmed.is_empty() {
        nyaterm_core::DEFAULT_TERMINAL_TIMESTAMP_FORMAT
    } else {
        trimmed
    }
}

fn parse_terminal_timestamp_tokens(mut input: &str) -> Vec<TerminalTimestampToken> {
    let mut tokens = Vec::new();
    while !input.is_empty() {
        let (token, consumed) = if input.starts_with("YYYY") {
            (TerminalTimestampToken::Year4, 4)
        } else if input.starts_with("SSS") {
            (TerminalTimestampToken::Millis3, 3)
        } else if input.starts_with("YY") {
            (TerminalTimestampToken::Year2, 2)
        } else if input.starts_with("MM") {
            (TerminalTimestampToken::Month2, 2)
        } else if input.starts_with("DD") {
            (TerminalTimestampToken::Day2, 2)
        } else if input.starts_with("HH") {
            (TerminalTimestampToken::Hour2, 2)
        } else if input.starts_with("mm") {
            (TerminalTimestampToken::Minute2, 2)
        } else if input.starts_with("ss") {
            (TerminalTimestampToken::Second2, 2)
        } else if input.starts_with("SS") {
            (TerminalTimestampToken::Millis2, 2)
        } else {
            let ch = input.chars().next().expect("non-empty timestamp format");
            let token = match ch {
                'M' => TerminalTimestampToken::Month,
                'D' => TerminalTimestampToken::Day,
                'H' => TerminalTimestampToken::Hour,
                'm' => TerminalTimestampToken::Minute,
                's' => TerminalTimestampToken::Second,
                'S' => TerminalTimestampToken::Millis1,
                _ => TerminalTimestampToken::Literal(ch),
            };
            (token, ch.len_utf8())
        };
        tokens.push(token);
        input = &input[consumed..];
    }
    tokens
}

fn terminal_timestamp_tokens_width(tokens: &[TerminalTimestampToken]) -> usize {
    tokens
        .iter()
        .map(|token| match token {
            TerminalTimestampToken::Year4 => 4,
            TerminalTimestampToken::Year2
            | TerminalTimestampToken::Month2
            | TerminalTimestampToken::Month
            | TerminalTimestampToken::Day2
            | TerminalTimestampToken::Day
            | TerminalTimestampToken::Hour2
            | TerminalTimestampToken::Hour
            | TerminalTimestampToken::Minute2
            | TerminalTimestampToken::Minute
            | TerminalTimestampToken::Second2
            | TerminalTimestampToken::Second
            | TerminalTimestampToken::Millis2 => 2,
            TerminalTimestampToken::Millis3 => 3,
            TerminalTimestampToken::Millis1 | TerminalTimestampToken::Literal(_) => 1,
        })
        .sum::<usize>()
        .clamp(1, 64)
}

fn render_terminal_timestamp_tokens(
    tokens: &[TerminalTimestampToken],
    datetime: time::OffsetDateTime,
    millis: u64,
) -> String {
    let mut out = String::with_capacity(terminal_timestamp_tokens_width(tokens));
    for token in tokens {
        match token {
            TerminalTimestampToken::Year4 => out.push_str(&format!("{:04}", datetime.year())),
            TerminalTimestampToken::Year2 => {
                out.push_str(&format!("{:02}", datetime.year().rem_euclid(100)))
            }
            TerminalTimestampToken::Month2 => {
                out.push_str(&format!("{:02}", datetime.month() as u8))
            }
            TerminalTimestampToken::Month => out.push_str(&(datetime.month() as u8).to_string()),
            TerminalTimestampToken::Day2 => out.push_str(&format!("{:02}", datetime.day())),
            TerminalTimestampToken::Day => out.push_str(&datetime.day().to_string()),
            TerminalTimestampToken::Hour2 => out.push_str(&format!("{:02}", datetime.hour())),
            TerminalTimestampToken::Hour => out.push_str(&datetime.hour().to_string()),
            TerminalTimestampToken::Minute2 => out.push_str(&format!("{:02}", datetime.minute())),
            TerminalTimestampToken::Minute => out.push_str(&datetime.minute().to_string()),
            TerminalTimestampToken::Second2 => out.push_str(&format!("{:02}", datetime.second())),
            TerminalTimestampToken::Second => out.push_str(&datetime.second().to_string()),
            TerminalTimestampToken::Millis3 => out.push_str(&format!("{millis:03}")),
            TerminalTimestampToken::Millis2 => out.push_str(&format!("{:02}", millis / 10)),
            TerminalTimestampToken::Millis1 => out.push_str(&(millis / 100).to_string()),
            TerminalTimestampToken::Literal(ch) => out.push(*ch),
        }
    }
    out
}

pub(in crate::features) struct TerminalGutterLabels {
    pub timestamp: String,
    pub line_number: String,
}

pub(in crate::features) fn terminal_gutter_labels(
    row: Option<&nyaterm_terminal::TerminalSnapshotRow>,
    absolute_line_number: usize,
    show_timestamps: bool,
    show_line_numbers: bool,
    line_number_digits: usize,
    timestamp_formatter: &TerminalTimestampFormatter,
) -> TerminalGutterLabels {
    let wrapped = row.is_some_and(|row| row.wrapped);
    let timestamp = if show_timestamps && !wrapped {
        row.and_then(|row| row.timestamp_ms)
            .map(|timestamp| timestamp_formatter.format(timestamp))
            .unwrap_or_else(|| " ".repeat(timestamp_formatter.width_chars()))
    } else {
        String::new()
    };
    let line_number = if show_line_numbers && !wrapped {
        format!("{absolute_line_number:>line_number_digits$}")
    } else {
        String::new()
    };
    TerminalGutterLabels {
        timestamp,
        line_number,
    }
}

pub(in crate::features) fn format_history_timestamp_ms(timestamp_ms: u64) -> String {
    if timestamp_ms == 0 {
        return "never".to_string();
    }
    let secs = (timestamp_ms / 1000) as i64;
    let hours = ((secs % 86_400) / 3_600).rem_euclid(24);
    let minutes = ((secs % 3_600) / 60).rem_euclid(60);
    let seconds = (secs % 60).rem_euclid(60);
    // Compact wall-clock style without pulling chrono; good enough for panel density.
    format!("{hours:02}:{minutes:02}:{seconds:02}")
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

pub(in crate::features) fn cloud_sync_status_text_color(
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

pub(in crate::features) fn cloud_sync_kind_text_color(
    palette: ThemePalette,
    kind: &str,
) -> gpui::Rgba {
    match kind {
        "sync" => rgb(palette.link),
        "backup" => rgb(palette.link),
        _ => rgb(palette.text_muted),
    }
}

pub(in crate::features) fn cloud_sync_history_summary(entry: &CloudSyncHistoryEntry) -> String {
    let normalized = entry
        .message
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if normalized.is_empty() {
        return format!("{} · {}", entry.kind, entry.status);
    }
    if !normalized.contains('\n') && normalized.chars().count() <= 110 {
        return normalized;
    }
    // Prefer first sentence when short enough.
    let first = normalized
        .split(['.', '!', '?'])
        .next()
        .unwrap_or("")
        .trim();
    if !first.is_empty() && first.chars().count() <= 110 {
        let end = first.chars().count();
        let punct = normalized.chars().nth(end).unwrap_or('.');
        if matches!(punct, '.' | '!' | '?') {
            return format!("{first}{punct}");
        }
        return first.to_string();
    }
    format!("{} · {}", entry.kind, entry.status)
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

pub(in crate::features) fn format_permissions_octal(mode: u32) -> String {
    format!("{:04o}", mode & 0o7777)
}

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
    use nyaterm_terminal::TerminalScreen;

    use super::{
        TerminalTimestampFormatter, terminal_gutter_labels, terminal_timestamp_format_width_chars,
    };

    #[test]
    fn terminal_timestamp_milliseconds_follow_format_tokens_only() {
        let seconds = TerminalTimestampFormatter::new("HH:mm:ss");
        let millis = TerminalTimestampFormatter::new("HH:mm:ss.SSS");

        assert_eq!(seconds.width_chars(), 8);
        assert_eq!(millis.width_chars(), 12);
        assert!(!seconds.format(123).ends_with(".123"));
        assert!(millis.format(123).ends_with(".123"));
        assert_eq!(terminal_timestamp_format_width_chars("H:m:s.S"), 10);
    }

    #[test]
    fn gutter_labels_are_independent_of_cursor_and_reserve_missing_timestamp_width() {
        let mut terminal = TerminalScreen::new(10, 3);
        terminal.advance(b"first\r\nsecond\r\nthird\x1b[2A");
        let snapshot = terminal.snapshot();
        assert_eq!(snapshot.cursor.row, 0);
        let formatter = TerminalTimestampFormatter::new("HH:mm:ss.SSS");

        let labels = terminal_gutter_labels(snapshot.row(2), 3, true, true, 2, &formatter);

        assert_eq!(labels.line_number, " 3");
        assert_eq!(labels.timestamp.len(), formatter.width_chars());
    }

    #[test]
    fn wrapped_continuation_has_no_duplicate_gutter_labels() {
        let mut terminal = TerminalScreen::new(4, 2);
        terminal.advance(b"abcdefgh");
        let snapshot = terminal.snapshot();
        assert!(snapshot.row(1).is_some_and(|row| row.wrapped));
        let formatter = TerminalTimestampFormatter::new("HH:mm:ss");

        let labels = terminal_gutter_labels(snapshot.row(1), 2, true, true, 1, &formatter);

        assert!(labels.timestamp.is_empty());
        assert!(labels.line_number.is_empty());
    }
}
