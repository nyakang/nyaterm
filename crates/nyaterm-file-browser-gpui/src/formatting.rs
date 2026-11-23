pub fn format_browser_file_size(size: Option<u64>) -> String {
    match size {
        None | Some(0) => "-".to_string(),
        Some(size) if size < 1024 => format!("{size} B"),
        Some(size) => {
            const UNITS: [&str; 6] = ["KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];
            let mut unit_index = 0;
            let mut divisor = 1024_u64;
            while size / divisor >= 1024 && unit_index < UNITS.len() - 1 {
                divisor *= 1024;
                unit_index += 1;
            }
            format!("{:.1} {}", size as f64 / divisor as f64, UNITS[unit_index])
        }
    }
}
pub fn format_sftp_modified(value: Option<u32>) -> String {
    let Some(seconds) = value.filter(|seconds| *seconds > 0) else {
        return "-".to_string();
    };
    let Ok(timestamp) = time::OffsetDateTime::from_unix_timestamp(i64::from(seconds)) else {
        return "-".to_string();
    };
    let offset = time::UtcOffset::current_local_offset().unwrap_or(time::UtcOffset::UTC);
    let format = time::macros::format_description!("[year]/[month]/[day] [hour]:[minute]");
    timestamp
        .to_offset(offset)
        .format(&format)
        .unwrap_or_else(|_| "-".to_string())
}
pub fn format_permissions_octal(mode: u32) -> String {
    format!("{:04o}", mode & 0o7777)
}
