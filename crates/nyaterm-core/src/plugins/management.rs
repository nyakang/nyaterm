//! Transient management identities and progress; never serialized into plugin packages.
use crate::plugins::native::NativeComponentId;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PluginKey {
    Native(NativeComponentId),
    Wasm(String),
}

impl PluginKey {
    pub fn id(&self) -> String {
        match self {
            Self::Native(id) => id.plugin_id(),
            Self::Wasm(id) => id.clone(),
        }
    }

    pub fn from_id(id: &str) -> Self {
        NativeComponentId::ALL
            .into_iter()
            .find(|component| component.plugin_id() == id)
            .map(Self::Native)
            .unwrap_or_else(|| Self::Wasm(id.to_owned()))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperationPhase {
    Queued,
    Checking,
    Downloading,
    Verifying,
    Installing,
    Committing,
}

impl OperationPhase {
    pub fn cancellable(self) -> bool {
        !matches!(self, Self::Committing)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstallProgress {
    pub phase: OperationPhase,
    pub downloaded: u64,
    pub total: Option<u64>,
}

impl InstallProgress {
    pub fn new(phase: OperationPhase) -> Self {
        Self {
            phase,
            downloaded: 0,
            total: None,
        }
    }

    pub fn ratio(&self) -> Option<f64> {
        self.total
            .filter(|total| *total > 0)
            .map(|total| (self.downloaded as f64 / total as f64).clamp(0., 1.))
    }
}

#[cfg(test)]
mod tests {
    use crate::plugins::management::{InstallProgress, OperationPhase, PluginKey};
    use crate::plugins::native::NativeComponentId;

    #[test]
    fn native_and_extension_identities_round_trip_without_colliding() {
        for id in NativeComponentId::ALL {
            let key = PluginKey::Native(id);
            assert_eq!(PluginKey::from_id(&key.id()), key);
        }
        let key = PluginKey::Wasm("diagnostics".into());
        assert_eq!(PluginKey::from_id(&key.id()), key);
    }

    #[test]
    fn unknown_or_zero_download_size_has_no_percentage_and_commit_cannot_cancel() {
        let mut progress = InstallProgress::new(OperationPhase::Downloading);
        assert_eq!(progress.ratio(), None);
        progress.total = Some(0);
        assert_eq!(progress.ratio(), None);
        progress.total = Some(100);
        progress.downloaded = 64;
        assert_eq!(progress.ratio(), Some(0.64));
        progress.downloaded = 101;
        assert_eq!(progress.ratio(), Some(1.));
        assert!(progress.phase.cancellable());
        assert!(!OperationPhase::Committing.cancellable());
    }
}
