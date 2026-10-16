//! Backend-agnostic file transfer lifecycle: pause, resume, cancel, progress.
//!
//! Every backend (SFTP, SCP Enhanced, SCP Normal) delegates transfer bookkeeping
//! to the same `TransferController` and global registries so the frontend
//! `transfer-event` contract is identical regardless of the underlying protocol.

use crate::error::{AppError, AppResult};
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tauri::Emitter;
use tokio::sync::Notify;

pub(crate) const TRANSFER_CANCELLED_MESSAGE: &str = "Transfer cancelled";
const RECENT_TRANSFER_TARGET_LIMIT: usize = 200;

lazy_static::lazy_static! {
    static ref ACTIVE_TRANSFERS: Arc<Mutex<HashMap<String, Arc<TransferController>>>> =
        Arc::new(Mutex::new(HashMap::new()));
    static ref RECENT_TRANSFER_TARGETS: Arc<Mutex<VecDeque<(String, TransferTargetSnapshot)>>> =
        Arc::new(Mutex::new(VecDeque::new()));
    static ref LOCAL_DOWNLOAD_TARGETS: Mutex<HashMap<PathBuf, bool>> =
        Mutex::new(HashMap::new());
}

/// Event payload emitted to the frontend to track file transfer lifecycle.
#[derive(Debug, Clone, Serialize)]
pub struct TransferEvent {
    pub id: String,
    pub session_id: String,
    pub file_name: String,
    pub remote_path: String,
    pub local_path: String,
    /// "upload" or "download"
    pub direction: String,
    /// "file" or "directory"
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// "started", "progress", "paused", "resumed", "completed", "skipped", "cancelled", or "error"
    pub status: String,
    pub size: u64,
    pub bytes_transferred: u64,
    pub total_size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_count_total: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_count_completed: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_msg: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TransferControlState {
    Running,
    Paused,
    Cancelled,
}

#[derive(Debug)]
pub(crate) struct TransferRuntime {
    id: String,
    session_id: String,
    file_name: String,
    remote_path: String,
    local_path: String,
    direction: String,
    kind: String,
    source: Option<String>,
    parent_id: Option<String>,
    bytes_transferred: u64,
    total_size: u64,
    item_count_total: Option<u64>,
    item_count_completed: Option<u64>,
    control_state: TransferControlState,
}

#[derive(Debug, Clone)]
struct TransferTargetSnapshot {
    local_path: String,
    direction: String,
    kind: String,
}

#[derive(Debug)]
pub(crate) struct TransferController {
    pub(crate) runtime: Mutex<TransferRuntime>,
    pub(crate) notify: Notify,
}

impl TransferController {
    pub(crate) fn new_with_kind(
        id: String,
        session_id: String,
        file_name: String,
        remote_path: String,
        local_path: String,
        direction: String,
        kind: String,
        parent_id: Option<String>,
        item_count_total: Option<u64>,
        item_count_completed: Option<u64>,
    ) -> Self {
        Self::new_with_kind_and_source(
            id,
            session_id,
            file_name,
            remote_path,
            local_path,
            direction,
            kind,
            parent_id,
            item_count_total,
            item_count_completed,
            None,
        )
    }

    pub(crate) fn new_with_kind_and_source(
        id: String,
        session_id: String,
        file_name: String,
        remote_path: String,
        local_path: String,
        direction: String,
        kind: String,
        parent_id: Option<String>,
        item_count_total: Option<u64>,
        item_count_completed: Option<u64>,
        source: Option<String>,
    ) -> Self {
        Self {
            runtime: Mutex::new(TransferRuntime {
                id,
                session_id,
                file_name,
                remote_path,
                local_path,
                direction,
                kind,
                source,
                parent_id,
                bytes_transferred: 0,
                total_size: 0,
                item_count_total,
                item_count_completed,
                control_state: TransferControlState::Running,
            }),
            notify: Notify::new(),
        }
    }

    pub(crate) fn id(&self) -> String {
        self.runtime.lock().unwrap().id.clone()
    }

    pub(crate) fn update_progress(&self, bytes_transferred: u64, total_size: u64) {
        let (parent_id, delta) = {
            let mut runtime = self.runtime.lock().unwrap();
            let delta = bytes_transferred.saturating_sub(runtime.bytes_transferred);
            runtime.bytes_transferred = bytes_transferred;
            runtime.total_size = total_size;
            (runtime.parent_id.clone(), delta)
        };

        if delta == 0 {
            return;
        }

        if let Some(parent_id) = parent_id {
            if let Some(parent) = find_transfer(&parent_id) {
                parent.add_bytes_transferred(delta);
            }
        }
    }

    pub(crate) fn update_item_progress(&self, completed: u64, total: u64) {
        let mut runtime = self.runtime.lock().unwrap();
        runtime.item_count_completed = Some(completed);
        runtime.item_count_total = Some(total);
    }

    pub(crate) fn update_totals(&self, total_size: u64, item_count_total: u64) {
        let mut runtime = self.runtime.lock().unwrap();
        runtime.total_size = total_size;
        runtime.item_count_total = Some(item_count_total);
    }

    fn add_bytes_transferred(&self, delta: u64) {
        let mut runtime = self.runtime.lock().unwrap();
        runtime.bytes_transferred = runtime.bytes_transferred.saturating_add(delta);
        if runtime.total_size > 0 {
            runtime.bytes_transferred = runtime.bytes_transferred.min(runtime.total_size);
        }
    }

    pub(crate) fn build_event(
        &self,
        status: &str,
        size: u64,
        error_msg: Option<String>,
    ) -> TransferEvent {
        let runtime = self.runtime.lock().unwrap();
        TransferEvent {
            id: runtime.id.clone(),
            session_id: runtime.session_id.clone(),
            file_name: runtime.file_name.clone(),
            remote_path: runtime.remote_path.clone(),
            local_path: runtime.local_path.clone(),
            direction: runtime.direction.clone(),
            kind: runtime.kind.clone(),
            source: runtime.source.clone(),
            status: status.to_string(),
            size,
            bytes_transferred: runtime.bytes_transferred,
            total_size: runtime.total_size,
            parent_id: runtime.parent_id.clone(),
            item_count_total: runtime.item_count_total,
            item_count_completed: runtime.item_count_completed,
            error_msg,
        }
    }

    fn pause(&self) -> Option<TransferEvent> {
        {
            let mut runtime = self.runtime.lock().unwrap();
            if runtime.control_state != TransferControlState::Running {
                return None;
            }
            runtime.control_state = TransferControlState::Paused;
        }
        Some(self.build_event("paused", 0, None))
    }

    fn resume(&self) -> Option<TransferEvent> {
        {
            let mut runtime = self.runtime.lock().unwrap();
            if runtime.control_state != TransferControlState::Paused {
                return None;
            }
            runtime.control_state = TransferControlState::Running;
        }
        self.notify.notify_waiters();
        Some(self.build_event("resumed", 0, None))
    }

    fn cancel(&self) -> Option<TransferEvent> {
        {
            let mut runtime = self.runtime.lock().unwrap();
            if runtime.control_state == TransferControlState::Cancelled {
                return None;
            }
            runtime.control_state = TransferControlState::Cancelled;
        }
        self.notify.notify_waiters();
        Some(self.build_event("cancelled", 0, None))
    }

    pub(crate) fn control_state(&self) -> TransferControlState {
        self.runtime.lock().unwrap().control_state
    }

    pub(crate) fn item_count_total(&self) -> Option<u64> {
        self.runtime.lock().unwrap().item_count_total
    }

    fn target_snapshot(&self) -> TransferTargetSnapshot {
        let runtime = self.runtime.lock().unwrap();
        TransferTargetSnapshot {
            local_path: runtime.local_path.clone(),
            direction: runtime.direction.clone(),
            kind: runtime.kind.clone(),
        }
    }
}

pub(crate) fn register_transfer(controller: Arc<TransferController>) {
    let mut active = ACTIVE_TRANSFERS.lock().unwrap();
    if RETAINED_TRANSFERS
        .lock()
        .unwrap()
        .contains_key(&controller.id())
    {
        if let Some(previous) = active.get(&controller.id()) {
            let control_state = previous.control_state();
            controller.runtime.lock().unwrap().control_state = control_state;
        }
    }
    active.insert(controller.id(), controller);
}

pub(crate) fn unregister_transfer(id: &str) {
    if RETAINED_TRANSFERS.lock().unwrap().contains_key(id) {
        return;
    }
    let removed = ACTIVE_TRANSFERS.lock().unwrap().remove(id);
    if let Some(controller) = removed {
        let snapshot = controller.target_snapshot();
        // 询问阶段尚未确定输出，不把跳过或提前取消登记为下载目标。
        if !snapshot.local_path.is_empty() {
            remember_transfer_target(id.to_string(), snapshot);
        }
    }
}

fn find_transfer(id: &str) -> Option<Arc<TransferController>> {
    ACTIVE_TRANSFERS.lock().unwrap().get(id).cloned()
}

fn remember_transfer_target(id: String, snapshot: TransferTargetSnapshot) {
    let mut recent = RECENT_TRANSFER_TARGETS.lock().unwrap();
    if let Some(index) = recent
        .iter()
        .position(|(existing_id, _)| existing_id == &id)
    {
        recent.remove(index);
    }

    recent.push_front((id, snapshot));
    while recent.len() > RECENT_TRANSFER_TARGET_LIMIT {
        recent.pop_back();
    }
}

fn find_transfer_target(id: &str) -> Option<TransferTargetSnapshot> {
    if let Some(controller) = find_transfer(id) {
        return Some(controller.target_snapshot());
    }

    RECENT_TRANSFER_TARGETS
        .lock()
        .unwrap()
        .iter()
        .find_map(|(recent_id, snapshot)| {
            if recent_id == id {
                Some(snapshot.clone())
            } else {
                None
            }
        })
}

pub(crate) fn transfer_target_directory(transfer_id: &str) -> AppResult<PathBuf> {
    let snapshot = find_transfer_target(transfer_id)
        .ok_or_else(|| AppError::Config("Transfer target is no longer available".to_string()))?;

    if snapshot.direction != "download" {
        return Err(AppError::Config(
            "Only download target directories can be opened".to_string(),
        ));
    }

    let local_path_text = snapshot.local_path.trim();
    if local_path_text.is_empty() {
        return Err(AppError::Config(
            "Transfer target path is empty".to_string(),
        ));
    }

    let local_path = PathBuf::from(local_path_text);
    let target_dir = if snapshot.kind == "directory" {
        local_path
    } else {
        local_path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf()
    };

    if !target_dir.exists() {
        return Err(AppError::Config(
            "Transfer target directory does not exist".to_string(),
        ));
    }
    if !target_dir.is_dir() {
        return Err(AppError::Config(
            "Transfer target path is not a directory".to_string(),
        ));
    }

    Ok(target_dir)
}

pub(crate) fn active_transfer_count() -> usize {
    ACTIVE_TRANSFERS.lock().unwrap().len()
}

pub(crate) fn file_name_from_path(path: &str) -> String {
    path.split(['/', '\\'])
        .filter(|segment| !segment.is_empty())
        .next_back()
        .unwrap_or(path)
        .to_string()
}

pub(crate) fn sanitize_local_download_target(local_path: &str, remote_path: &str) -> String {
    let remote_file_name = remote_file_name_from_path(remote_path);
    let safe_file_name = super::util::sanitize_download_file_name(&remote_file_name);
    if safe_file_name == remote_file_name {
        return local_path.to_string();
    }

    if let Some(prefix) = local_path.strip_suffix(&remote_file_name)
        && prefix
            .chars()
            .next_back()
            .is_some_and(|ch| matches!(ch, '/' | '\\'))
    {
        return format!("{prefix}{safe_file_name}");
    }

    let path = std::path::Path::new(local_path);
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return local_path.to_string();
    };

    if file_name != remote_file_name {
        return local_path.to_string();
    }

    path.parent()
        .map(|parent| parent.join(&safe_file_name))
        .unwrap_or_else(|| std::path::PathBuf::from(&safe_file_name))
        .to_string_lossy()
        .to_string()
}

fn remote_file_name_from_path(path: &str) -> String {
    path.split('/')
        .filter(|segment| !segment.is_empty())
        .next_back()
        .unwrap_or(path)
        .to_string()
}

pub(crate) fn create_directory_transfer_controller(
    id: Option<String>,
    session_id: &str,
    display_name: String,
    remote_path: &str,
    local_path: &str,
    direction: &str,
    item_count_total: u64,
    total_size: u64,
) -> Arc<TransferController> {
    let controller = Arc::new(TransferController::new_with_kind(
        id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
        session_id.to_string(),
        display_name,
        remote_path.to_string(),
        local_path.to_string(),
        direction.to_string(),
        "directory".to_string(),
        None,
        Some(item_count_total),
        Some(0),
    ));
    controller.update_progress(0, total_size);
    controller
}

pub(crate) fn create_child_file_transfer_controller(
    id: Option<String>,
    session_id: &str,
    file_name: String,
    remote_path: &str,
    local_path: &str,
    direction: &str,
    parent_id: Option<String>,
) -> Arc<TransferController> {
    Arc::new(TransferController::new_with_kind(
        id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
        session_id.to_string(),
        file_name,
        remote_path.to_string(),
        local_path.to_string(),
        direction.to_string(),
        "file".to_string(),
        parent_id,
        None,
        None,
    ))
}

pub(crate) async fn wait_for_transfer_ready(controller: &Arc<TransferController>) -> AppResult<()> {
    loop {
        let notified = controller.notify.notified();
        match controller.control_state() {
            TransferControlState::Running => return Ok(()),
            TransferControlState::Cancelled => {
                return Err(AppError::Cancelled(TRANSFER_CANCELLED_MESSAGE.to_string()));
            }
            TransferControlState::Paused => notified.await,
        }
    }
}

lazy_static::lazy_static! {
    static ref RETAINED_TRANSFERS: Mutex<HashMap<String, ()>> = Mutex::new(HashMap::new());
}

// 保留注册以覆盖下载询问/重试，以及移动完成后的验证阶段。
pub(crate) struct RetainedTransfer(String);
impl RetainedTransfer {
    pub(crate) fn new(controller: Arc<TransferController>) -> Self {
        let id = controller.id();
        RETAINED_TRANSFERS.lock().unwrap().insert(id.clone(), ());
        register_transfer(controller);
        Self(id)
    }
    pub(crate) async fn ready(&self) -> AppResult<()> {
        let controller = find_transfer(&self.0)
            .ok_or_else(|| AppError::Cancelled(TRANSFER_CANCELLED_MESSAGE.into()))?;
        wait_for_transfer_ready(&controller).await
    }
}
impl Drop for RetainedTransfer {
    fn drop(&mut self) {
        RETAINED_TRANSFERS.lock().unwrap().remove(&self.0);
        unregister_transfer(&self.0);
    }
}

#[cfg(test)]
mod retained_transfer_tests {
    use super::*;
    #[tokio::test]
    async fn clipboard_move_retains_cancellation_after_copy_completion() {
        let id = uuid::Uuid::new_v4().to_string();
        let make = || {
            create_child_file_transfer_controller(
                Some(id.clone()),
                "session",
                "file".into(),
                "/source",
                "/target",
                "copy",
                None,
            )
        };
        let retained = RetainedTransfer::new(make());
        find_transfer(&id).unwrap().cancel();
        register_transfer(make());
        unregister_transfer(&id);
        assert!(matches!(
            retained.ready().await,
            Err(AppError::Cancelled(_))
        ));
        drop(retained);
        assert!(find_transfer(&id).is_none());
    }
}

pub(crate) async fn wait_for_transfer_cancelled(
    controller: &Arc<TransferController>,
) -> AppResult<()> {
    loop {
        let notified = controller.notify.notified();
        if controller.control_state() == TransferControlState::Cancelled {
            return Err(AppError::Cancelled(TRANSFER_CANCELLED_MESSAGE.to_string()));
        }
        notified.await;
    }
}

pub(crate) async fn wait_for_transfer_chain(
    controller: &Arc<TransferController>,
    parent_controller: Option<&Arc<TransferController>>,
) -> AppResult<()> {
    if let Some(parent) = parent_controller {
        wait_for_transfer_ready(parent).await?;
    }
    wait_for_transfer_ready(controller).await
}

pub(crate) fn emit_parent_progress(
    app: &tauri::AppHandle,
    parent_controller: Option<&Arc<TransferController>>,
) {
    if let Some(parent) = parent_controller {
        let _ = app.emit("transfer-event", &parent.build_event("progress", 0, None));
    }
}

pub(crate) async fn cleanup_cancelled_download(local_path: &str) {
    if tokio::fs::remove_file(local_path).await.is_err() {
        let _ = tokio::fs::remove_dir_all(local_path).await;
    }
}

pub(crate) fn remember_transfer_target_external(
    id: String,
    local_path: String,
    direction: String,
    kind: String,
) {
    remember_transfer_target(
        id,
        TransferTargetSnapshot {
            local_path,
            direction,
            kind,
        },
    );
}

pub async fn pause_transfer(app: tauri::AppHandle, transfer_id: &str) -> AppResult<()> {
    if let Some(controller) = find_transfer(transfer_id) {
        if let Some(event) = controller.pause() {
            let _ = app.emit("transfer-event", &event);
        }
    }
    Ok(())
}

pub async fn resume_transfer(app: tauri::AppHandle, transfer_id: &str) -> AppResult<()> {
    if let Some(controller) = find_transfer(transfer_id) {
        if let Some(event) = controller.resume() {
            let _ = app.emit("transfer-event", &event);
        }
    }
    Ok(())
}

pub async fn cancel_transfer(app: tauri::AppHandle, transfer_id: &str) -> AppResult<()> {
    if let Some(controller) = find_transfer(transfer_id) {
        if let Some(event) = controller.cancel() {
            let _ = app.emit("transfer-event", &event);
        }
    }
    Ok(())
}

/// 持有目标占用直到下载结束，避免并发任务选中尚未创建的同一路径。
pub(crate) struct LocalDownloadTarget {
    path: String,
    key: PathBuf,
}

impl std::ops::Deref for LocalDownloadTarget {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.path
    }
}

impl Drop for LocalDownloadTarget {
    fn drop(&mut self) {
        LOCAL_DOWNLOAD_TARGETS.lock().unwrap().remove(&self.key);
    }
}

fn local_download_target_key(path: &str) -> AppResult<PathBuf> {
    let absolute = std::path::absolute(path)
        .map_err(|error| AppError::Channel(format!("Invalid download path: {error}")))?;
    // 未创建的父目录也从最近的已有祖先规范化，保持创建前后的占用键一致。
    let mut ancestor = absolute.as_path();
    let mut suffix = Vec::new();
    let mut key = loop {
        if let Ok(existing) = ancestor.canonicalize() {
            break existing;
        }
        if let Some(name) = ancestor.file_name() {
            suffix.push(name.to_os_string());
        }
        match ancestor.parent() {
            Some(parent) => ancestor = parent,
            None => {
                return Err(AppError::Channel(format!(
                    "Failed to normalize download path: {path}"
                )));
            }
        }
    };
    for component in suffix.into_iter().rev() {
        key.push(component);
    }
    #[cfg(windows)]
    let key = PathBuf::from(key.to_string_lossy().to_lowercase());
    Ok(key)
}

fn local_path_exists(path: &str) -> AppResult<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(AppError::Channel(format!(
            "Failed to inspect download path '{path}': {error}"
        ))),
    }
}

fn reserve_local_download_target(
    path: &str,
    is_directory: bool,
    overwrite: bool,
) -> AppResult<Option<LocalDownloadTarget>> {
    let key = local_download_target_key(path)?;
    let mut targets = LOCAL_DOWNLOAD_TARGETS.lock().unwrap();
    let occupied = targets.iter().any(|(target, directory)| {
        *target == key
            || (*directory && key.starts_with(target))
            || (is_directory && target.starts_with(&key))
    });
    if occupied {
        return if overwrite {
            Err(AppError::Channel(format!(
                "Download destination is in use by another transfer: {path}"
            )))
        } else {
            Ok(None)
        };
    }
    if !overwrite && local_path_exists(path)? {
        return Ok(None);
    }
    targets.insert(key.clone(), is_directory);
    Ok(Some(LocalDownloadTarget {
        path: path.to_string(),
        key,
    }))
}

fn local_rename_candidate(local_path: &str, is_directory: bool, index: u32) -> String {
    let path = Path::new(local_path);
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let stem = if is_directory {
        path.file_name()
    } else {
        path.file_stem()
    }
    .and_then(|value| value.to_str())
    .filter(|value| !value.is_empty())
    .unwrap_or(if is_directory { "folder" } else { "file" });
    let ext = if is_directory {
        String::new()
    } else {
        path.extension()
            .and_then(|value| value.to_str())
            .map(|value| format!(".{value}"))
            .unwrap_or_default()
    };
    parent
        .join(format!("{stem}({index}){ext}"))
        .to_string_lossy()
        .to_string()
}

pub(crate) fn generate_unique_local_path(
    local_path: &str,
    is_directory: bool,
) -> AppResult<String> {
    if !local_path_exists(local_path)? {
        return Ok(local_path.to_string());
    }
    for index in 1..=999 {
        let candidate = local_rename_candidate(local_path, is_directory, index);
        if !local_path_exists(&candidate)? {
            return Ok(candidate);
        }
    }
    Err(AppError::Channel(format!(
        "No available download name for '{local_path}'"
    )))
}

fn reserve_renamed_local_download_target(
    local_path: &str,
    is_directory: bool,
) -> AppResult<LocalDownloadTarget> {
    for index in 1..=999 {
        let candidate = local_rename_candidate(local_path, is_directory, index);
        if let Some(target) = reserve_local_download_target(&candidate, is_directory, false)? {
            return Ok(target);
        }
    }
    Err(AppError::Channel(format!(
        "No available download name for '{local_path}'"
    )))
}

pub(crate) async fn resolve_local_path_with_prompt(
    app: &tauri::AppHandle,
    session_manager: &crate::core::SessionManager,
    session_id: &str,
    local_path: &str,
    remote_path: &str,
    strategy: &str,
    is_directory: bool,
    transfer_id: Option<&str>,
) -> AppResult<Option<LocalDownloadTarget>> {
    use super::duplicate::DuplicateChoice;

    let controller = transfer_id.and_then(find_transfer);
    if let Some(controller) = &controller {
        wait_for_transfer_ready(controller).await?;
    }
    if let Some(target) = reserve_local_download_target(local_path, is_directory, false)? {
        return Ok(Some(target));
    }
    let choice = match strategy {
        "skip" => DuplicateChoice::Skip,
        "rename" => DuplicateChoice::Rename,
        "overwrite" => DuplicateChoice::Overwrite,
        "ask" => {
            let file_name = remote_path
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or(remote_path);
            super::duplicate::prompt_duplicate_choice_with_destination(
                app,
                session_manager,
                session_id,
                local_path,
                file_name,
                is_directory,
                Some("local"),
                controller.as_ref(),
            )
            .await?
        }
        _ => {
            return Err(AppError::Config(format!(
                "Invalid duplicate strategy: {strategy}"
            )));
        }
    };
    if let Some(controller) = &controller {
        wait_for_transfer_ready(controller).await?;
    }
    match choice {
        DuplicateChoice::Skip => Ok(None),
        DuplicateChoice::Overwrite => reserve_local_download_target(local_path, is_directory, true),
        DuplicateChoice::Rename => {
            reserve_renamed_local_download_target(local_path, is_directory).map(Some)
        }
    }
}

pub(crate) fn emit_skipped_download(
    app: &tauri::AppHandle,
    session_id: &str,
    remote_path: &str,
    local_path: &str,
    transfer_id: Option<String>,
    is_directory: bool,
) {
    let event = build_skipped_download_event(
        session_id,
        remote_path,
        local_path,
        transfer_id,
        is_directory,
    );
    let _ = app.emit("transfer-event", &event);
}

fn build_skipped_download_event(
    session_id: &str,
    remote_path: &str,
    local_path: &str,
    transfer_id: Option<String>,
    is_directory: bool,
) -> TransferEvent {
    let controller = TransferController::new_with_kind(
        transfer_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
        session_id.to_string(),
        file_name_from_path(remote_path),
        remote_path.to_string(),
        local_path.to_string(),
        "download".to_string(),
        if is_directory { "directory" } else { "file" }.to_string(),
        None,
        if is_directory { Some(0) } else { None },
        if is_directory { Some(0) } else { None },
    );
    // 跳过并未产生下载文件，不登记目标历史，也不能作为下载完成上报。
    controller.build_event("skipped", 0, None)
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct LocalDirectoryStats {
    pub(crate) file_count: u64,
    pub(crate) total_size: u64,
}

pub(crate) async fn collect_local_directory_stats(
    local_path: &str,
) -> AppResult<LocalDirectoryStats> {
    let mut stats = LocalDirectoryStats {
        file_count: 0,
        total_size: 0,
    };
    let mut stack = vec![PathBuf::from(local_path)];

    while let Some(path) = stack.pop() {
        let mut read_dir = tokio::fs::read_dir(&path)
            .await
            .map_err(|e| AppError::Channel(format!("Failed to read local dir: {}", e)))?;

        while let Some(entry) = read_dir
            .next_entry()
            .await
            .map_err(|e| AppError::Channel(format!("Failed to read dir entry: {}", e)))?
        {
            let file_type = entry
                .file_type()
                .await
                .map_err(|e| AppError::Channel(format!("Failed to get file type: {}", e)))?;
            if file_type.is_dir() {
                stack.push(entry.path());
            } else if file_type.is_file() {
                let metadata = entry.metadata().await.map_err(|e| {
                    AppError::Channel(format!("Failed to read file metadata: {}", e))
                })?;
                stats.file_count = stats.file_count.saturating_add(1);
                stats.total_size = stats.total_size.saturating_add(metadata.len());
            }
        }
    }

    Ok(stats)
}

#[cfg(test)]
#[path = "transfer_tests.rs"]
mod tests;
