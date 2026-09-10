use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};

/// One dial attempt, shared by the pending tab, jump chain and authentication prompts.
#[derive(Clone, Debug, Default)]
pub struct ConnectionAttempt(Arc<AtomicBool>);

impl ConnectionAttempt {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
    pub fn check(&self) -> Result<(), String> {
        if self.is_cancelled() {
            Err("connection attempt cancelled".into())
        } else {
            Ok(())
        }
    }
    pub async fn until_cancelled<T>(
        &self,
        operation: impl std::future::Future<Output = T>,
    ) -> Result<T, String> {
        self.check()?;
        tokio::select! {
            result = operation => { self.check()?; Ok(result) },
            _ = async { while !self.is_cancelled() { tokio::time::sleep(Duration::from_millis(25)).await; } } => Err("connection attempt cancelled".into()),
        }
    }
    pub fn receive<T>(&self, receiver: &mpsc::Receiver<T>, timeout: Duration) -> Result<T, String> {
        let deadline = Instant::now() + timeout;
        loop {
            self.check()?;
            if Instant::now() >= deadline {
                return Err("connection prompt timed out".into());
            }
            match receiver.recv_timeout(Duration::from_millis(25)) {
                Ok(value) => {
                    self.check()?;
                    return Ok(value);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("connection prompt closed".into());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ConnectionAttempt;
    use std::time::Duration;

    #[tokio::test]
    async fn cancellation_interrupts_a_pending_dial() {
        let attempt = ConnectionAttempt::default();
        let cancel = attempt.clone();
        let task =
            tokio::spawn(
                async move { attempt.until_cancelled(std::future::pending::<()>()).await },
            );
        cancel.cancel();
        assert!(
            tokio::time::timeout(Duration::from_secs(1), task)
                .await
                .unwrap()
                .unwrap()
                .is_err()
        );
    }
}
