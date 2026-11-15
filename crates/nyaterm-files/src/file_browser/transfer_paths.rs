//! Transfer path policies. Session resolution and directory caches belong to callers.
use std::path::Path;

pub fn upload_local_name(path: &Path, fallback: &str) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or(fallback)
        .to_string()
}

/// Resolve the destination directory for a "Send to" transfer.
///
/// Prefers the target session's own cached browser directory; otherwise falls
/// back to that session's home (or `cwd`). The source session's path must never
/// leak in here, so both fallbacks are explicit target-session inputs.
pub fn send_to_target_directory(
    cached_current_path: Option<&str>,
    target_home_or_cwd: Option<&str>,
) -> String {
    let normalize = |value: &str| -> Option<String> {
        let trimmed = value.trim();
        (!trimmed.is_empty() && trimmed != ".").then(|| trimmed.trim_end_matches('/').to_string())
    };
    cached_current_path
        .and_then(normalize)
        .or_else(|| target_home_or_cwd.and_then(normalize))
        .unwrap_or_else(|| ".".to_string())
}

pub fn remote_parent_path(path: &str) -> String {
    let path = path.trim_end_matches('/');
    match path.rfind('/') {
        Some(0) => "/".to_string(),
        Some(index) => path[..index].to_string(),
        None => ".".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{remote_parent_path, send_to_target_directory, upload_local_name};
    use std::path::Path;
    #[test]
    fn send_to_target_directory_prefers_cache_then_home_never_source() {
        // Cached browser path wins.
        assert_eq!(
            send_to_target_directory(Some("/srv/data/"), Some("/home/bob")),
            "/srv/data"
        );
        // No usable cache falls back to the target session's home/cwd.
        assert_eq!(
            send_to_target_directory(None, Some("/home/bob")),
            "/home/bob"
        );
        assert_eq!(
            send_to_target_directory(Some("."), Some("/home/bob")),
            "/home/bob"
        );
        // Nothing usable at all degrades to the safe relative default, and it is
        // never seeded from a source path (the caller passes only target inputs).
        assert_eq!(send_to_target_directory(Some("   "), None), ".");
        assert_eq!(send_to_target_directory(None, None), ".");
    }

    #[test]
    fn parent_and_local_name_keep_legacy_fallbacks() {
        assert_eq!(remote_parent_path("/dir/file"), "/dir");
        assert_eq!(remote_parent_path("/file"), "/");
        assert_eq!(remote_parent_path("file"), ".");
        assert_eq!(remote_parent_path("/"), ".");
        assert_eq!(
            upload_local_name(Path::new("/"), "uploaded_item"),
            "uploaded_item"
        );
    }
}
