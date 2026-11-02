use std::collections::{VecDeque, hash_map::RandomState};
use std::hash::{BuildHasher, Hash, Hasher};
use std::time::{Duration, Instant};

use super::{ACTIVATION_QUEUE_CAPACITY, ExternalConnectionRequest};

/// Process-local keyed fingerprints. Raw links and credentials are never retained.
#[derive(Default)]
pub struct RecentConnectionRequests {
    recent: VecDeque<(u64, Instant)>,
    hasher: RandomState,
}

impl RecentConnectionRequests {
    pub fn remember(&mut self, request: &ExternalConnectionRequest, now: Instant) -> bool {
        self.recent
            .retain(|(_, time)| now.saturating_duration_since(*time) <= Duration::from_millis(500));
        let mut hasher = self.hasher.build_hasher();
        std::mem::discriminant(request).hash(&mut hasher);
        match request {
            ExternalConnectionRequest::Local { cwd } => cwd.hash(&mut hasher),
            ExternalConnectionRequest::Telnet { host, port } => {
                host.hash(&mut hasher);
                port.hash(&mut hasher);
            }
            ExternalConnectionRequest::Ssh {
                host,
                port,
                username,
                password,
                runtime_mode,
            } => {
                host.hash(&mut hasher);
                port.hash(&mut hasher);
                username.hash(&mut hasher);
                password
                    .as_ref()
                    .map(|password| password.expose_secret())
                    .hash(&mut hasher);
                runtime_mode.hash(&mut hasher);
            }
        }
        let fingerprint = hasher.finish();
        if self.recent.iter().any(|(key, _)| *key == fingerprint) {
            return false;
        }
        self.recent.push_back((fingerprint, now));
        while self.recent.len() > ACTIVATION_QUEUE_CAPACITY * 4 {
            self.recent.pop_front();
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::RecentConnectionRequests;
    use crate::activation::{ActivationAction, parse_deep_link};
    use std::time::{Duration, Instant};

    #[test]
    fn startup_and_callback_duplicates_expire_and_distinct_credentials_stay_distinct() {
        let connection = |text| match parse_deep_link(text).unwrap() {
            ActivationAction::Connect(request) => request,
            _ => panic!("expected connection"),
        };
        let first = connection("ssh://user:first@host");
        let second = connection("ssh://user:second@host");
        let mut cache = RecentConnectionRequests::default();
        let now = Instant::now();
        assert!(cache.remember(&first, now));
        assert!(!cache.remember(&first, now + Duration::from_millis(499)));
        assert!(cache.remember(&second, now + Duration::from_millis(499)));
        assert!(cache.remember(&first, now + Duration::from_millis(501)));
    }
}
