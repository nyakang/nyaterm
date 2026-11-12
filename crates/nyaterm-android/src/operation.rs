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
    deadline: Option<Instant>,
}
impl AdbOperationControl {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
            || self
                .parent
                .as_ref()
                .is_some_and(|parent| parent.is_cancelled())
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
        Ok(())
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
    pub fn remaining(&self, maximum: Duration) -> Duration {
        let own = self.deadline.map_or(maximum, |deadline| {
            deadline
                .saturating_duration_since(Instant::now())
                .min(maximum)
        });
        self.parent
            .as_ref()
            .map_or(own, |parent| parent.remaining(own))
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
}
