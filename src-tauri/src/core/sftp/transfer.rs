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
    /// "started", "progress", "paused", "resumed", "completed", "cancelled", or "error"
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
        Self {
            runtime: Mutex::new(TransferRuntime {
                id,
                session_id,
                file_name,
                remote_path,
                local_path,
                direction,
                kind,
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
        let mut runtime = self.runtime.lock().unwrap();
        runtime.bytes_transferred = bytes_transferred;
        runtime.total_size = total_size;
    }

    pub(crate) fn update_item_progress(&self, completed: u64, total: u64) {
        let mut runtime = self.runtime.lock().unwrap();
        runtime.item_count_completed = Some(completed);
        runtime.item_count_total = Some(total);
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
    ACTIVE_TRANSFERS
        .lock()
        .unwrap()
        .insert(controller.id(), controller);
}

pub(crate) fn unregister_transfer(id: &str) {
    let removed = ACTIVE_TRANSFERS.lock().unwrap().remove(id);
    if let Some(controller) = removed {
        remember_transfer_target(id.to_string(), controller.target_snapshot());
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

pub(crate) fn create_directory_transfer_controller(
    session_id: &str,
    display_name: String,
    remote_path: &str,
    local_path: &str,
    direction: &str,
    item_count_total: u64,
) -> Arc<TransferController> {
    Arc::new(TransferController::new_with_kind(
        uuid::Uuid::new_v4().to_string(),
        session_id.to_string(),
        display_name,
        remote_path.to_string(),
        local_path.to_string(),
        direction.to_string(),
        "directory".to_string(),
        None,
        Some(item_count_total),
        Some(0),
    ))
}

pub(crate) fn create_child_file_transfer_controller(
    session_id: &str,
    file_name: String,
    remote_path: &str,
    local_path: &str,
    direction: &str,
    parent_id: Option<String>,
) -> Arc<TransferController> {
    Arc::new(TransferController::new_with_kind(
        uuid::Uuid::new_v4().to_string(),
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
                return Err(AppError::Cancelled(TRANSFER_CANCELLED_MESSAGE.to_string()))
            }
            TransferControlState::Paused => notified.await,
        }
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

/// Resolve the actual local path, applying duplicate strategy.
pub(crate) fn resolve_local_path(local_path: &str, strategy: &str) -> Option<String> {
    let path = std::path::Path::new(local_path);
    if !path.exists() {
        return Some(local_path.to_string());
    }
    match strategy {
        "skip" => None,
        "rename" => {
            let stem = path.file_stem().unwrap_or_default().to_string_lossy();
            let ext = path
                .extension()
                .map(|e| format!(".{}", e.to_string_lossy()))
                .unwrap_or_default();
            let parent = path.parent().unwrap_or(std::path::Path::new("."));
            for i in 1..=999 {
                let candidate = parent.join(format!("{}({}){}", stem, i, ext));
                if !candidate.exists() {
                    return Some(candidate.to_string_lossy().to_string());
                }
            }
            Some(local_path.to_string())
        }
        _ => Some(local_path.to_string()),
    }
}

pub(crate) async fn count_local_files(local_path: &str) -> AppResult<u64> {
    let mut count = 0;
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
                count += 1;
            }
        }
    }

    Ok(count)
}
