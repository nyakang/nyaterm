//! Cancellation and deadlines injected by runtime owners, without UI dependencies.
use crate::error::{AdbError, AdbResult};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

#[derive(Clone, Default)]
pub struct AdbOperationControl {
    cancelled: Arc<AtomicBool>,
    parent: Option<Arc<Self>>,
    linked: Option<Arc<Self>>,
    deadline: Option<Instant>,
    external_cancelled: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
}
impl AdbOperationControl {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
    pub fn is_cancelled(&self) -> bool {
        self.external_cancelled
            .as_ref()
            .is_some_and(|check| check())
            || self.cancelled.load(Ordering::Acquire)
            || self
                .parent
                .as_ref()
                .is_some_and(|parent| parent.is_cancelled())
            || self
                .linked
                .as_ref()
                .is_some_and(|linked| linked.is_cancelled())
    }
    pub fn check(&self) -> AdbResult<()> {
        if self.is_cancelled() {
            return Err(AdbError::Cancelled);
        }
        if self
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            return Err(AdbError::Timeout);
        }
        if let Some(parent) = &self.parent {
            parent.check()?;
        }
        if let Some(linked) = &self.linked {
            linked.check()?;
        }
        Ok(())
    }
    /// Inject an owner's cancellation without depending on its runtime types.
    pub fn with_cancellation(&self, cancelled: impl Fn() -> bool + Send + Sync + 'static) -> Self {
        Self {
            parent: Some(Arc::new(self.clone())),
            external_cancelled: Some(Arc::new(cancelled)),
            ..Self::default()
        }
    }
    pub fn child(&self) -> Self {
        Self {
            parent: Some(Arc::new(self.clone())),
            ..Self::default()
        }
    }
    pub fn with_timeout(&self, timeout: Duration) -> Self {
        Self {
            parent: Some(Arc::new(self.clone())),
            deadline: Some(Instant::now() + timeout),
            ..Self::default()
        }
    }
    pub(crate) fn linked_to(&self, other: &Self) -> Self {
        Self {
            parent: Some(Arc::new(self.clone())),
            linked: Some(Arc::new(other.clone())),
            ..Self::default()
        }
    }
    pub fn remaining(&self, maximum: Duration) -> Duration {
        let own = self.deadline.map_or(maximum, |deadline| {
            deadline
                .saturating_duration_since(Instant::now())
                .min(maximum)
        });
        let remaining = self
            .parent
            .as_ref()
            .map_or(own, |parent| parent.remaining(own));
        self.linked
            .as_ref()
            .map_or(remaining, |linked| linked.remaining(remaining))
    }
    pub(crate) async fn until_stopped(&self) -> AdbError {
        loop {
            if let Err(error) = self.check() {
                return error;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }
}
impl std::fmt::Debug for AdbOperationControl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdbOperationControl")
            .field("cancelled", &self.is_cancelled())
            .finish_non_exhaustive()
    }
}
#[cfg(test)]
mod tests {
    use crate::error::AdbError;
    use crate::operation::AdbOperationControl;
    use std::time::Duration;
    #[test]
    fn children_share_device_cancellation_and_keep_independent_deadlines() {
        let parent = AdbOperationControl::default();
        let child = parent.with_timeout(Duration::ZERO);
        assert!(matches!(child.check(), Err(AdbError::Timeout)));
        assert!(parent.check().is_ok());
        parent.cancel();
        assert!(matches!(child.check(), Err(AdbError::Cancelled)));
    }

    #[test]
    fn linked_controls_stop_for_either_owner_and_keep_the_earliest_deadline() {
        let device = AdbOperationControl::default();
        let application = AdbOperationControl::default();
        let operation = device.linked_to(&application);
        assert!(operation.check().is_ok());
        application.cancel();
        assert!(matches!(operation.check(), Err(AdbError::Cancelled)));
        let application = AdbOperationControl::default();
        let operation = device.linked_to(&application);
        device.cancel();
        assert!(matches!(operation.check(), Err(AdbError::Cancelled)));
        let device = AdbOperationControl::default().with_timeout(Duration::ZERO);
        let operation = application
            .linked_to(&device)
            .with_timeout(Duration::from_secs(15));
        assert!(matches!(operation.check(), Err(AdbError::Timeout)));
        assert_eq!(operation.remaining(Duration::from_secs(15)), Duration::ZERO);
    }
}
