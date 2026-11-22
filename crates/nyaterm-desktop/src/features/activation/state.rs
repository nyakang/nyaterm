use std::collections::VecDeque;

use nyaterm_core::SavedConnection;
use nyaterm_core::activation::ExternalConnectionRequest;
use nyaterm_core::models::ssh_runtime::SshRuntimeMode;

#[derive(Clone)]
pub(super) enum ExternalDialog {
    Spice {
        imported: std::sync::Arc<super::spice_file::ImportedSpiceFile>,
        delete_source: bool,
        fullscreen: bool,
    },
    Choose {
        request: ExternalConnectionRequest,
        connections: Vec<SavedConnection>,
    },
    Confirm {
        connection: Box<SavedConnection>,
        mode: Option<SshRuntimeMode>,
    },
}

#[derive(Default)]
pub(in crate::features) struct ActivationFeatureState {
    pub(super) pending: VecDeque<ExternalConnectionRequest>,
    pub(super) dialog: Option<ExternalDialog>,
    pub(super) clock_armed: bool,
    pub(super) waiting_for_start: bool,
    pub(super) spice_loading: Option<String>,
}

impl ActivationFeatureState {
    pub(super) fn enqueue(&mut self, request: ExternalConnectionRequest) -> bool {
        if self.pending.len() >= nyaterm_core::activation::ACTIVATION_QUEUE_CAPACITY {
            return false;
        }
        self.pending.push_back(request);
        true
    }

    pub(super) fn is_empty(&self) -> bool {
        self.pending.is_empty()
            && self.dialog.is_none()
            && !self.waiting_for_start
            && self.spice_loading.is_none()
    }
    pub(in crate::features) fn clear(&mut self) {
        self.pending.clear();
        self.dialog = None;
        self.waiting_for_start = false;
        self.spice_loading = None;
    }
}

#[cfg(test)]
mod tests {
    use super::ActivationFeatureState;
    use nyaterm_core::activation::{ACTIVATION_QUEUE_CAPACITY, ExternalConnectionRequest};
    use nyaterm_core::secret::SecretString;

    fn request(password: &str) -> ExternalConnectionRequest {
        ExternalConnectionRequest::Ssh {
            host: "host".into(),
            port: 22,
            username: None,
            password: Some(SecretString::new(password)),
            runtime_mode: None,
        }
    }

    #[test]
    fn capacity_failure_preserves_queued_requests_and_close_releases_everything() {
        let mut state = ActivationFeatureState::default();
        for index in 0..ACTIVATION_QUEUE_CAPACITY {
            assert!(state.enqueue(request(&index.to_string())));
        }
        assert!(!state.enqueue(request("overflow")));
        assert_eq!(state.pending.len(), ACTIVATION_QUEUE_CAPACITY);
        state.waiting_for_start = true;
        state.clear();
        assert!(state.is_empty());
    }
}
