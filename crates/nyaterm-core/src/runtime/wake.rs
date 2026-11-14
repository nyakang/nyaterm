//! Edge-triggered wake for a queue that coalesces.
//!
//! Several application queues merge or drop entries as they are pushed: the
//! terminal frame queue compacts consecutive output, the RDP and VNC session
//! queues keep only the newest frame, and the credential-autofill queue keeps
//! only the latest match per prompt. None of them can be replaced by a channel
//! without losing that behaviour, so the queue stays where it is and only the
//! *signal* moves here.
//!
//! [`EventWake::arm`] declares interest; the first [`EventWake::signal`] after an
//! arm sends exactly one wake and clears the interest again. A producer flooding
//! the queue therefore costs one wake per drain cycle rather than one per entry,
//! which is the property that makes this usable on the paths where a poll would
//! otherwise be the only affordable option.
//!
//! Interests are a bitmask so a consumer waiting for one kind of entry is not
//! woken -- and so disarmed -- by another kind arriving. A queue carrying one
//! kind can use [`ANY_INTEREST`] throughout.
//!
//! # Ordering: arm before checking for work
//!
//! A consumer must arm *before* it looks for work, not after:
//!
//! ```ignore
//! loop {
//!     wake.arm(ANY_INTEREST);
//!     if drain_found_work() {
//!         continue;
//!     }
//!     if wake_rx.next().await.is_none() {
//!         break;
//!     }
//! }
//! ```
//!
//! Checking first and arming afterwards loses a producer that pushed in between:
//! the check saw an empty queue, the arm came too late to make that push signal,
//! and the consumer then sleeps on a queue that already has work in it until
//! something unrelated happens to arrive. Arming first can instead produce one
//! redundant wake -- the next iteration drains nothing and goes back to sleep --
//! which is harmless.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};

use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};

/// Every interest at once, for a queue that carries a single kind of entry.
pub const ANY_INTEREST: u8 = u8::MAX;

/// The producer half. Cheap to clone, and safe to hold on a worker thread:
/// `signal` never blocks and never allocates.
#[derive(Clone, Debug)]
pub struct EventWake {
    tx: UnboundedSender<()>,
    interests: Arc<AtomicU8>,
    signal_count: Arc<AtomicU64>,
}

impl EventWake {
    /// How many wakes have been sent, for asserting the coalescing contract.
    pub fn signal_count(&self) -> u64 {
        self.signal_count.load(Ordering::Relaxed)
    }

    pub fn new() -> (Self, UnboundedReceiver<()>) {
        let (tx, rx) = unbounded();
        (
            Self {
                tx,
                interests: Arc::new(AtomicU8::new(0)),
                signal_count: Arc::new(AtomicU64::new(0)),
            },
            rx,
        )
    }

    /// Declare interest in the next entry matching `interest`.
    ///
    /// Call this before checking the queue for work; see the module docs for why
    /// the other order loses wakes.
    pub fn arm(&self, interest: u8) {
        if interest != 0 {
            self.interests.fetch_or(interest, Ordering::Release);
        }
    }

    /// Report that an entry matching `interest` was pushed.
    ///
    /// Sends a wake only if the consumer had armed for this interest, and clears
    /// that interest so a burst produces one wake rather than one per entry.
    /// Returns whether a wake was actually sent.
    pub fn signal(&self, interest: u8) -> bool {
        if interest == 0 {
            return false;
        }
        if self.interests.fetch_and(!interest, Ordering::AcqRel) & interest == 0 {
            return false;
        }
        if self.tx.unbounded_send(()).is_err() {
            return false;
        }
        self.signal_count.fetch_add(1, Ordering::Relaxed);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::{ANY_INTEREST, EventWake};

    const OUTPUT: u8 = 1 << 0;
    const SNAPSHOT: u8 = 1 << 1;

    fn queued(rx: &mut futures::channel::mpsc::UnboundedReceiver<()>) -> usize {
        let mut count = 0;
        while rx.try_recv().is_ok() {
            count += 1;
        }
        count
    }

    #[test]
    fn a_signal_without_an_arm_sends_nothing() {
        let (wake, mut rx) = EventWake::new();

        assert!(!wake.signal(ANY_INTEREST));
        assert_eq!(queued(&mut rx), 0);
        assert_eq!(wake.signal_count(), 0);
    }

    #[test]
    fn a_burst_after_one_arm_sends_exactly_one_wake() {
        let (wake, mut rx) = EventWake::new();

        wake.arm(ANY_INTEREST);
        assert!(wake.signal(ANY_INTEREST));
        for _ in 0..64 {
            assert!(
                !wake.signal(ANY_INTEREST),
                "a flood must not queue one wake per entry"
            );
        }

        assert_eq!(queued(&mut rx), 1);
        assert_eq!(wake.signal_count(), 1);
    }

    #[test]
    fn rearming_allows_the_next_wake() {
        let (wake, mut rx) = EventWake::new();

        wake.arm(ANY_INTEREST);
        assert!(wake.signal(ANY_INTEREST));
        wake.arm(ANY_INTEREST);
        assert!(wake.signal(ANY_INTEREST));

        assert_eq!(queued(&mut rx), 2);
    }

    #[test]
    fn one_interest_is_not_disarmed_by_another() {
        let (wake, mut rx) = EventWake::new();

        wake.arm(OUTPUT);
        assert!(
            !wake.signal(SNAPSHOT),
            "a snapshot must not consume an arm for output"
        );
        assert!(
            wake.signal(OUTPUT),
            "the output arm must still be live after an unrelated signal"
        );

        assert_eq!(queued(&mut rx), 1);
    }

    #[test]
    fn a_signal_matching_any_armed_interest_wakes() {
        let (wake, mut rx) = EventWake::new();

        wake.arm(OUTPUT | SNAPSHOT);
        assert!(wake.signal(SNAPSHOT));
        assert!(
            wake.signal(OUTPUT),
            "signalling one interest must leave the others armed"
        );

        assert_eq!(queued(&mut rx), 2);
    }

    #[test]
    fn a_dropped_receiver_stops_reporting_wakes() {
        let (wake, rx) = EventWake::new();
        drop(rx);

        wake.arm(ANY_INTEREST);
        assert!(!wake.signal(ANY_INTEREST));
        assert_eq!(wake.signal_count(), 0);
    }
}
