//! File conflict prompts and the scope of one multi-item upload operation.

use super::sftp_duplicate_prompt_id;
use crate::models::event_wake::{ANY_INTEREST, EventWake};
use nyaterm_files::sftp_transfer_types::{
    SftpDuplicateDecision, SftpDuplicateRequest, SftpDuplicateResolver, SftpTransferDirection,
};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

/// Only the broker changes this flag, under its pending-queue lock. The same
/// lock serializes enqueueing with accepting all remaining conflicts.
#[derive(Debug, Default)]
pub(in crate::features) struct UploadBatchDuplicateContext {
    overwrite_remaining: AtomicBool,
}

#[derive(Debug, Clone, Copy)]
pub(in crate::features) enum DuplicatePromptAction {
    Current(SftpDuplicateDecision),
    OverwriteUploadBatch,
}

/// Sibling uploads share this resolver, while their path caches stay independent.
pub(in crate::features) struct UploadBatchDuplicateResolver {
    broker: Arc<SftpDuplicatePromptBroker>,
    batch: Arc<UploadBatchDuplicateContext>,
}

impl UploadBatchDuplicateResolver {
    pub(in crate::features) fn new(broker: Arc<SftpDuplicatePromptBroker>) -> Self {
        Self {
            broker,
            batch: Arc::default(),
        }
    }

    #[cfg(test)]
    pub(in crate::features) fn enqueue_for_test(
        &self,
        request: SftpDuplicateRequest,
    ) -> mpsc::Receiver<SftpDuplicateDecision> {
        self.broker
            .enqueue_batch_request(request, Some(self.batch.clone()))
            .unwrap()
    }
}

impl SftpDuplicateResolver for UploadBatchDuplicateResolver {
    fn resolve_duplicate(
        &self,
        request: &SftpDuplicateRequest,
    ) -> Result<SftpDuplicateDecision, String> {
        let batch =
            (request.direction == SftpTransferDirection::Upload).then(|| self.batch.clone());
        self.broker
            .enqueue_batch_request(request.clone(), batch)?
            .recv_timeout(Duration::from_secs(300))
            .map_err(|_| "remote transfer duplicate prompt timed out".to_string())
    }
}

#[derive(Debug)]
pub(in crate::features) struct SftpDuplicatePromptRequest {
    pub(in crate::features) id: String,
    pub(in crate::features) request: SftpDuplicateRequest,
    pub(in crate::features) response_tx: mpsc::Sender<SftpDuplicateDecision>,
    pub(in crate::features) upload_batch: Option<Arc<UploadBatchDuplicateContext>>,
}

#[derive(Debug, Clone)]
pub(in crate::features) struct SftpDuplicatePromptState {
    pub(in crate::features) id: String,
    pub(in crate::features) request: SftpDuplicateRequest,
    pub(in crate::features) response_tx: mpsc::Sender<SftpDuplicateDecision>,
    pub(in crate::features) upload_batch: Option<Arc<UploadBatchDuplicateContext>>,
}

#[derive(Debug, Default)]
pub(in crate::features) struct SftpDuplicatePromptBroker {
    pending: Mutex<VecDeque<SftpDuplicatePromptRequest>>,
    /// 同一文件可能在连续上传中重复出现；序号让每个对话框都有独立生命周期。
    next_id: AtomicU64,
    /// Signalled after a transport thread enqueues, so activation does not
    /// have to be polled. `Option` because the brokers are `Default`-built,
    /// including in tests that drive them without a wake at all.
    wake: Mutex<Option<EventWake>>,
}

impl SftpDuplicatePromptBroker {
    pub(in crate::features) fn set_wake(&self, wake: EventWake) {
        if let Ok(mut slot) = self.wake.lock() {
            *slot = Some(wake);
        }
    }

    fn signal_wake(&self) {
        if let Ok(slot) = self.wake.lock()
            && let Some(wake) = slot.as_ref()
        {
            wake.signal(ANY_INTEREST);
        }
    }

    fn request_decision(
        &self,
        request: SftpDuplicateRequest,
    ) -> Result<SftpDuplicateDecision, String> {
        self.enqueue_decision_request(request)?
            .recv_timeout(Duration::from_secs(300))
            .map_err(|_| "remote transfer duplicate prompt timed out".to_string())
    }

    /// 完成入队和唤醒后才返回 receiver，调用方不会观察到只入队、尚未唤醒的中间态。
    fn enqueue_decision_request(
        &self,
        request: SftpDuplicateRequest,
    ) -> Result<mpsc::Receiver<SftpDuplicateDecision>, String> {
        self.enqueue_batch_request(request, None)
    }

    fn enqueue_batch_request(
        &self,
        request: SftpDuplicateRequest,
        upload_batch: Option<Arc<UploadBatchDuplicateContext>>,
    ) -> Result<mpsc::Receiver<SftpDuplicateDecision>, String> {
        let (response_tx, response_rx) = mpsc::channel();
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| "remote transfer duplicate prompt queue is poisoned".to_string())?;
        if upload_batch
            .as_ref()
            .is_some_and(|batch| batch.overwrite_remaining.load(Ordering::Relaxed))
        {
            let _ = response_tx.send(SftpDuplicateDecision::Overwrite);
            return Ok(response_rx);
        }
        let prompt_id = format!(
            "{}-{}",
            sftp_duplicate_prompt_id(&request),
            self.next_id.fetch_add(1, Ordering::Relaxed)
        );
        let request = SftpDuplicatePromptRequest {
            id: prompt_id,
            request,
            response_tx,
            upload_batch,
        };
        pending.push_back(request);
        drop(pending);
        self.signal_wake();
        Ok(response_rx)
    }

    pub(in crate::features) fn resolve(
        &self,
        prompt: SftpDuplicatePromptState,
        action: DuplicatePromptAction,
    ) -> Result<SftpDuplicateDecision, String> {
        let decision = match action {
            DuplicatePromptAction::Current(decision) => decision,
            DuplicatePromptAction::OverwriteUploadBatch => {
                let Some(batch) = &prompt.upload_batch else {
                    // A stale or invalid batch action must not widen its scope.
                    let _ = prompt.response_tx.send(SftpDuplicateDecision::Skip);
                    return Err("duplicate prompt does not belong to an upload batch".to_string());
                };
                let mut pending = self.pending.lock().map_err(|_| {
                    "remote transfer duplicate prompt queue is poisoned".to_string()
                })?;
                batch.overwrite_remaining.store(true, Ordering::Relaxed);
                pending.retain(|request| {
                    if request
                        .upload_batch
                        .as_ref()
                        .is_some_and(|other| Arc::ptr_eq(batch, other))
                    {
                        let _ = request.response_tx.send(SftpDuplicateDecision::Overwrite);
                        false
                    } else {
                        true
                    }
                });
                SftpDuplicateDecision::Overwrite
            }
        };
        let _ = prompt.response_tx.send(decision);
        self.signal_wake();
        Ok(decision)
    }

    #[cfg(test)]
    pub(in crate::features) fn enqueue_decision_request_for_test(
        &self,
        request: SftpDuplicateRequest,
    ) -> Result<mpsc::Receiver<SftpDuplicateDecision>, String> {
        self.enqueue_decision_request(request)
    }

    pub(in crate::features) fn pop_pending(&self) -> Option<SftpDuplicatePromptRequest> {
        self.pending
            .lock()
            .ok()
            .and_then(|mut pending| pending.pop_front())
    }

    pub(in crate::features) fn has_pending(&self) -> bool {
        self.pending
            .lock()
            .ok()
            .is_some_and(|pending| !pending.is_empty())
    }
}

impl SftpDuplicateResolver for SftpDuplicatePromptBroker {
    fn resolve_duplicate(
        &self,
        request: &SftpDuplicateRequest,
    ) -> Result<SftpDuplicateDecision, String> {
        self.request_decision(request.clone())
    }
}

#[cfg(test)]
mod tests;
