use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use nyaterm_remote_ops::stats::NetworkSummaryInfo;
use nyaterm_ui::plot::NyaLinePoint;

pub const NETWORK_WINDOW: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, PartialEq)]
pub struct NetworkHistorySample {
    pub sampled_at: Instant,
    pub rx_bytes_per_sec: f64,
    pub tx_bytes_per_sec: f64,
    pub interfaces: HashMap<String, (f64, f64)>,
    pub valid_summary: bool,
    pub break_before: bool,
}

impl NetworkHistorySample {
    pub fn traffic(&self, interface: Option<&str>) -> Option<NetworkSummaryInfo> {
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
pub struct NetworkChart {
    origin: Option<Instant>,
    pub rx: Arc<[NyaLinePoint]>,
    pub tx: Arc<[NyaLinePoint]>,
}

pub struct NetworkChartScale {
    pub x_range: [f64; 2],
    pub peak: Option<f64>,
    pub maximum: f64,
    pub unit: &'static str,
    pub divisor: f64,
    pub available_points: usize,
}

impl NetworkChart {
    pub fn new(history: &[NetworkHistorySample], interface: Option<&str>) -> Self {
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

    pub fn scale(&self, now: Instant) -> NetworkChartScale {
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
        let units = ["B/s", "KiB/s", "MiB/s", "GiB/s", "TiB/s"];
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
