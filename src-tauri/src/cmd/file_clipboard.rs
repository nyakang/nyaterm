use std::time::Duration;

#[cfg(not(target_os = "windows"))]
use std::path::PathBuf;

const CLIPBOARD_TIMEOUT: Duration = Duration::from_secs(1);

/// Read the local file paths currently held on the OS clipboard (e.g. files
/// copied/cut in the system file manager). Unlike `read_clipboard_path_payload`,
/// this returns ALL paths, not just images, so SFTP paste can upload arbitrary
/// files. Returns an empty vector when the clipboard holds no file paths.
#[tauri::command]
pub async fn read_clipboard_file_paths() -> Vec<String> {
    let result = tokio::time::timeout(
        CLIPBOARD_TIMEOUT,
        tokio::task::spawn_blocking(read_clipboard_file_paths_blocking),
    )
    .await;

    match result {
        Ok(Ok(paths)) => paths,
        _ => Vec::new(),
    }
}

fn read_clipboard_file_paths_blocking() -> Vec<String> {
    #[cfg(target_os = "windows")]
    {
        if let Some(paths) = read_windows_clipboard_file_paths() {
            return paths;
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        if let Some(paths) = read_clipboard_native_file_paths() {
            return paths;
        }
        if let Some(paths) = read_clipboard_text_file_paths() {
            return paths;
        }
    }

    Vec::new()
}

/// Read the native file list from the clipboard. On macOS, Finder puts file
/// URLs on the pasteboard rather than plain text, so the text parser alone
/// would never see them. Returns `None` when the clipboard holds no native
/// file list (e.g. plain text or images).
#[cfg(not(target_os = "windows"))]
fn read_clipboard_native_file_paths() -> Option<Vec<String>> {
    let mut clipboard = arboard::Clipboard::new().ok()?;
    match clipboard.get().ok()? {
        arboard::Content::Files(files) => Some(
            files
                .into_iter()
                .filter_map(|file| file.path)
                .filter(|path| path.exists())
                .map(|path| path.to_string_lossy().to_string())
                .collect(),
        ),
        _ => None,
    }
}

#[cfg(not(target_os = "windows"))]
fn read_clipboard_text_file_paths() -> Option<Vec<String>> {
    let mut clipboard = arboard::Clipboard::new().ok()?;
    let text = clipboard.get_text().ok()?;
    Some(
        text.lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .filter_map(parse_clipboard_path_text_line)
            .filter(|path| path.exists())
            .map(|path| path.to_string_lossy().to_string())
            .collect(),
    )
}

#[cfg(not(target_os = "windows"))]
fn parse_clipboard_path_text_line(line: &str) -> Option<PathBuf> {
    let unwrapped = line
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .or_else(|| {
            line.strip_prefix('\'')
                .and_then(|value| value.strip_suffix('\''))
        })
        .unwrap_or(line);

    if let Some(uri_path) = unwrapped.strip_prefix("file://") {
        let local_uri_path = match uri_path.strip_prefix("localhost/") {
            Some(path) => format!("/{path}"),
            None => uri_path.to_string(),
        };
        let decoded = urlencoding::decode(&local_uri_path).ok()?;
        return Some(PathBuf::from(decoded.as_ref()));
    }

    let path = PathBuf::from(unwrapped);
    if path.is_absolute() { Some(path) } else { None }
}

#[cfg(target_os = "windows")]
fn read_windows_clipboard_file_paths() -> Option<Vec<String>> {
    use windows::Win32::{
        System::{
            DataExchange::{CloseClipboard, GetClipboardData, OpenClipboard},
            Ole::CF_HDROP,
        },
        UI::Shell::{DragQueryFileW, HDROP},
    };

    struct ClipboardGuard;
    impl Drop for ClipboardGuard {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseClipboard();
            }
        }
    }

    unsafe {
        OpenClipboard(None).ok()?;
        let _guard = ClipboardGuard;
        let handle = GetClipboardData(u32::from(CF_HDROP.0)).ok()?;
        let hdrop = HDROP(handle.0);
        let count = DragQueryFileW(hdrop, u32::MAX, None);
        if count == 0 {
            return Some(Vec::new());
        }

        let mut paths = Vec::new();
        for index in 0..count {
            let char_count = DragQueryFileW(hdrop, index, None);
            if char_count == 0 {
                continue;
            }

            let mut buffer = vec![0u16; char_count as usize + 1];
            let written = DragQueryFileW(hdrop, index, Some(&mut buffer));
            if written == 0 {
                continue;
            }

            let path = String::from_utf16_lossy(&buffer[..written as usize]);
            if !path.trim().is_empty() {
                paths.push(path);
            }
        }

        Some(paths)
    }
}
