use crate::features::remote::job_state::{RemoteJobState, RemoteJobTicket};
use crate::features::runtime_jobs::StatsJobResult;
use futures::channel::mpsc::UnboundedReceiver;
use nyaterm_transport::{CpuUsageSource, NetworkSummaryInfo, RemoteStats, RemoteStatsSampler};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Instant;

pub(in crate::features) enum StatsApplyOutcome {
    Ignored,
    CompletedInactive,
    IconProbeCompleted {
        session_id: String,
        stats: Box<RemoteStats>,
    },
    Applied {
        session_id: String,
        stats: Box<RemoteStats>,
        status: String,
    },
    Failed {
        status: String,
    },
}

pub(super) struct StatsPaneState {
    sampler: Arc<RemoteStatsSampler>,
    job: RemoteJobState<StatsJobResult>,
    status: String,
    cpu_expanded: bool,
    manual_job_id: Option<u64>,
    warmup_retry_pending: bool,
    warmup_retry_attempted: bool,
    /// Bumped by every mutation that changes what `stats_presentation` returns.
    revision: u64,
    users_revision: u64,
    users_refresh_pending: bool,
    active_session_id: Option<String>,
    network_history: HashMap<String, VecDeque<NetworkHistorySample>>,
    selected_network_interfaces: HashMap<String, String>,
    snapshots: HashMap<String, RemoteStats>,
    icon_probes: HashMap<String, IconProbe>,
}

#[derive(Default)]
struct IconProbe {
    attempts: u8,
    pending_job: Option<u64>,
    retry_after: Option<Instant>,
    finished: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(in crate::features) struct NetworkHistorySample {
    pub sampled_at: Instant,
    pub rx_bytes_per_sec: f64,
    pub tx_bytes_per_sec: f64,
    pub interfaces: HashMap<String, (f64, f64)>,
}
#[derive(Clone)]
pub(in crate::features) struct StatsPresentationState {
    pub data: Option<RemoteStats>,
    pub network_history: Arc<[NetworkHistorySample]>,
    pub selected_network_interface: Option<String>,
    pub cpu_expanded: bool,
    pub pending: bool,
    pub error: bool,
    pub consecutive_refresh_failures: u8,
}

impl NetworkHistorySample {
    pub(in crate::features) fn traffic(
        &self,
        interface: Option<&str>,
    ) -> Option<NetworkSummaryInfo> {
        let (rx_bytes_per_sec, tx_bytes_per_sec) = match interface {
            Some(nic) => *self.interfaces.get(nic)?,
            None => (self.rx_bytes_per_sec, self.tx_bytes_per_sec),
        };
        Some(NetworkSummaryInfo {
            rx_bytes_per_sec,
            tx_bytes_per_sec,
        })
    }
}

impl StatsPresentationState {
    pub(in crate::features) fn network_traffic(&self) -> Option<NetworkSummaryInfo> {
        let stats = self.data.as_ref()?;
        match self.selected_network_interface.as_deref() {
            Some(nic) => stats
                .networks
                .iter()
                .find(|network| network.nic == nic)
                .map(|network| NetworkSummaryInfo {
                    rx_bytes_per_sec: network.rx_bytes_per_sec,
                    tx_bytes_per_sec: network.tx_bytes_per_sec,
                }),
            None => Some(stats.network_summary.clone()),
        }
    }
}

impl StatsPaneState {
    /// Record that the presentation changed.
    fn touch(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    pub(super) fn users_revision(&self) -> u64 {
        self.users_revision
    }

    pub(super) fn invalidate_users(&mut self, enabled: bool) {
        self.users_revision = self.users_revision.wrapping_add(1);
        self.users_refresh_pending = enabled;
        for stats in self.snapshots.values_mut() {
            stats.users.clear();
            stats.login_sessions.clear();
            stats.users_status = nyaterm_transport::LoggedInUsersStatus::NotCollected;
        }
        self.touch();
    }

    pub(super) fn users_refresh_pending(&self) -> bool {
        self.users_refresh_pending
    }

    pub(super) fn take_users_refresh_due(&mut self) -> bool {
        std::mem::take(&mut self.users_refresh_pending)
    }

    pub(super) fn revision(&self) -> u64 {
        self.revision
    }

    pub(super) fn apply_data(&mut self, session_id: &str, stats: RemoteStats) {
        if stats.cpu.usage_source == CpuUsageSource::WarmingUp {
            self.warmup_retry_pending = !self.warmup_retry_attempted;
        } else {
            self.warmup_retry_pending = false;
            self.warmup_retry_attempted = false;
        }
        self.activate_session(session_id);
        self.record_network_sample(session_id, &stats);
        self.snapshots.insert(session_id.to_string(), stats);
        self.touch();
    }

    fn record_network_sample(&mut self, session_id: &str, stats: &RemoteStats) {
        // A removed interface must not become selected again if it later reappears.
        if self
            .selected_network_interfaces
            .get(session_id)
            .is_some_and(|nic| !stats.networks.iter().any(|network| &network.nic == nic))
        {
            self.selected_network_interfaces.remove(session_id);
        }
        let sample = NetworkHistorySample {
            sampled_at: Instant::now(),
            rx_bytes_per_sec: stats.network_summary.rx_bytes_per_sec,
            tx_bytes_per_sec: stats.network_summary.tx_bytes_per_sec,
            interfaces: stats
                .networks
                .iter()
                .map(|network| {
                    (
                        network.nic.clone(),
                        (network.rx_bytes_per_sec, network.tx_bytes_per_sec),
                    )
                })
                .collect(),
        };
        let history = self
            .network_history
            .entry(session_id.to_string())
            .or_default();
        history.push_back(sample);
        while history.len() > 60 {
            history.pop_front();
        }
        self.touch();
    }

    pub(super) fn activate_session(&mut self, session_id: &str) {
        self.active_session_id = Some(session_id.to_string());
        self.touch();
    }

    fn active_network_history(&self) -> Arc<[NetworkHistorySample]> {
        self.active_session_id
            .as_deref()
            .and_then(|session_id| self.network_history.get(session_id))
            .map(|history| Arc::from(history.iter().cloned().collect::<Vec<_>>()))
            .unwrap_or_else(|| Arc::from([]))
    }

    pub(super) fn select_network_interface(
        &mut self,
        session_id: &str,
        interface: Option<&str>,
    ) -> bool {
        if self.active_session_id.as_deref() != Some(session_id) {
            return false;
        }
        if let Some(nic) = interface
            && !self
                .snapshots
                .get(session_id)
                .is_some_and(|stats| stats.networks.iter().any(|network| network.nic == nic))
        {
            return false;
        }
        if self
            .selected_network_interfaces
            .get(session_id)
            .map(String::as_str)
            == interface
        {
            return false;
        }
        match interface {
            Some(nic) => {
                self.selected_network_interfaces
                    .insert(session_id.to_string(), nic.to_string());
            }
            None => {
                self.selected_network_interfaces.remove(session_id);
            }
        }
        self.touch();
        true
    }

    fn clear_data(&mut self) {
        if let Some(session_id) = self.active_session_id.as_deref() {
            self.snapshots.remove(session_id);
        }
        self.touch();
    }

    pub(super) fn set_status(&mut self, status: impl Into<String>) {
        self.status = status.into();
        self.touch();
    }

    pub(super) fn is_pending(&self) -> bool {
        self.job.is_pending()
    }

    pub(super) fn last_refresh_at(&self) -> Option<Instant> {
        self.job.last_refresh_at()
    }

    pub(super) fn consecutive_refresh_failures(&self) -> u8 {
        self.job.consecutive_refresh_failures()
    }

    pub(super) fn is_pending_for(&self, session_id: &str) -> bool {
        self.job.is_pending_for(session_id)
            || self
                .icon_probes
                .get(session_id)
                .is_some_and(|probe| probe.pending_job.is_some())
    }

    pub(super) fn begin_job(
        &mut self,
        session_id: String,
        manual: bool,
    ) -> RemoteJobTicket<StatsJobResult> {
        self.touch();
        self.users_refresh_pending = false;
        let ticket = self.job.begin(session_id);
        if manual {
            self.manual_job_id = Some(ticket.job_id);
        }
        ticket
    }

    pub(super) fn mark_refresh_started(&mut self) {
        self.job.mark_refresh_started();
    }

    pub(super) fn take_event_receiver(&mut self) -> Option<UnboundedReceiver<StatsJobResult>> {
        self.job.take_event_receiver()
    }

    pub(super) fn complete_event(&mut self, job_id: u64, session_id: &str) -> bool {
        let matched = self.job.complete_if_matches(job_id, session_id);
        if matched {
            if self.manual_job_id == Some(job_id) {
                self.manual_job_id = None;
            }
            self.touch();
        }
        matched
    }

    pub(super) fn take_warmup_retry_due(&mut self) -> bool {
        if !self.warmup_retry_pending
            || self
                .last_refresh_at()
                .is_none_or(|started| started.elapsed() < std::time::Duration::from_secs(1))
        {
            return false;
        }
        self.warmup_retry_pending = false;
        self.warmup_retry_attempted = true;
        self.touch();
        true
    }

    pub(super) fn reset_refresh_failures(&mut self) {
        self.job.reset_refresh_failures();
        self.touch();
    }

    pub(super) fn record_refresh_failure(&mut self) -> u8 {
        self.touch();
        self.job.record_refresh_failure(false)
    }

    pub(super) fn toggle_cpu_expanded(&mut self) {
        self.cpu_expanded = !self.cpu_expanded;
        self.status = if self.cpu_expanded {
            "showing per-core CPU usage".to_string()
        } else {
            "collapsed per-core CPU usage".to_string()
        };
        self.touch();
    }

    pub(super) fn reset_for_session_switch(&mut self) {
        self.job.reset_for_session_switch();
        self.manual_job_id = None;
        self.warmup_retry_pending = false;
        self.warmup_retry_attempted = false;
        self.active_session_id = None;
        // Through the touching methods, not the fields: a session switch changes the
        // presentation, so it has to move the revision like any other mutation.
        self.clear_data();
        self.set_status("start an SSH session to inspect remote stats");
    }
}

impl StatsPaneState {
    pub(super) fn new() -> Self {
        Self {
            job: RemoteJobState::new(),
            status: "start an SSH session to inspect remote stats".to_string(),
            cpu_expanded: false,
            manual_job_id: None,
            warmup_retry_pending: false,
            warmup_retry_attempted: false,
            revision: 0,
            users_revision: 0,
            users_refresh_pending: false,
            active_session_id: None,
            network_history: HashMap::new(),
            selected_network_interfaces: HashMap::new(),
            snapshots: HashMap::new(),
            icon_probes: HashMap::new(),
            sampler: Arc::new(RemoteStatsSampler::default()),
        }
    }
}

impl StatsPaneState {
    pub(super) fn stats_sampler(&self) -> Arc<RemoteStatsSampler> {
        self.sampler.clone()
    }

    pub(super) fn clear_stats_sample(&mut self, session_id: &str) {
        self.icon_probes.remove(session_id);
        self.sampler.clear_session(session_id);
        self.network_history.remove(session_id);
        self.selected_network_interfaces.remove(session_id);
        self.snapshots.remove(session_id);
        if self.job.is_pending_for(session_id) {
            self.job.reset_for_session_switch();
        }
        if self.active_session_id.as_deref() == Some(session_id) {
            self.clear_data();
        }
    }

    pub(super) fn stats_presentation(&self) -> StatsPresentationState {
        StatsPresentationState {
            data: self
                .active_session_id
                .as_deref()
                .and_then(|id| self.snapshots.get(id))
                .cloned(),
            network_history: self.active_network_history(),
            selected_network_interface: self
                .active_session_id
                .as_ref()
                .and_then(|id| self.selected_network_interfaces.get(id))
                .cloned(),
            cpu_expanded: self.cpu_expanded,
            pending: self.is_pending(),
            error: self.consecutive_refresh_failures() > 0,
            consecutive_refresh_failures: self.consecutive_refresh_failures(),
        }
    }

    pub(super) fn stats_status(&self) -> &str {
        &self.status
    }

    pub(super) fn stats_manual_refreshing(&self) -> bool {
        self.manual_job_id.is_some()
    }

    #[cfg(test)]
    pub(super) fn record_stats_refresh_failure(&mut self) -> u8 {
        let failures = self.record_refresh_failure();
        if failures >= 3 {
            self.clear_data();
        }
        failures
    }

    pub(super) fn apply_stats_event(
        &mut self,
        mut event: StatsJobResult,
        active_session_id: Option<&str>,
    ) -> StatsApplyOutcome {
        if event.users_revision != self.users_revision
            && let Ok(stats) = &mut event.result
        {
            stats.users.clear();
            stats.login_sessions.clear();
            stats.users_status = nyaterm_transport::LoggedInUsersStatus::NotCollected;
        }
        // A tab switch invalidates the presentation job, but a bounded icon
        // probe still belongs to its original connection. Reconnect/close removes
        // its token, so a reply from an older connection cannot satisfy a new one.
        let probe_matches = self
            .icon_probes
            .get(&event.session_id)
            .is_some_and(|probe| probe.pending_job == Some(event.job_id));
        if probe_matches {
            let probe = self
                .icon_probes
                .get_mut(&event.session_id)
                .expect("matching probe");
            probe.pending_job = None;
            if event.result.is_ok() || probe.attempts >= 2 {
                probe.finished = true;
            } else {
                probe.retry_after = Some(Instant::now() + std::time::Duration::from_secs(1));
            }
        }
        if !self.complete_event(event.job_id, &event.session_id) && !probe_matches {
            return StatsApplyOutcome::Ignored;
        }
        if active_session_id != Some(event.session_id.as_str()) {
            if let Ok(stats) = &event.result {
                self.record_network_sample(&event.session_id, stats);
                self.snapshots
                    .insert(event.session_id.clone(), stats.clone());
            }
            if probe_matches && let Ok(stats) = event.result {
                return StatsApplyOutcome::IconProbeCompleted {
                    session_id: event.session_id,
                    stats: Box::new(stats),
                };
            }
            return StatsApplyOutcome::CompletedInactive;
        }
        match event.result {
            Ok(stats) => {
                self.reset_refresh_failures();
                let status = format!(
                    "loaded stats for {} 路 load {:.2}/{:.2}/{:.2}",
                    if stats.system.hostname.trim().is_empty() {
                        "remote host"
                    } else {
                        stats.system.hostname.as_str()
                    },
                    stats.load.load1,
                    stats.load.load5,
                    stats.load.load15
                );
                self.set_status(status.clone());
                self.apply_data(&event.session_id, stats.clone());
                StatsApplyOutcome::Applied {
                    session_id: event.session_id,
                    stats: Box::new(stats),
                    status,
                }
            }
            Err(error) => {
                let failures = self.record_refresh_failure();
                if failures >= 3 {
                    self.clear_data();
                }
                let status = format!("stats refresh failed: {error}");
                self.set_status(status.clone());
                StatsApplyOutcome::Failed { status }
            }
        }
    }
}

impl StatsPaneState {
    pub(super) fn icon_probe_needed(&self, session_id: &str) -> bool {
        self.icon_probes
            .get(session_id)
            .is_none_or(|probe| !probe.finished)
    }

    pub(super) fn icon_probe_due(&self, session_id: &str) -> bool {
        self.icon_probes.get(session_id).is_none_or(|probe| {
            !probe.finished
                && probe.pending_job.is_none()
                && probe
                    .retry_after
                    .is_none_or(|deadline| Instant::now() >= deadline)
        })
    }

    pub(super) fn begin_icon_probe(&mut self, session_id: &str, job_id: u64) {
        let probe = self.icon_probes.entry(session_id.into()).or_default();
        if !probe.finished && probe.pending_job.is_none() {
            probe.attempts += 1;
            probe.pending_job = Some(job_id);
            probe.retry_after = None;
        }
    }

    pub(super) fn take_icon_probe_snapshot(&mut self, session_id: &str) -> Option<RemoteStats> {
        if !self.icon_probe_needed(session_id) {
            return None;
        }
        let snapshot = self.snapshots.get(session_id)?.clone();
        self.icon_probes
            .entry(session_id.into())
            .or_default()
            .finished = true;
        Some(snapshot)
    }
}

#[cfg(test)]
mod tests {
    use super::{StatsApplyOutcome, StatsPaneState};
    use crate::features::runtime_jobs::StatsJobResult;
    use nyaterm_transport::{NetworkInfo, NetworkSummaryInfo, RemoteStats};
    use std::time::Instant;

    #[test]
    fn icon_probe_retries_once_after_delay_and_stays_finished_across_tab_switches() {
        let mut state = StatsPaneState::new();
        assert!(state.icon_probe_due("a"));
        for attempt in 0..2 {
            let ticket = state.begin_job("a".into(), false);
            state.begin_icon_probe("a", ticket.job_id);
            assert!(!state.icon_probe_due("a"));
            state.apply_stats_event(
                StatsJobResult {
                    users_revision: 0,
                    job_id: ticket.job_id,
                    session_id: "a".into(),
                    result: Err("probe failed".into()),
                },
                Some("a"),
            );
            if attempt == 0 {
                assert!(state.icon_probe_needed("a"));
                assert!(!state.icon_probe_due("a"));
                state.icon_probes.get_mut("a").unwrap().retry_after = Some(Instant::now());
                assert!(state.icon_probe_due("a"));
            }
        }
        assert!(!state.icon_probe_needed("a"));
        state.reset_for_session_switch();
        state.activate_session("a");
        assert!(!state.icon_probe_needed("a"));
        state.clear_stats_sample("a");
        assert!(state.icon_probe_needed("a"));
    }

    #[test]
    fn icon_probe_reuses_cached_system_snapshot_without_a_job() {
        let mut state = StatsPaneState::new();
        state.apply_data("a", stats(&[]));
        assert!(state.take_icon_probe_snapshot("a").is_some());
        assert!(!state.is_pending());
        assert!(!state.icon_probe_needed("a"));
        assert!(state.take_icon_probe_snapshot("a").is_none());
    }

    #[test]
    fn icon_probe_reply_after_tab_switch_does_not_complete_another_sessions_job() {
        let mut state = StatsPaneState::new();
        let old = state.begin_job("a".into(), false);
        state.begin_icon_probe("a", old.job_id);
        state.reset_for_session_switch();
        let current = state.begin_job("b".into(), false);
        let outcome = state.apply_stats_event(
            StatsJobResult {
                users_revision: 0,
                job_id: old.job_id,
                session_id: "a".into(),
                result: Ok(stats(&[])),
            },
            Some("b"),
        );
        assert!(
            matches!(outcome, StatsApplyOutcome::IconProbeCompleted { session_id, .. } if session_id == "a")
        );
        assert!(state.is_pending_for("b"));
        assert!(!state.icon_probe_needed("a"));
        assert!(state.complete_event(current.job_id, "b"));
    }

    #[test]
    fn a_closed_or_reconnected_session_rejects_old_icon_probe_results() {
        let mut state = StatsPaneState::new();
        let old = state.begin_job("a".into(), false);
        state.begin_icon_probe("a", old.job_id);
        state.clear_stats_sample("a");
        let current = state.begin_job("a".into(), false);
        state.begin_icon_probe("a", current.job_id);
        assert!(matches!(
            state.apply_stats_event(
                StatsJobResult {
                    users_revision: 0,
                    job_id: old.job_id,
                    session_id: "a".into(),
                    result: Ok(stats(&[]))
                },
                Some("a")
            ),
            StatsApplyOutcome::Ignored
        ));
        assert!(state.icon_probe_needed("a"));
        assert!(!state.icon_probe_due("a"));
        assert!(state.is_pending_for("a"));
        state.apply_stats_event(
            StatsJobResult {
                users_revision: 0,
                job_id: current.job_id,
                session_id: "a".into(),
                result: Ok(stats(&[])),
            },
            Some("a"),
        );
        assert!(!state.icon_probe_needed("a"));
    }

    #[test]
    fn toggling_users_clears_cache_and_discards_only_user_data_from_old_jobs() {
        use nyaterm_transport::{LoggedInSession, LoggedInUser, LoggedInUsersStatus};
        let mut state = StatsPaneState::new();
        let mut snapshot = stats(&[("eth0", 10., 20.)]);
        snapshot.users = vec![LoggedInUser {
            name: "alice".into(),
            sessions: 2,
        }];
        snapshot.users_status = LoggedInUsersStatus::Available;
        snapshot.login_sessions = vec![LoggedInSession {
            name: "alice".into(),
            terminal: "pts/0".into(),
            login_time: "2026-11-01 23:37".into(),
            source: Some("host.example".into()),
        }];
        state.apply_data("a", snapshot.clone());
        let ticket = state.begin_job("a".into(), false);
        state.invalidate_users(false);
        assert!(state.stats_presentation().data.unwrap().users.is_empty());
        assert!(
            state
                .stats_presentation()
                .data
                .unwrap()
                .login_sessions
                .is_empty()
        );
        assert!(!state.take_users_refresh_due());
        state.apply_stats_event(
            StatsJobResult {
                users_revision: 0,
                job_id: ticket.job_id,
                session_id: "a".into(),
                result: Ok(snapshot),
            },
            Some("a"),
        );
        let current = state.stats_presentation().data.unwrap();
        assert!(current.users.is_empty());
        assert!(current.login_sessions.is_empty());
        assert_eq!(current.users_status, LoggedInUsersStatus::NotCollected);
        assert_eq!(current.networks.len(), 1);
        state.invalidate_users(true);
        state.invalidate_users(true);
        assert!(state.take_users_refresh_due());
        assert!(!state.take_users_refresh_due());
    }

    fn stats(interfaces: &[(&str, f64, f64)]) -> RemoteStats {
        RemoteStats {
            networks: interfaces
                .iter()
                .map(|(nic, rx, tx)| NetworkInfo {
                    nic: nic.to_string(),
                    state: "up".to_string(),
                    rx_bytes_per_sec: *rx,
                    tx_bytes_per_sec: *tx,
                })
                .collect(),
            network_summary: NetworkSummaryInfo {
                rx_bytes_per_sec: interfaces.iter().map(|(_, rx, _)| rx).sum(),
                tx_bytes_per_sec: interfaces.iter().map(|(_, _, tx)| tx).sum(),
            },
            ..Default::default()
        }
    }

    #[test]
    fn selecting_an_interface_changes_current_and_historical_traffic_together() {
        let mut state = StatsPaneState::new();
        let started = Instant::now();
        state.apply_data("a", stats(&[("eth0", 10., 20.)]));
        state.apply_data("a", stats(&[("eth0", 30., 40.), ("eth1", 100., 200.)]));
        assert_eq!(
            state
                .stats_presentation()
                .network_traffic()
                .unwrap()
                .rx_bytes_per_sec,
            130.
        );
        let revision = state.revision();
        assert!(state.select_network_interface("a", Some("eth1")));
        assert_ne!(state.revision(), revision);
        let presentation = state.stats_presentation();
        let interface = presentation.selected_network_interface.as_deref();
        assert_eq!(interface, Some("eth1"));
        assert_eq!(
            presentation.network_traffic().unwrap(),
            NetworkSummaryInfo {
                rx_bytes_per_sec: 100.,
                tx_bytes_per_sec: 200.,
            }
        );
        assert_eq!(presentation.network_history[0].traffic(interface), None);
        assert_eq!(
            presentation.network_history[1].traffic(interface),
            presentation.network_traffic()
        );
        assert!(presentation.network_history[0].sampled_at >= started);
        assert!(
            presentation.network_history[1].sampled_at
                >= presentation.network_history[0].sampled_at
        );
        let revision = state.revision();
        assert!(!state.select_network_interface("a", Some("eth1")));
        assert!(!state.select_network_interface("a", Some("missing")));
        assert_eq!(state.revision(), revision);
        assert!(state.select_network_interface("a", None));
        assert_eq!(
            state
                .stats_presentation()
                .network_traffic()
                .unwrap()
                .tx_bytes_per_sec,
            240.
        );
    }

    #[test]
    fn network_selection_is_restored_per_session_and_cleared_on_close() {
        let mut state = StatsPaneState::new();
        state.apply_data("a", stats(&[("eth0", 10., 20.), ("eth1", 30., 40.)]));
        assert!(state.select_network_interface("a", Some("eth1")));
        state.reset_for_session_switch();
        state.apply_data("b", stats(&[("eth0", 100., 200.)]));
        assert!(
            state
                .stats_presentation()
                .selected_network_interface
                .is_none()
        );
        assert!(state.select_network_interface("b", Some("eth0")));
        assert!(
            !state.select_network_interface("a", None),
            "an inactive session cannot change selection"
        );
        state.reset_for_session_switch();
        state.activate_session("a");
        assert_eq!(
            state
                .stats_presentation()
                .selected_network_interface
                .as_deref(),
            Some("eth1")
        );
        assert_eq!(
            state
                .stats_presentation()
                .network_traffic()
                .unwrap()
                .rx_bytes_per_sec,
            30.
        );
        state.clear_stats_sample("a");
        let presentation = state.stats_presentation();
        assert!(presentation.selected_network_interface.is_none());
        assert!(presentation.network_history.is_empty());
        state.apply_data("a", stats(&[("eth1", 50., 60.)]));
        assert!(
            state
                .stats_presentation()
                .selected_network_interface
                .is_none()
        );
        state.activate_session("b");
        assert_eq!(
            state
                .stats_presentation()
                .selected_network_interface
                .as_deref(),
            Some("eth0")
        );
    }

    #[test]
    fn a_removed_interface_falls_back_to_summary_and_stays_unselected_when_it_returns() {
        let mut state = StatsPaneState::new();
        state.apply_data("a", stats(&[("eth0", 10., 20.), ("eth1", 30., 40.)]));
        state.select_network_interface("a", Some("eth1"));
        state.apply_data("a", stats(&[("eth0", 50., 60.)]));
        let presentation = state.stats_presentation();
        assert!(presentation.selected_network_interface.is_none());
        assert_eq!(
            presentation.network_traffic().unwrap().rx_bytes_per_sec,
            50.
        );
        assert!(
            presentation.network_history[1]
                .traffic(Some("eth1"))
                .is_none()
        );
        assert!(!state.select_network_interface("a", Some("eth1")));
        state.apply_data("a", stats(&[("eth0", 70., 80.), ("eth1", 90., 100.)]));
        assert!(
            state
                .stats_presentation()
                .selected_network_interface
                .is_none()
        );
        state.apply_data("a", stats(&[]));
        assert_eq!(
            state.stats_presentation().network_traffic(),
            Some(NetworkSummaryInfo::default())
        );
    }

    #[test]
    fn an_inactive_reply_reconciles_only_its_own_network_selection() {
        let mut state = StatsPaneState::new();
        state.apply_data("a", stats(&[("eth1", 10., 20.)]));
        state.select_network_interface("a", Some("eth1"));
        let ticket = state.begin_job("a".to_string(), false);
        state.activate_session("b");
        assert!(matches!(
            state.apply_stats_event(
                StatsJobResult {
                    users_revision: 0,
                    job_id: ticket.job_id,
                    session_id: "a".to_string(),
                    result: Ok(stats(&[("eth0", 30., 40.)])),
                },
                Some("b")
            ),
            StatsApplyOutcome::CompletedInactive
        ));
        assert!(state.stats_presentation().data.is_none());
        state.activate_session("a");
        assert!(
            state
                .stats_presentation()
                .selected_network_interface
                .is_none()
        );
        assert_eq!(
            state
                .stats_presentation()
                .network_traffic()
                .unwrap()
                .rx_bytes_per_sec,
            30.
        );
    }
}
