use crate::core::sftp::util::content_hash;
use crate::core::sftp::{
    DirectoryChild, FileEntry, FileProperties, RemoteBinaryFile, RemoteTextFile,
    TextFileOpenResult, WriteRemoteTextResult, classify_text_file,
};
use crate::core::{SessionManager, SessionType};
use crate::error::{AppError, AppResult};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[tauri::command]
pub async fn get_local_home_dir(
    state: tauri::State<'_, Arc<SessionManager>>,
    session_id: String,
) -> AppResult<String> {
    ensure_local_session(state.inner(), &session_id).await?;
    local_home_dir_impl()
}

fn local_home_dir_impl() -> AppResult<String> {
    dirs::home_dir()
        .map(path_to_string)
        .filter(|path| !path.is_empty())
        .ok_or_else(|| AppError::Config("Failed to determine local home directory".to_string()))
}

#[tauri::command]
pub async fn list_local_dir(
    state: tauri::State<'_, Arc<SessionManager>>,
    session_id: String,
    path: String,
) -> AppResult<Vec<FileEntry>> {
    ensure_local_session(state.inner(), &session_id).await?;
    let dir_path = PathBuf::from(path.clone());
    let entries = list_local_dir_impl(&dir_path).await?;

    tracing::debug!(
        target: "user_action",
        action = "list",
        entity = "local_directory",
        session_id = %session_id,
        local_path = path,
        item_count = entries.len(),
        "User listed local directory"
    );

    Ok(entries)
}

#[tauri::command]
pub async fn list_local_child_directories(
    state: tauri::State<'_, Arc<SessionManager>>,
    session_id: String,
    path: String,
    show_hidden_files: bool,
) -> AppResult<Vec<DirectoryChild>> {
    ensure_local_session(state.inner(), &session_id).await?;
    let dir_path = PathBuf::from(path.clone());
    let entries = list_local_child_directories_impl(&dir_path, show_hidden_files).await?;

    tracing::debug!(
        target: "user_action",
        action = "list",
        entity = "local_child_directories",
        session_id = %session_id,
        local_path = path,
        item_count = entries.len(),
        "User listed local child directories"
    );

    Ok(entries)
}

async fn list_local_dir_impl(dir_path: &Path) -> AppResult<Vec<FileEntry>> {
    let mut read_dir = tokio::fs::read_dir(dir_path).await.map_err(|error| {
        AppError::Io(std::io::Error::new(
            error.kind(),
            format!(
                "Failed to list local directory '{}': {error}",
                dir_path.display()
            ),
        ))
    })?;

    let mut entries = Vec::new();
    while let Some(entry) = read_dir.next_entry().await? {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.is_empty() {
            continue;
        }
        if let Ok(file_entry) = file_entry_from_path(&entry.path(), name).await {
            entries.push(file_entry);
        }
    }
    Ok(entries)
}

async fn list_local_child_directories_impl(
    dir_path: &Path,
    show_hidden_files: bool,
) -> AppResult<Vec<DirectoryChild>> {
    let mut read_dir = tokio::fs::read_dir(dir_path).await.map_err(|error| {
        AppError::Io(std::io::Error::new(
            error.kind(),
            format!(
                "Failed to list local child directories '{}': {error}",
                dir_path.display()
            ),
        ))
    })?;

    let mut entries = Vec::new();
    while let Some(entry) = read_dir.next_entry().await? {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.is_empty() || name == "." || name == ".." {
            continue;
        }
        if !show_hidden_files && name.starts_with('.') {
            continue;
        }

        let path = entry.path();
        let symlink_metadata = match tokio::fs::symlink_metadata(&path).await {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        let metadata = tokio::fs::metadata(&path)
            .await
            .unwrap_or_else(|_| symlink_metadata.clone());
        if !metadata.is_dir() {
            continue;
        }

        entries.push(DirectoryChild {
            name,
            path: path_to_string(path),
            is_symlink: symlink_metadata.file_type().is_symlink(),
            raw_path_token: None,
        });
    }

    entries.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.name.cmp(&right.name))
    });
    Ok(entries)
}

#[tauri::command]
pub async fn create_local_file(
    state: tauri::State<'_, Arc<SessionManager>>,
    session_id: String,
    path: String,
    mode: Option<String>,
) -> AppResult<()> {
    ensure_local_session(state.inner(), &session_id).await?;
    create_local_file_impl(&path, mode).await
}

async fn create_local_file_impl(path: &str, mode: Option<String>) -> AppResult<()> {
    tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .await?;
    set_local_mode_if_supported(&path, mode).await?;
    Ok(())
}

#[tauri::command]
pub async fn create_local_dir(
    state: tauri::State<'_, Arc<SessionManager>>,
    session_id: String,
    path: String,
    mode: Option<String>,
) -> AppResult<()> {
    ensure_local_session(state.inner(), &session_id).await?;
    create_local_dir_impl(&path, mode).await
}

async fn create_local_dir_impl(path: &str, mode: Option<String>) -> AppResult<()> {
    tokio::fs::create_dir(&path).await?;
    set_local_mode_if_supported(&path, mode).await?;
    Ok(())
}

#[tauri::command]
pub async fn rename_local_file(
    state: tauri::State<'_, Arc<SessionManager>>,
    session_id: String,
    old_path: String,
    new_path: String,
) -> AppResult<()> {
    ensure_local_session(state.inner(), &session_id).await?;
    rename_local_file_impl(&old_path, &new_path).await
}

async fn rename_local_file_impl(old_path: &str, new_path: &str) -> AppResult<()> {
    tokio::fs::rename(&old_path, &new_path).await?;
    Ok(())
}

#[tauri::command]
pub async fn delete_local_file(
    state: tauri::State<'_, Arc<SessionManager>>,
    session_id: String,
    path: String,
) -> AppResult<()> {
    ensure_local_session(state.inner(), &session_id).await?;
    delete_local_file_impl(&path).await
}

async fn delete_local_file_impl(path: &str) -> AppResult<()> {
    let metadata = tokio::fs::symlink_metadata(&path).await?;
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        tokio::fs::remove_dir_all(&path).await?;
    } else {
        tokio::fs::remove_file(&path).await?;
    }
    Ok(())
}

#[tauri::command]
pub async fn get_local_file_properties(
    state: tauri::State<'_, Arc<SessionManager>>,
    session_id: String,
    path: String,
) -> AppResult<FileProperties> {
    ensure_local_session(state.inner(), &session_id).await?;
    file_properties_from_path(Path::new(&path)).await
}

#[tauri::command]
pub async fn read_local_file_text(
    state: tauri::State<'_, Arc<SessionManager>>,
    session_id: String,
    path: String,
    max_bytes: u64,
) -> AppResult<RemoteTextFile> {
    ensure_local_session(state.inner(), &session_id).await?;
    read_local_file_text_impl(&path, max_bytes).await
}

#[tauri::command]
pub async fn open_local_file_text(
    state: tauri::State<'_, Arc<SessionManager>>,
    session_id: String,
    path: String,
    max_bytes: u64,
) -> AppResult<TextFileOpenResult> {
    ensure_local_session(state.inner(), &session_id).await?;
    let file = read_local_file_bytes_impl(&path, max_bytes).await?;
    Ok(classify_text_file(file))
}

async fn read_local_file_text_impl(path: &str, max_bytes: u64) -> AppResult<RemoteTextFile> {
    let metadata = tokio::fs::metadata(&path).await?;
    if metadata.is_dir() {
        return Err(AppError::Config(
            "Cannot read a directory as text".to_string(),
        ));
    }
    if metadata.len() > max_bytes {
        return Err(AppError::Config(format!(
            "File is too large to open ({} bytes)",
            metadata.len()
        )));
    }

    let bytes = tokio::fs::read(&path).await?;
    let file_hash = content_hash(&bytes);
    let content = String::from_utf8(bytes)
        .map_err(|_| AppError::Config("File is not valid UTF-8 text".to_string()))?;
    Ok(RemoteTextFile {
        path: path.to_string(),
        content,
        size: metadata.len(),
        mtime: modified_time_secs(&metadata),
        mtime_nanos: modified_time_nanos(&metadata),
        content_hash: file_hash,
    })
}

#[tauri::command]
pub async fn read_local_file_bytes(
    state: tauri::State<'_, Arc<SessionManager>>,
    session_id: String,
    path: String,
    max_bytes: u64,
) -> AppResult<RemoteBinaryFile> {
    ensure_local_session(state.inner(), &session_id).await?;
    read_local_file_bytes_impl(&path, max_bytes).await
}

async fn read_local_file_bytes_impl(path: &str, max_bytes: u64) -> AppResult<RemoteBinaryFile> {
    let metadata = tokio::fs::metadata(&path).await?;
    if metadata.is_dir() {
        return Err(AppError::Config(
            "Cannot read a directory as bytes".to_string(),
        ));
    }
    if metadata.len() > max_bytes {
        return Err(AppError::Config(format!(
            "File is too large to preview ({} bytes)",
            metadata.len()
        )));
    }

    let bytes = tokio::fs::read(&path).await?;
    Ok(RemoteBinaryFile {
        path: path.to_string(),
        content_bytes: bytes,
        size: metadata.len(),
        mtime: modified_time_secs(&metadata),
        mtime_nanos: modified_time_nanos(&metadata),
    })
}

#[tauri::command]
pub async fn write_local_file_text(
    state: tauri::State<'_, Arc<SessionManager>>,
    session_id: String,
    path: String,
    content: String,
    expected_mtime: Option<u64>,
    expected_size: Option<u64>,
    expected_mtime_nanos: Option<String>,
    expected_hash: Option<String>,
    force: Option<bool>,
) -> AppResult<WriteRemoteTextResult> {
    ensure_local_session(state.inner(), &session_id).await?;
    write_local_file_text_impl(
        &path,
        &content,
        expected_mtime,
        expected_size,
        expected_mtime_nanos.as_deref(),
        expected_hash.as_deref(),
        force.unwrap_or(false),
    )
    .await
}

async fn write_local_file_text_impl(
    path: &str,
    content: &str,
    expected_mtime: Option<u64>,
    expected_size: Option<u64>,
    expected_mtime_nanos: Option<&str>,
    expected_hash: Option<&str>,
    force: bool,
) -> AppResult<WriteRemoteTextResult> {
    let metadata = tokio::fs::metadata(&path).await?;
    let current_mtime = modified_time_secs(&metadata);
    let current_mtime_nanos = modified_time_nanos(&metadata);
    let current_size = metadata.len();
    let has_conflict = expected_mtime.is_some_and(|mtime| mtime != current_mtime)
        || expected_size.is_some_and(|size| size != current_size)
        || expected_mtime_nanos.is_some_and(|mtime| Some(mtime) != current_mtime_nanos.as_deref());

    let has_hash_conflict = if !force && !has_conflict {
        match expected_hash {
            Some(expected_hash) => {
                let current_bytes = tokio::fs::read(&path).await?;
                content_hash(&current_bytes) != expected_hash
            }
            None => false,
        }
    } else {
        false
    };

    if (has_conflict || has_hash_conflict) && !force {
        return Ok(WriteRemoteTextResult::conflict(
            current_mtime,
            current_size,
            current_mtime_nanos,
        ));
    }

    tokio::fs::write(&path, content).await?;
    let next_metadata = tokio::fs::metadata(&path).await?;
    Ok(WriteRemoteTextResult::saved(
        modified_time_secs(&next_metadata),
        next_metadata.len(),
        modified_time_nanos(&next_metadata),
        content_hash(content.as_bytes()),
    ))
}

async fn ensure_local_session(manager: &SessionManager, session_id: &str) -> AppResult<()> {
    let sessions = manager.sessions.lock().await;
    let session = sessions
        .get(session_id)
        .ok_or_else(|| AppError::SessionNotFound(format!("Session '{}' not found", session_id)))?;

    if session.info.session_type != SessionType::Local {
        return Err(AppError::Config(
            "Local file browser commands require a local terminal session".to_string(),
        ));
    }

    Ok(())
}

async fn file_entry_from_path(path: &Path, name: String) -> AppResult<FileEntry> {
    let symlink_metadata = tokio::fs::symlink_metadata(path).await?;
    let metadata = tokio::fs::metadata(path)
        .await
        .unwrap_or_else(|_| symlink_metadata.clone());
    let file_type = symlink_metadata.file_type();

    Ok(FileEntry {
        name,
        is_dir: metadata.is_dir(),
        is_symlink: file_type.is_symlink(),
        size: if metadata.is_dir() { 0 } else { metadata.len() },
        permissions: permissions_string(&metadata, metadata.is_dir()),
        owner: owner_string(&metadata),
        group: group_string(&metadata),
        mtime: modified_time_secs(&metadata),
        raw_path_token: None,
    })
}

async fn file_properties_from_path(path: &Path) -> AppResult<FileProperties> {
    let symlink_metadata = tokio::fs::symlink_metadata(path).await?;
    let is_symlink = symlink_metadata.file_type().is_symlink();
    let symlink_target = if is_symlink {
        Some(path_to_string(tokio::fs::read_link(path).await?))
    } else {
        None
    };
    let metadata = tokio::fs::metadata(path)
        .await
        .unwrap_or_else(|_| symlink_metadata.clone());
    let name = path
        .file_name()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_else(|| path_to_string(path.to_path_buf()));

    Ok(FileProperties {
        name,
        is_dir: metadata.is_dir(),
        is_symlink,
        symlink_target,
        size: if metadata.is_dir() { 0 } else { metadata.len() },
        permissions: permissions_string(&metadata, metadata.is_dir()),
        owner: owner_string(&metadata),
        group: group_string(&metadata),
        uid: uid_string(&metadata),
        gid: gid_string(&metadata),
        mtime: modified_time_secs(&metadata),
        atime: accessed_time_secs(&metadata),
    })
}

fn path_to_string(path: PathBuf) -> String {
    path.to_string_lossy().to_string()
}

fn system_time_secs(time: std::io::Result<std::time::SystemTime>) -> u64 {
    time.ok()
        .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |duration| duration.as_secs())
}

fn modified_time_secs(metadata: &std::fs::Metadata) -> u64 {
    system_time_secs(metadata.modified())
}

fn modified_time_nanos(metadata: &std::fs::Metadata) -> Option<String> {
    metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos().to_string())
}

fn accessed_time_secs(metadata: &std::fs::Metadata) -> u64 {
    system_time_secs(metadata.accessed())
}

#[cfg(unix)]
fn permissions_string(metadata: &std::fs::Metadata, is_dir: bool) -> String {
    use std::os::unix::fs::PermissionsExt;

    let mode = metadata.permissions().mode();
    let mut output = String::with_capacity(10);
    output.push(if is_dir { 'd' } else { '-' });
    for (read, write, exec) in [
        (0o400, 0o200, 0o100),
        (0o040, 0o020, 0o010),
        (0o004, 0o002, 0o001),
    ] {
        output.push(if mode & read != 0 { 'r' } else { '-' });
        output.push(if mode & write != 0 { 'w' } else { '-' });
        output.push(if mode & exec != 0 { 'x' } else { '-' });
    }
    output
}

#[cfg(not(unix))]
fn permissions_string(metadata: &std::fs::Metadata, is_dir: bool) -> String {
    let mut output = String::from(if is_dir { "d" } else { "-" });
    output.push_str(if metadata.permissions().readonly() {
        "r-xr-xr-x"
    } else {
        "rwxrwxrwx"
    });
    output
}

#[cfg(unix)]
fn owner_string(metadata: &std::fs::Metadata) -> String {
    use std::os::unix::fs::MetadataExt;
    metadata.uid().to_string()
}

#[cfg(not(unix))]
fn owner_string(_metadata: &std::fs::Metadata) -> String {
    String::new()
}

#[cfg(unix)]
fn group_string(metadata: &std::fs::Metadata) -> String {
    use std::os::unix::fs::MetadataExt;
    metadata.gid().to_string()
}

#[cfg(not(unix))]
fn group_string(_metadata: &std::fs::Metadata) -> String {
    String::new()
}

fn uid_string(metadata: &std::fs::Metadata) -> String {
    owner_string(metadata)
}

fn gid_string(metadata: &std::fs::Metadata) -> String {
    group_string(metadata)
}

#[cfg(unix)]
async fn set_local_mode_if_supported(path: &str, mode: Option<String>) -> AppResult<()> {
    use std::os::unix::fs::PermissionsExt;

    let Some(mode) = mode else {
        return Ok(());
    };
    let parsed = u32::from_str_radix(mode.trim(), 8)
        .map_err(|_| AppError::Config(format!("Invalid file mode: {mode}")))?;
    let permissions = std::fs::Permissions::from_mode(parsed);
    tokio::fs::set_permissions(path, permissions).await?;
    Ok(())
}

#[cfg(not(unix))]
#[allow(clippy::unused_async)]
async fn set_local_mode_if_supported(_path: &str, _mode: Option<String>) -> AppResult<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AiExecutionProfile;
    use crate::core::{
        DynamicTitleCapabilities, SessionHandle, SessionInfo, session_command_channel,
    };
    use tokio::sync::Mutex;

    fn temp_test_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("nyaterm-local-fs-{name}-{}", uuid::Uuid::new_v4()))
    }

    async fn cleanup(path: &Path) {
        let _ = tokio::fs::remove_dir_all(path).await;
        let _ = tokio::fs::remove_file(path).await;
    }

    #[tokio::test]
    async fn local_dir_listing_returns_files_and_directories() {
        let root = temp_test_dir("list");
        tokio::fs::create_dir_all(root.join("nested"))
            .await
            .unwrap();
        tokio::fs::write(root.join("note.txt"), "hello")
            .await
            .unwrap();

        let entries = list_local_dir_impl(&root).await.unwrap();
        let names: std::collections::HashSet<_> =
            entries.iter().map(|entry| entry.name.as_str()).collect();

        assert!(names.contains("nested"));
        assert!(names.contains("note.txt"));
        assert!(
            entries
                .iter()
                .any(|entry| entry.name == "nested" && entry.is_dir)
        );
        assert!(
            entries
                .iter()
                .any(|entry| entry.name == "note.txt" && !entry.is_dir)
        );

        cleanup(&root).await;
    }

    #[test]
    fn local_home_dir_returns_existing_directory() {
        let home = local_home_dir_impl().unwrap();
        assert!(!home.is_empty());
        assert!(Path::new(&home).is_dir());
    }

    #[tokio::test]
    async fn local_create_rename_read_write_and_delete_work() {
        let root = temp_test_dir("mutate");
        tokio::fs::create_dir_all(&root).await.unwrap();
        let original = root.join("draft.txt");
        let renamed = root.join("final.txt");

        create_local_file_impl(original.to_str().unwrap(), Some("0644".to_string()))
            .await
            .unwrap();
        rename_local_file_impl(original.to_str().unwrap(), renamed.to_str().unwrap())
            .await
            .unwrap();
        write_local_file_text_impl(
            renamed.to_str().unwrap(),
            "hello",
            None,
            None,
            None,
            None,
            false,
        )
        .await
        .unwrap();
        let text = read_local_file_text_impl(renamed.to_str().unwrap(), 1024)
            .await
            .unwrap();

        assert_eq!(text.content, "hello");
        assert_eq!(text.size, 5);
        assert_eq!(text.content_hash, content_hash(b"hello"));

        let saved = write_local_file_text_impl(
            renamed.to_str().unwrap(),
            "hello!",
            Some(text.mtime),
            Some(text.size),
            text.mtime_nanos.as_deref(),
            Some(&text.content_hash),
            false,
        )
        .await
        .unwrap();
        assert_eq!(saved.status, "saved");
        assert_eq!(
            saved.content_hash.as_deref(),
            Some(content_hash(b"hello!").as_str())
        );

        let text = read_local_file_text_impl(renamed.to_str().unwrap(), 1024)
            .await
            .unwrap();

        let conflict = write_local_file_text_impl(
            renamed.to_str().unwrap(),
            "changed",
            Some(text.mtime + 1),
            Some(text.size),
            text.mtime_nanos.as_deref(),
            Some(&text.content_hash),
            false,
        )
        .await
        .unwrap();
        assert_eq!(conflict.status, "conflict");

        let conflict = write_local_file_text_impl(
            renamed.to_str().unwrap(),
            "changed",
            Some(text.mtime),
            Some(text.size + 1),
            text.mtime_nanos.as_deref(),
            Some(&text.content_hash),
            false,
        )
        .await
        .unwrap();
        assert_eq!(conflict.status, "conflict");

        let conflict = write_local_file_text_impl(
            renamed.to_str().unwrap(),
            "changed",
            Some(text.mtime),
            Some(text.size),
            text.mtime_nanos.as_deref(),
            Some("definitely-not-the-current-hash"),
            false,
        )
        .await
        .unwrap();
        assert_eq!(conflict.status, "conflict");

        let forced = write_local_file_text_impl(
            renamed.to_str().unwrap(),
            "changed",
            Some(text.mtime),
            Some(text.size + 1),
            text.mtime_nanos.as_deref(),
            Some("definitely-not-the-current-hash"),
            true,
        )
        .await
        .unwrap();
        assert_eq!(forced.status, "saved");
        assert_eq!(
            forced.content_hash.as_deref(),
            Some(content_hash(b"changed").as_str())
        );

        delete_local_file_impl(root.to_str().unwrap())
            .await
            .unwrap();
        assert!(!root.exists());
    }

    #[tokio::test]
    async fn local_delete_missing_path_returns_error() {
        let missing = temp_test_dir("missing");
        let error = delete_local_file_impl(missing.to_str().unwrap())
            .await
            .expect_err("missing path should error");
        assert!(matches!(error, AppError::Io(_)));
    }

    #[tokio::test]
    async fn local_read_file_bytes_returns_byte_payload() {
        let root = temp_test_dir("bytes");
        tokio::fs::create_dir_all(&root).await.unwrap();
        let file = root.join("image.bin");
        tokio::fs::write(&file, [0_u8, 1, 2, 253, 254, 255])
            .await
            .unwrap();

        let binary = read_local_file_bytes_impl(file.to_str().unwrap(), 1024)
            .await
            .unwrap();

        assert_eq!(binary.content_bytes, [0, 1, 2, 253, 254, 255]);
        assert_eq!(binary.size, 6);

        cleanup(&root).await;
    }

    #[tokio::test]
    async fn local_read_file_bytes_rejects_directories() {
        let root = temp_test_dir("bytes-dir");
        tokio::fs::create_dir_all(&root).await.unwrap();

        let error = read_local_file_bytes_impl(root.to_str().unwrap(), 1024)
            .await
            .expect_err("directories should be rejected");

        assert!(matches!(error, AppError::Config(_)));
        cleanup(&root).await;
    }

    #[tokio::test]
    async fn local_read_file_bytes_rejects_files_over_limit() {
        let root = temp_test_dir("bytes-limit");
        tokio::fs::create_dir_all(&root).await.unwrap();
        let file = root.join("large.bin");
        tokio::fs::write(&file, [1_u8, 2, 3, 4]).await.unwrap();

        let error = read_local_file_bytes_impl(file.to_str().unwrap(), 3)
            .await
            .expect_err("oversized files should be rejected");

        assert!(matches!(error, AppError::Config(_)));
        cleanup(&root).await;
    }

    #[tokio::test]
    async fn regular_file_properties_have_no_symlink_target() {
        let root = temp_test_dir("regular-properties");
        tokio::fs::create_dir_all(&root).await.unwrap();
        let file = root.join("file.txt");
        tokio::fs::write(&file, b"hello").await.unwrap();

        let properties = file_properties_from_path(&file).await.unwrap();
        assert!(!properties.is_symlink);
        assert_eq!(properties.symlink_target, None);

        cleanup(&root).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn dangling_symlink_properties_preserve_relative_target() {
        use std::os::unix::fs::symlink;

        let root = temp_test_dir("dangling-symlink-properties");
        tokio::fs::create_dir_all(&root).await.unwrap();
        let link = root.join("current");
        symlink("../missing-release", &link).unwrap();

        let properties = file_properties_from_path(&link).await.unwrap();
        assert!(properties.is_symlink);
        assert_eq!(
            properties.symlink_target.as_deref(),
            Some("../missing-release")
        );

        cleanup(&root).await;
    }

    #[tokio::test]
    async fn ensure_local_session_rejects_non_local_sessions() {
        let manager = SessionManager::new();
        let (cmd_tx, _cmd_rx) = session_command_channel("ssh-1");
        manager
            .add_session(SessionHandle {
                info: SessionInfo {
                    id: "ssh-1".to_string(),
                    name: "SSH".to_string(),
                    session_type: SessionType::SSH,
                    started_at: crate::core::now_session_started_at(),
                    connection_id: None,
                    connected: true,
                    owner_window_label: None,
                    ai_execution_profile: AiExecutionProfile::default(),
                    injection_active: true,
                    dynamic_title_capabilities: DynamicTitleCapabilities::default(),
                    remote_file_browser_enabled: true,
                    remote_stats_enabled: true,
                    ssh_profile: None,
                },
                cmd_tx,
                startup_input_barrier: None,
                ssh_config: None,
                ssh_handle: None,
                cwd: Arc::new(Mutex::new(Default::default())),
                remote_fs: None,
            })
            .await;

        let error = ensure_local_session(&manager, "ssh-1")
            .await
            .expect_err("SSH session should be rejected");
        assert!(matches!(error, AppError::Config(_)));
    }
}
