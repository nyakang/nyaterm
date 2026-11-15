//! Shared, time-injected network sampling for native and SSH collectors.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use super::{RemoteStats, update_network_summary};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum NetworkRateStatus {
    #[default]
    WarmingUp,
    Available,
    CounterReset,
    Interrupted,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkSampleTime {
    pub sampled_at: Option<Instant>,
    pub interval: Duration,
}

impl Default for NetworkSampleTime {
    fn default() -> Self {
        Self {
            sampled_at: None,
            interval: Duration::from_secs(3),
        }
    }
}

pub fn network_stale_after(interval: Duration) -> Duration {
    interval.max(Duration::from_secs(1)).saturating_mul(3)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NetworkCounters {
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

#[derive(Debug, Default, Clone)]
pub(crate) struct NetworkRateSampler {
    counters: BTreeMap<String, NetworkCounters>,
    time: NetworkSampleTime,
}

impl NetworkRateSampler {
    pub(crate) fn sample(
        &mut self,
        mut counters: BTreeMap<String, NetworkCounters>,
        stats: &mut RemoteStats,
        now: Instant,
        interval: Duration,
    ) {
        let elapsed = self
            .time
            .sampled_at
            .map(|at| now.saturating_duration_since(at));
        let maximum_gap = network_stale_after(interval.max(self.time.interval));
        for network in &mut stats.networks {
            // Legacy NETWORK records already contain rates, rather than counters.
            let Some(current) = counters.get(&network.nic) else {
                continue;
            };
            network.rx_bytes_per_sec = 0.;
            network.tx_bytes_per_sec = 0.;
            network.rate_status = match (self.counters.get(&network.nic), elapsed) {
                (Some(_), Some(elapsed)) if elapsed > maximum_gap => NetworkRateStatus::Interrupted,
                (Some(previous), Some(elapsed)) if elapsed >= Duration::from_millis(100) => {
                    match (
                        current.rx_bytes.checked_sub(previous.rx_bytes),
                        current.tx_bytes.checked_sub(previous.tx_bytes),
                    ) {
                        (Some(rx), Some(tx)) => {
                            network.rx_bytes_per_sec = rx as f64 / elapsed.as_secs_f64();
                            network.tx_bytes_per_sec = tx as f64 / elapsed.as_secs_f64();
                            NetworkRateStatus::Available
                        }
                        _ => NetworkRateStatus::CounterReset,
                    }
                }
                (Some(_), Some(_)) => NetworkRateStatus::Unavailable,
                _ => NetworkRateStatus::WarmingUp,
            };
            if network.rate_status == NetworkRateStatus::CounterReset {
                // Do not use a suspect zero reading as a baseline: sysinfo can return
                // zero on a read error. The next reading establishes a new baseline.
                counters.remove(&network.nic);
            }
        }
        self.counters = counters;
        self.time = NetworkSampleTime {
            sampled_at: Some(now),
            interval,
        };
        stats.network_sample_time = self.time.clone();
        update_network_summary(stats);
    }
}

#[cfg(test)]
mod tests {
    use crate::stats::{NetworkRateStatus, RemoteStats, RemoteStatsSampler, parse_stats_output};
    use std::time::{Duration, Instant};

    fn sample(
        sampler: &RemoteStatsSampler,
        session: &str,
        at: Instant,
        interval: u64,
        counters: &[(&str, u64, u64)],
    ) -> RemoteStats {
        let output = counters
            .iter()
            .map(|(nic, rx, tx)| format!("NETDEV\t{nic}\tup\t{rx}\t{tx}\n"))
            .collect::<String>();
        sampler.complete_snapshot_at(
            session,
            parse_stats_output(&output).unwrap(),
            at,
            Duration::from_secs(interval),
        )
    }

    #[test]
    fn first_sample_is_missing_but_unchanged_counters_are_valid_zero() {
        let sampler = RemoteStatsSampler::default();
        let now = Instant::now();
        let first = sample(&sampler, "a", now, 3, &[("eth0", 100, 200)]);
        assert_eq!(first.networks[0].rate_status, NetworkRateStatus::WarmingUp);
        assert!(first.network_traffic().is_none());
        let zero = sample(
            &sampler,
            "a",
            now + Duration::from_secs(3),
            3,
            &[("eth0", 100, 200)],
        );
        assert_eq!(zero.network_traffic().unwrap().rx_bytes_per_sec, 0.);
        assert_eq!(zero.networks[0].rate_status, NetworkRateStatus::Available);
    }

    #[test]
    fn rates_use_real_elapsed_time_and_require_every_interface_for_summary() {
        let sampler = RemoteStatsSampler::default();
        let now = Instant::now();
        sample(&sampler, "a", now, 3, &[("eth0", 100, 200)]);
        let stats = sample(
            &sampler,
            "a",
            now + Duration::from_millis(2500),
            3,
            &[("eth0", 200, 400), ("eth1", 10, 20)],
        );
        assert_eq!(stats.networks[0].traffic().unwrap().rx_bytes_per_sec, 40.);
        assert_eq!(stats.networks[0].traffic().unwrap().tx_bytes_per_sec, 80.);
        assert!(stats.network_traffic().is_none());
        let next = sample(
            &sampler,
            "a",
            now + Duration::from_millis(4500),
            3,
            &[("eth0", 220, 440), ("eth1", 50, 80)],
        );
        assert_eq!(next.network_traffic().unwrap().rx_bytes_per_sec, 30.);
        assert_eq!(next.network_traffic().unwrap().tx_bytes_per_sec, 50.);
    }

    #[test]
    fn rollback_discards_the_suspect_baseline_and_recovery_cannot_spike() {
        let sampler = RemoteStatsSampler::default();
        let now = Instant::now();
        sample(&sampler, "a", now, 3, &[("eth0", 100000, 200000)]);
        let reset = sample(
            &sampler,
            "a",
            now + Duration::from_secs(3),
            3,
            &[("eth0", 0, 0)],
        );
        assert_eq!(
            reset.networks[0].rate_status,
            NetworkRateStatus::CounterReset
        );
        assert!(reset.network_traffic().is_none());
        let baseline = sample(
            &sampler,
            "a",
            now + Duration::from_secs(6),
            3,
            &[("eth0", 100100, 200100)],
        );
        assert!(baseline.network_traffic().is_none());
        let recovered = sample(
            &sampler,
            "a",
            now + Duration::from_secs(9),
            3,
            &[("eth0", 100130, 200160)],
        );
        assert_eq!(recovered.network_traffic().unwrap().rx_bytes_per_sec, 10.);
        assert_eq!(recovered.network_traffic().unwrap().tx_bytes_per_sec, 20.);
    }

    #[test]
    fn long_and_zero_intervals_are_missing_and_frequency_changes_use_both_schedules() {
        let sampler = RemoteStatsSampler::default();
        let now = Instant::now();
        sample(&sampler, "a", now, 10, &[("eth0", 0, 0)]);
        let shorter = sample(
            &sampler,
            "a",
            now + Duration::from_secs(10),
            1,
            &[("eth0", 100, 200)],
        );
        assert_eq!(shorter.network_traffic().unwrap().rx_bytes_per_sec, 10.);
        let delayed = sample(
            &sampler,
            "a",
            now + Duration::from_secs(20),
            1,
            &[("eth0", 1000, 2000)],
        );
        assert_eq!(
            delayed.networks[0].rate_status,
            NetworkRateStatus::Interrupted
        );
        assert!(delayed.network_traffic().is_none());
        let zero = sample(
            &sampler,
            "a",
            now + Duration::from_secs(20),
            1,
            &[("eth0", 1001, 2001)],
        );
        assert_eq!(zero.networks[0].rate_status, NetworkRateStatus::Unavailable);
        let recovered = sample(
            &sampler,
            "a",
            now + Duration::from_secs(21),
            1,
            &[("eth0", 1011, 2021)],
        );
        assert_eq!(recovered.network_traffic().unwrap().rx_bytes_per_sec, 10.);
    }

    #[test]
    fn disappearing_interfaces_and_sessions_never_reuse_another_baseline() {
        let sampler = RemoteStatsSampler::default();
        let now = Instant::now();
        sample(&sampler, "a", now, 3, &[("eth0", 100, 100)]);
        assert!(
            sample(&sampler, "b", now, 3, &[("eth0", 900, 900)])
                .network_traffic()
                .is_none()
        );
        sample(&sampler, "a", now + Duration::from_secs(3), 3, &[]);
        assert!(
            sample(
                &sampler,
                "a",
                now + Duration::from_secs(6),
                3,
                &[("eth0", 200, 200)]
            )
            .network_traffic()
            .is_none()
        );
        assert_eq!(
            sample(
                &sampler,
                "b",
                now + Duration::from_secs(6),
                3,
                &[("eth0", 960, 1020)]
            )
            .network_traffic()
            .unwrap()
            .tx_bytes_per_sec,
            20.
        );
        sampler.clear_session("b");
        assert!(
            sample(
                &sampler,
                "b",
                now + Duration::from_secs(9),
                3,
                &[("eth0", 990, 1030)]
            )
            .network_traffic()
            .is_none()
        );
    }

    #[test]
    fn legacy_rates_are_validated_and_runtime_metadata_does_not_change_serialized_fields() {
        for value in ["NaN", "inf", "-1", "broken"] {
            assert!(parse_stats_output(&format!("NETWORK\teth0\tup\t{value}\t0\n")).is_err());
        }
        let sampler = RemoteStatsSampler::default();
        let stats = sampler.complete_snapshot(
            "a",
            parse_stats_output("NETWORK\teth0\tup\t10\t20\n").unwrap(),
        );
        assert_eq!(stats.network_traffic().unwrap().rx_bytes_per_sec, 10.);
        let json = serde_json::to_value(&stats).unwrap();
        assert_eq!(
            json["networks"][0],
            serde_json::json!({"nic":"eth0", "state":"up", "rx_bytes_per_sec":10., "tx_bytes_per_sec":20.})
        );
        assert!(json.get("network_sample_time").is_none());
    }
}
