use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use nyaterm_remote_ops::stats::{NetworkSummaryInfo, RemoteStats, network_stale_after};
use nyaterm_ui::plot::NyaLinePoint;

pub(in crate::features) const NETWORK_WINDOW: Duration = Duration::from_secs(60);
const MAX_HISTORY_SAMPLES: usize = 256;

#[derive(Clone, Debug, PartialEq)]
pub(in crate::features) struct NetworkHistorySample {
    pub sampled_at: Instant,
    pub rx_bytes_per_sec: f64,
    pub tx_bytes_per_sec: f64,
    pub interfaces: HashMap<String, (f64, f64)>,
    valid_summary: bool,
    break_before: bool,
}

impl NetworkHistorySample {
    pub(in crate::features) fn traffic(
        &self,
        interface: Option<&str>,
    ) -> Option<NetworkSummaryInfo> {
        let (rx_bytes_per_sec, tx_bytes_per_sec) = match interface {
            Some(nic) => *self.interfaces.get(nic)?,
            None if self.valid_summary => (self.rx_bytes_per_sec, self.tx_bytes_per_sec),
            None => return None,
        };
        Some(NetworkSummaryInfo {
            rx_bytes_per_sec,
            tx_bytes_per_sec,
        })
    }
}

/// Immutable projected series, rebuilt only on sampling or selection changes.
/// Advancing the viewport changes its domain, not these point allocations.
#[derive(Default)]
pub(in crate::features) struct NetworkChart {
    origin: Option<Instant>,
    pub rx: Arc<[NyaLinePoint]>,
    pub tx: Arc<[NyaLinePoint]>,
}

pub(in crate::features) struct NetworkChartScale {
    pub x_range: [f64; 2],
    pub peak: Option<f64>,
    pub maximum: f64,
    pub unit: &'static str,
    pub divisor: f64,
    pub available_points: usize,
}

impl NetworkChart {
    fn new(history: &[NetworkHistorySample], interface: Option<&str>) -> Self {
        let origin = history.last().map(|sample| sample.sampled_at);
        let mut rx = Vec::new();
        let mut tx = Vec::new();
        if let Some(origin) = origin {
            for sample in history {
                let x = -origin
                    .saturating_duration_since(sample.sampled_at)
                    .as_secs_f64();
                if sample.break_before {
                    rx.push(NyaLinePoint { x, value: None });
                    tx.push(NyaLinePoint { x, value: None });
                }
                let traffic = sample.traffic(interface);
                rx.push(NyaLinePoint {
                    x,
                    value: traffic.as_ref().map(|rate| rate.rx_bytes_per_sec),
                });
                tx.push(NyaLinePoint {
                    x,
                    value: traffic.as_ref().map(|rate| rate.tx_bytes_per_sec),
                });
            }
        }
        Self {
            origin,
            rx: rx.into(),
            tx: tx.into(),
        }
    }

    pub(in crate::features) fn scale(&self, now: Instant) -> NetworkChartScale {
        let end = self.origin.map_or(0., |origin| {
            now.saturating_duration_since(origin).as_secs_f64()
        });
        let x_range = [end - NETWORK_WINDOW.as_secs_f64(), end];
        let visible = |point: &&NyaLinePoint| point.x >= x_range[0] && point.x <= x_range[1];
        let peak = self
            .rx
            .iter()
            .chain(self.tx.iter())
            .filter(visible)
            .filter_map(|point| point.value)
            .filter(|value| value.is_finite() && *value >= 0.)
            .reduce(f64::max);
        let available_points = self
            .rx
            .iter()
            .filter(visible)
            .filter(|point| point.value.is_some())
            .count();
        let units = ["B/s", "KB/s", "MB/s", "GB/s", "TB/s"];
        let mut unit = 0;
        let mut divisor = 1.;
        while peak.unwrap_or(0.) / divisor >= 1024. && unit + 1 < units.len() {
            divisor *= 1024.;
            unit += 1;
        }
        let maximum = if peak.unwrap_or(0.) == 0. {
            100.
        } else {
            let step = (peak.unwrap() / divisor * 1.15 / 2.).max(0.01);
            let power = 10_f64.powf(step.log10().floor());
            let nice = [1., 2., 2.5, 5., 10.]
                .into_iter()
                .find(|nice| *nice * power >= step)
                .unwrap_or(10.);
            (2. * nice * power * divisor).min(f64::MAX)
        };
        NetworkChartScale {
            x_range,
            peak,
            maximum,
            unit: units[unit],
            divisor,
            available_points,
        }
    }
}

pub(super) struct NetworkSessionState {
    pub history: Arc<[NetworkHistorySample]>,
    pub chart: Arc<NetworkChart>,
    pub expanded: bool,
    pub last_success_at: Option<Instant>,
    pub failures: u8,
    interval: Duration,
}

impl Default for NetworkSessionState {
    fn default() -> Self {
        Self {
            history: Arc::from([]),
            chart: Arc::default(),
            expanded: false,
            last_success_at: None,
            failures: 0,
            interval: Duration::from_secs(3),
        }
    }
}

impl NetworkSessionState {
    pub fn record(&mut self, stats: &RemoteStats, interface: Option<&str>) {
        let sampled_at = stats
            .network_sample_time
            .sampled_at
            .unwrap_or_else(Instant::now);
        let interval = stats.network_sample_time.interval;
        let sample = NetworkHistorySample {
            sampled_at,
            rx_bytes_per_sec: stats.network_summary.rx_bytes_per_sec,
            tx_bytes_per_sec: stats.network_summary.tx_bytes_per_sec,
            valid_summary: stats.network_traffic().is_some(),
            interfaces: stats
                .networks
                .iter()
                .filter_map(|network| {
                    let traffic = network.traffic()?;
                    Some((
                        network.nic.clone(),
                        (traffic.rx_bytes_per_sec, traffic.tx_bytes_per_sec),
                    ))
                })
                .collect(),
            break_before: self.last_success_at.is_some_and(|previous| {
                sampled_at.saturating_duration_since(previous)
                    > network_stale_after(interval.max(self.interval))
            }),
        };
        self.last_success_at = Some(sampled_at);
        self.interval = interval;
        self.failures = 0;
        self.push(sample, interface);
    }

    pub fn fail(&mut self, now: Instant, interface: Option<&str>) {
        let already_failed = self.failures > 0;
        self.failures = self.failures.saturating_add(1);
        if !already_failed {
            self.push(
                NetworkHistorySample {
                    sampled_at: now,
                    rx_bytes_per_sec: 0.,
                    tx_bytes_per_sec: 0.,
                    interfaces: HashMap::new(),
                    valid_summary: false,
                    break_before: true,
                },
                interface,
            );
        }
    }

    fn push(&mut self, sample: NetworkHistorySample, interface: Option<&str>) {
        let now = sample.sampled_at;
        let mut history: Vec<_> = self
            .history
            .iter()
            .filter(|old| now.saturating_duration_since(old.sampled_at) <= NETWORK_WINDOW)
            .cloned()
            .collect();
        history.push(sample);
        if history.len() > MAX_HISTORY_SAMPLES {
            history.drain(..history.len() - MAX_HISTORY_SAMPLES);
        }
        self.history = history.into();
        self.select(interface);
    }

    pub fn select(&mut self, interface: Option<&str>) {
        self.chart = Arc::new(NetworkChart::new(&self.history, interface));
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_HISTORY_SAMPLES, NetworkSessionState};
    use nyaterm_remote_ops::stats::{
        NetworkInfo, NetworkRateStatus, NetworkSampleTime, NetworkSummaryInfo, RemoteStats,
    };
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    fn stats(at: Instant, status: NetworkRateStatus, rx: f64, tx: f64) -> RemoteStats {
        RemoteStats {
            network_sample_time: NetworkSampleTime {
                sampled_at: Some(at),
                interval: Duration::from_secs(3),
            },
            networks: vec![NetworkInfo {
                nic: "eth0".into(),
                state: "up".into(),
                rate_status: status,
                rx_bytes_per_sec: rx,
                tx_bytes_per_sec: tx,
            }],
            network_summary: NetworkSummaryInfo {
                rx_bytes_per_sec: rx,
                tx_bytes_per_sec: tx,
            },
            ..Default::default()
        }
    }

    #[test]
    fn warmup_is_missing_and_short_irregular_history_keeps_a_sixty_second_domain() {
        let mut state = NetworkSessionState::default();
        let now = Instant::now();
        state.record(&stats(now, NetworkRateStatus::WarmingUp, 0., 0.), None);
        assert!(state.chart.rx[0].value.is_none());
        assert_eq!(state.chart.scale(now).peak, None);
        state.record(
            &stats(
                now + Duration::from_secs(3),
                NetworkRateStatus::Available,
                100.,
                200.,
            ),
            None,
        );
        state.record(
            &stats(
                now + Duration::from_millis(7500),
                NetworkRateStatus::Available,
                50.,
                75.,
            ),
            None,
        );
        let scale = state.chart.scale(now + Duration::from_millis(7500));
        assert_eq!(scale.x_range, [-60., 0.]);
        assert_eq!(
            state
                .chart
                .rx
                .iter()
                .map(|point| point.x)
                .collect::<Vec<_>>(),
            vec![-7.5, -4.5, 0.]
        );
        assert_eq!(
            (state.chart.rx[2].x - state.chart.rx[1].x) / (scale.x_range[1] - scale.x_range[0]),
            0.075
        );
        assert_eq!(scale.available_points, 2);
        assert_eq!(
            state
                .chart
                .rx
                .iter()
                .map(|point| point.x)
                .collect::<Vec<_>>(),
            state
                .chart
                .tx
                .iter()
                .map(|point| point.x)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn visible_peak_excludes_expired_points_and_has_a_shared_nice_y_axis() {
        let mut state = NetworkSessionState::default();
        let now = Instant::now();
        state.record(
            &stats(now, NetworkRateStatus::Available, 90. * 1024., 120. * 1024.),
            None,
        );
        let scale = state.chart.scale(now);
        assert_eq!(scale.peak, Some(120. * 1024.));
        assert_eq!(scale.maximum, 200. * 1024.);
        assert_eq!(scale.unit, "KB/s");
        assert!(scale.maximum >= scale.peak.unwrap() * 1.15);
        let empty = state.chart.scale(now + Duration::from_secs(61));
        assert_eq!(empty.peak, None);
        assert_eq!(empty.available_points, 0);
        assert_eq!(empty.x_range, [1., 61.]);
        assert_eq!(empty.maximum, 100.);
        state.record(
            &stats(
                now + Duration::from_secs(61),
                NetworkRateStatus::Available,
                0.,
                0.,
            ),
            None,
        );
        assert_eq!(state.history.len(), 1);
        assert_eq!(
            state.chart.scale(now + Duration::from_secs(61)).peak,
            Some(0.)
        );
    }

    #[test]
    fn failure_and_long_gaps_break_lines_without_erasing_success_time() {
        let mut state = NetworkSessionState::default();
        let now = Instant::now();
        state.record(&stats(now, NetworkRateStatus::Available, 10., 20.), None);
        state.fail(now + Duration::from_secs(3), None);
        let history = state.history.clone();
        state.fail(now + Duration::from_secs(6), None);
        assert!(
            Arc::ptr_eq(&history, &state.history),
            "consecutive failures must not grow history"
        );
        assert_eq!(state.last_success_at, Some(now));
        state.record(
            &stats(
                now + Duration::from_secs(7),
                NetworkRateStatus::Available,
                30.,
                40.,
            ),
            None,
        );
        assert_eq!(state.failures, 0);
        assert!(
            state.chart.rx[1..state.chart.rx.len() - 1]
                .iter()
                .any(|point| point.value.is_none())
        );
        state.record(
            &stats(
                now + Duration::from_secs(30),
                NetworkRateStatus::Available,
                30.,
                40.,
            ),
            None,
        );
        assert_eq!(state.chart.rx[state.chart.rx.len() - 2].value, None);
    }

    #[test]
    fn selection_missing_interfaces_and_bounded_history_preserve_real_zero() {
        let mut state = NetworkSessionState::default();
        let now = Instant::now();
        for _ in 0..MAX_HISTORY_SAMPLES + 10 {
            state.record(
                &stats(now, NetworkRateStatus::Available, 0., 0.),
                Some("eth0"),
            );
        }
        assert_eq!(state.history.len(), MAX_HISTORY_SAMPLES);
        assert!(state.chart.rx.iter().all(|point| point.value == Some(0.)));
        state.select(Some("missing"));
        assert!(state.chart.rx.iter().all(|point| point.value.is_none()));
        state.select(None);
        assert_eq!(state.chart.scale(now).peak, Some(0.));
    }
}
