use gpui::Pixels;
use std::path::Path;

use crate::models::TransferBrowserColumnWidths;

pub(in crate::features::pages::transfers) fn remote_file_name(path: &str) -> String {
    if looks_like_native_path(path) {
        return Path::new(path)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string());
    }
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(path)
        .to_string()
}

pub(in crate::features::pages::transfers) fn remote_parent_path(path: &str) -> String {
    if looks_like_native_path(path) {
        return Path::new(path)
            .parent()
            .map(|parent| parent.to_string_lossy().into_owned())
            .filter(|parent| !parent.is_empty())
            .unwrap_or_else(|| path.to_string());
    }
    let path = path.trim_end_matches('/');
    match path.rfind('/') {
        Some(0) => "/".to_string(),
        Some(index) => path[..index].to_string(),
        None => ".".to_string(),
    }
}

pub(in crate::features::pages::transfers) fn remote_child_path(
    parent: &str,
    child_name: &str,
) -> String {
    if looks_like_native_path(parent) {
        return Path::new(parent)
            .join(child_name)
            .to_string_lossy()
            .into_owned();
    }
    let trimmed = parent.trim_end_matches('/');
    match trimmed {
        "" if parent.starts_with('/') => format!("/{child_name}"),
        "" | "." => child_name.to_string(),
        parent => format!("{parent}/{child_name}"),
    }
}

pub(in crate::features::pages::transfers) fn normalized_transfer_browser_path(
    path: &str,
) -> String {
    crate::features::transfers::browser_paths::normalized_transfer_browser_path(path)
}

pub(in crate::features::pages::transfers) fn valid_remote_child_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains('/')
}

fn looks_like_native_path(path: &str) -> bool {
    path.contains('\\')
        || path.as_bytes().get(1) == Some(&b':')
        || (path.starts_with("//") && !path.starts_with("///"))
}

pub(in crate::features::pages::transfers) fn transfer_browser_path_is_root(path: &str) -> bool {
    crate::features::transfers::browser_paths::transfer_browser_path_is_root(path)
}

#[derive(Debug, Clone, Copy)]
pub(in crate::features::pages::transfers) enum TransferPathPart {
    Full,
    Name,
    Directory,
}

impl TransferPathPart {
    pub(in crate::features::pages::transfers) fn label(self) -> &'static str {
        match self {
            Self::Full => "path",
            Self::Name => "name",
            Self::Directory => "directory",
        }
    }
}

pub(in crate::features::pages::transfers) fn transfer_browser_table_width(
    widths: TransferBrowserColumnWidths,
) -> Pixels {
    widths.name + widths.modified + widths.size + widths.permissions + widths.owner + widths.group
}

pub(in crate::features::pages::transfers) fn transfer_path_part_value(
    path: &str,
    part: TransferPathPart,
) -> String {
    match part {
        TransferPathPart::Full => path.to_string(),
        TransferPathPart::Name => remote_file_name(path),
        TransferPathPart::Directory => remote_parent_path(path),
    }
}

pub(in crate::features) fn format_sftp_modified(value: Option<u32>) -> String {
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

#[cfg(test)]
mod tests {
    use super::remote_child_path;
    #[cfg(windows)]
    use super::transfer_browser_path_is_root;

    #[test]
    fn remote_child_path_preserves_filesystem_root() {
        assert_eq!(remote_child_path("/", "var"), "/var");
        assert_eq!(remote_child_path("///", "var"), "/var");
        assert_eq!(remote_child_path("/var/", "log"), "/var/log");
        assert_eq!(remote_child_path(".", "relative"), "relative");
    }

    #[cfg(windows)]
    #[test]
    fn windows_drive_and_unc_roots_are_terminal() {
        assert!(transfer_browser_path_is_root(r"C:\"));
        assert!(transfer_browser_path_is_root(r"\\server\share\"));
        assert!(!transfer_browser_path_is_root(r"C:\tmp"));
    }
}
