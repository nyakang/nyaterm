use crate::features::remote::job_state::{RemoteJobState, RemoteJobTicket};
use crate::features::runtime_jobs::{ProcessJobOutput, ProcessJobResult};
use crate::models::{RemoteProcessSortDirection, RemoteProcessSortKey};
use futures::channel::mpsc::UnboundedReceiver;
use nyaterm_remote_ops::remote_process::RemoteProcess;
use std::sync::Arc;
use std::time::Instant;

pub(in crate::features) enum ProcessApplyOutcome {
    Ignored,
    CompletedInactive,
    Applied { status: String },
}

pub(super) struct ProcessPaneState {
    job: RemoteJobState<ProcessJobResult>,
    items: Arc<[RemoteProcess]>,
    /// Lowercased text each process is searched by, parallel to `items`. Built on the
    /// first non-empty query after a new list arrives and dropped with the list, so a
    /// panel nobody searches never pays for it.
    search_keys: Option<Arc<[String]>>,
    /// Bumped by every mutation that changes what `process_presentation` returns.
    revision: u64,
    data_generation: u64,
    derived: Option<ProcessDerivedCache>,
    snapshot_loaded: bool,
    status: String,
    search_draft: String,
    sort_key: RemoteProcessSortKey,
    sort_direction: RemoteProcessSortDirection,
    selected_pid: Option<u32>,
    menu_pid: Option<u32>,
    nice_draft: String,
}

#[derive(Clone, PartialEq, Eq)]
struct ProcessDerivedKey {
    data_generation: u64,
    normalized_query: String,
    sort_key: RemoteProcessSortKey,
    sort_direction: RemoteProcessSortDirection,
}

struct ProcessDerivedCache {
    key: ProcessDerivedKey,
    items: Arc<[RemoteProcess]>,
}

#[derive(Clone)]
pub(in crate::features) struct ProcessPresentationState {
    pub items: Arc<[RemoteProcess]>,
    pub snapshot_loaded: bool,
    pub status: String,
    pub search_draft: String,
    pub sort_key: RemoteProcessSortKey,
    pub sort_direction: RemoteProcessSortDirection,
    pub selected_pid: Option<u32>,
    pub menu_pid: Option<u32>,
    pub nice_draft: String,
    pub pending: bool,
}

impl ProcessPaneState {
    pub(super) fn is_pending(&self) -> bool {
        self.job.is_pending()
    }

    pub(super) fn last_refresh_at(&self) -> Option<Instant> {
        self.job.last_refresh_at()
    }

    pub(super) fn is_pending_for(&self, session_id: &str) -> bool {
        self.job.is_pending_for(session_id)
    }

    pub(super) fn begin_job(&mut self, session_id: String) -> RemoteJobTicket<ProcessJobResult> {
        // `pending` is part of the presentation, so starting and finishing a job moves the
        // revision. A status change happens to accompany both today, which masked it.
        self.touch();
        self.job.begin(session_id)
    }

    pub(super) fn mark_refresh_started(&mut self) {
        self.job.mark_refresh_started();
    }

    pub(super) fn take_event_receiver(&mut self) -> Option<UnboundedReceiver<ProcessJobResult>> {
        self.job.take_event_receiver()
    }

    pub(super) fn complete_event(&mut self, job_id: u64, session_id: &str) -> bool {
        self.touch();
        self.job.complete_if_matches(job_id, session_id)
    }

    pub(super) fn reset_refresh_failures(&mut self) {
        self.job.reset_refresh_failures();
    }

    pub(super) fn record_refresh_failure(&mut self, terminal: bool) -> u8 {
        self.job.record_refresh_failure(terminal)
    }

    pub(super) fn apply_search(&mut self, text: String) {
        self.search_draft = text;
        self.selected_pid = None;
        self.menu_pid = None;
        self.nice_draft = "0".to_string();
        self.reconcile();
    }

    pub(super) fn toggle_sort(&mut self, key: RemoteProcessSortKey) {
        if self.sort_key == key {
            self.sort_direction = self.sort_direction.reversed();
        } else {
            self.sort_key = key;
            self.sort_direction = match key {
                RemoteProcessSortKey::Cpu | RemoteProcessSortKey::Memory => {
                    RemoteProcessSortDirection::Descending
                }
                RemoteProcessSortKey::Pid
                | RemoteProcessSortKey::User
                | RemoteProcessSortKey::Command => RemoteProcessSortDirection::Ascending,
            };
        }
        self.reconcile();
        self.status = format!(
            "sorted processes by {} {}",
            self.sort_key.label(),
            self.sort_direction.marker()
        );
    }

    pub(super) fn toggle_selection(&mut self, pid: u32) {
        self.touch();
        self.menu_pid = None;
        self.selected_pid = (self.selected_pid != Some(pid)).then_some(pid);
        self.nice_draft = "0".to_string();
    }

    pub(super) fn apply_nice_input(&mut self, text: String) {
        let negative = text.starts_with('-');
        let digits: String = text.chars().filter(char::is_ascii_digit).take(3).collect();
        self.nice_draft = if negative {
            format!("-{digits}")
        } else {
            digits
        };
        self.touch();
    }

    pub(super) fn validated_nice_draft(&mut self) -> Option<(u32, i32)> {
        let Some(pid) = self.selected_pid else {
            self.set_status("select a process before applying nice");
            return None;
        };
        let Ok(nice) = self.nice_draft.trim().parse::<i32>() else {
            self.set_status("nice must be an integer from -20 to 19");
            return None;
        };
        if !(-20..=19).contains(&nice) {
            self.set_status("nice must be between -20 and 19");
            return None;
        }
        Some((pid, nice))
    }

    pub(super) fn apply_processes(&mut self, processes: Vec<RemoteProcess>) {
        let contains_pid = |pid| processes.iter().any(|process| process.pid == pid);
        if self.selected_pid.is_some_and(|pid| !contains_pid(pid)) {
            self.selected_pid = None;
            self.nice_draft = "0".to_string();
        }
        if self.menu_pid.is_some_and(|pid| !contains_pid(pid)) {
            self.menu_pid = None;
        }
        self.items = processes.into();
        self.search_keys = None;
        self.data_generation = self.data_generation.wrapping_add(1);
        self.derived = None;
        self.snapshot_loaded = true;
        self.reconcile();
    }

    fn clear_data(&mut self) {
        self.items = Arc::from([]);
        self.search_keys = None;
        self.data_generation = self.data_generation.wrapping_add(1);
        self.derived = None;
        self.snapshot_loaded = false;
        self.selected_pid = None;
        self.menu_pid = None;
        self.reconcile();
    }

    /// Record that the presentation changed.
    fn touch(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    pub(super) fn revision(&self) -> u64 {
        self.revision
    }

    pub(super) fn set_status(&mut self, status: impl Into<String>) {
        self.status = status.into();
        self.touch();
    }

    pub(super) fn toggle_menu(&mut self, pid: u32) {
        self.menu_pid = (self.menu_pid != Some(pid)).then_some(pid);
        self.touch();
    }

    pub(super) fn close_menu(&mut self) {
        self.menu_pid = None;
        self.touch();
    }

    /// Bring the derived list and sort key back in step.
    ///
    /// Called by every mutator that changes one of their inputs, so a reader never has
    /// to trigger the recompute -- which is what the render pass used to do, by calling
    /// `derived_items` and `clamp_*` through `&mut self` while building elements.
    ///
    /// Cheap to call redundantly: the recompute is keyed, so an unchanged key returns
    /// the cached list.
    fn reconcile(&mut self) {
        self.derived_items();
        self.touch();
    }

    /// The filtered, sorted list, without recomputing.
    ///
    /// `reconcile` guarantees the cache is populated and current, so this is a read.
    pub(super) fn derived(&self) -> Arc<[RemoteProcess]> {
        self.derived
            .as_ref()
            .map(|cache| cache.items.clone())
            .unwrap_or_else(|| Arc::from([]))
    }

    fn derived_items(&mut self) -> Arc<[RemoteProcess]> {
        let key = ProcessDerivedKey {
            data_generation: self.data_generation,
            normalized_query: self.search_draft.trim().to_ascii_lowercase(),
            sort_key: self.sort_key,
            sort_direction: self.sort_direction,
        };
        if let Some(cache) = self.derived.as_ref()
            && cache.key == key
        {
            return cache.items.clone();
        }
        let mut items = if key.normalized_query.is_empty() {
            self.items.to_vec()
        } else {
            let items = &self.items;
            let search_keys = self
                .search_keys
                .get_or_insert_with(|| items.iter().map(process_search_key).collect());
            debug_assert_eq!(items.len(), search_keys.len());
            items
                .iter()
                .zip(search_keys.iter())
                .filter(|(_, search_key)| search_key.contains(&key.normalized_query))
                .map(|(process, _)| process.clone())
                .collect::<Vec<_>>()
        };
        sort_processes(&mut items, key.sort_key, key.sort_direction);
        let items: Arc<[RemoteProcess]> = items.into();
        self.derived = Some(ProcessDerivedCache {
            key,
            items: items.clone(),
        });
        items
    }

    pub(super) fn reset_for_session_switch(&mut self) {
        self.job.reset_for_session_switch();
        self.clear_data();
        self.status = "ready".to_string();
    }
}

fn process_search_key(process: &RemoteProcess) -> String {
    format!(
        "{} {} {} {} {} {}",
        process.pid,
        process.ppid,
        process.user,
        process.state,
        process.command,
        process.command_line
    )
    .to_ascii_lowercase()
}

fn sort_processes(
    processes: &mut [RemoteProcess],
    key: RemoteProcessSortKey,
    direction: RemoteProcessSortDirection,
) {
    processes.sort_by(|left, right| compare_processes(left, right, key, direction));
}

fn compare_processes(
    left: &RemoteProcess,
    right: &RemoteProcess,
    key: RemoteProcessSortKey,
    direction: RemoteProcessSortDirection,
) -> std::cmp::Ordering {
    // Remote metrics may contain NaN. Treating unordered values as equal makes
    // the secondary keys create cycles with otherwise ordered numeric values.
    let ordering = match key {
        RemoteProcessSortKey::Command => left
            .command
            .cmp(&right.command)
            .then_with(|| left.pid.cmp(&right.pid)),
        RemoteProcessSortKey::Memory => left
            .memory_percent
            .total_cmp(&right.memory_percent)
            .then_with(|| left.rss_kb.cmp(&right.rss_kb))
            .then_with(|| left.pid.cmp(&right.pid)),
        RemoteProcessSortKey::Pid => left.pid.cmp(&right.pid),
        RemoteProcessSortKey::User => left
            .user
            .cmp(&right.user)
            .then_with(|| left.pid.cmp(&right.pid)),
        RemoteProcessSortKey::Cpu => left
            .cpu_percent
            .total_cmp(&right.cpu_percent)
            .then_with(|| left.memory_percent.total_cmp(&right.memory_percent))
            .then_with(|| left.pid.cmp(&right.pid)),
    };

    match direction {
        RemoteProcessSortDirection::Ascending => ordering,
        RemoteProcessSortDirection::Descending => ordering.reverse(),
    }
}

impl ProcessPaneState {
    pub(super) fn new() -> Self {
        Self {
            job: RemoteJobState::new(),
            items: Arc::from([]),
            search_keys: None,
            revision: 0,
            data_generation: 0,
            derived: None,
            snapshot_loaded: false,
            status: "ready".to_string(),
            search_draft: String::new(),
            sort_key: RemoteProcessSortKey::Cpu,
            sort_direction: RemoteProcessSortDirection::Descending,
            selected_pid: None,
            menu_pid: None,
            nice_draft: "0".to_string(),
        }
    }
}

impl ProcessPaneState {
    pub(super) fn process_presentation(&self) -> ProcessPresentationState {
        ProcessPresentationState {
            items: self.items.clone(),
            snapshot_loaded: self.snapshot_loaded,
            status: self.status.clone(),
            search_draft: self.search_draft.clone(),
            sort_key: self.sort_key,
            sort_direction: self.sort_direction,
            selected_pid: self.selected_pid,
            menu_pid: self.menu_pid,
            nice_draft: self.nice_draft.clone(),
            pending: self.is_pending(),
        }
    }

    pub(super) fn process_status(&self) -> &str {
        &self.status
    }

    pub(super) fn loaded_process_count(&self) -> Option<usize> {
        (self.snapshot_loaded && !self.items.is_empty()).then_some(self.items.len())
    }

    pub(super) fn apply_process_event(
        &mut self,
        event: ProcessJobResult,
        active_session_id: Option<&str>,
    ) -> ProcessApplyOutcome {
        if !self.complete_event(event.job_id, &event.session_id) {
            return ProcessApplyOutcome::Ignored;
        }
        if active_session_id != Some(event.session_id.as_str()) {
            return ProcessApplyOutcome::CompletedInactive;
        }
        let was_list_refresh = self.status == "listing remote processes";
        let status = match event.result {
            Ok(ProcessJobOutput::Listed(processes)) => {
                self.reset_refresh_failures();
                let status = format!("loaded {} remote process(es)", processes.len());
                self.apply_processes(processes);
                status
            }
            Ok(ProcessJobOutput::Signalled {
                pid,
                signal,
                processes,
            }) => {
                self.apply_processes(processes);
                format!("sent {signal} to pid {pid}")
            }
            Ok(ProcessJobOutput::Reniced {
                pid,
                nice,
                processes,
            }) => {
                self.apply_processes(processes);
                format!("reniced pid {pid} to {nice}")
            }
            Err(error) => {
                if was_list_refresh {
                    let terminal = error.contains(
                        nyaterm_remote_ops::remote_process::PROCESS_LIST_UNSUPPORTED_ERROR,
                    );
                    if self.record_refresh_failure(terminal) >= 3 {
                        self.clear_data();
                    }
                }
                format!("process operation failed: {error}")
            }
        };
        self.set_status(status.clone());
        ProcessApplyOutcome::Applied { status }
    }
}

#[cfg(test)]
mod search_key_tests {
    use nyaterm_remote_ops::remote_process::RemoteProcess;

    use super::ProcessPaneState;

    fn process(pid: u32, user: &str, command: &str, command_line: &str) -> RemoteProcess {
        RemoteProcess {
            pid,
            ppid: 1,
            user: user.to_string(),
            state: "S".to_string(),
            cpu_percent: 0.0,
            memory_percent: 0.0,
            rss_kb: 0,
            vsz_kb: 0,
            elapsed: String::new(),
            command: command.to_string(),
            command_line: command_line.to_string(),
        }
    }

    fn sample() -> Vec<RemoteProcess> {
        vec![
            process(100, "root", "sshd", "/usr/sbin/sshd -D"),
            process(2001, "www-data", "nginx", "nginx: worker process"),
            process(31337, "Dev", "Python3", "python3 -m http.server 8000"),
            process(4, "root", "kworker", "[kworker/0:1-events]"),
        ]
    }

    /// The matching rule from before the keys were cached.
    fn reference_matches(process: &RemoteProcess, normalized_query: &str) -> bool {
        if normalized_query.is_empty() {
            return true;
        }
        format!(
            "{} {} {} {} {} {}",
            process.pid,
            process.ppid,
            process.user,
            process.state,
            process.command,
            process.command_line
        )
        .to_ascii_lowercase()
        .contains(normalized_query)
    }

    fn pids(items: &[RemoteProcess]) -> Vec<u32> {
        let mut pids: Vec<u32> = items.iter().map(|item| item.pid).collect();
        pids.sort_unstable();
        pids
    }

    #[test]
    fn search_keys_are_built_only_for_a_non_empty_query_and_dropped_with_the_list() {
        let mut state = ProcessPaneState::new();
        state.apply_processes(sample());
        assert!(
            state.search_keys.is_none(),
            "an unused panel must not pay for the keys"
        );
        state.derived_items();
        assert!(state.search_keys.is_none(), "an empty query needs no keys");

        state.apply_search("ssh".to_string());
        assert_eq!(state.search_keys.as_ref().map(|keys| keys.len()), Some(4));

        // While a query is active a fresh list is searched at once, so its keys are
        // rebuilt immediately and must describe the new list, never the old one.
        state.apply_processes(vec![process(7, "alice", "sshd", "sshd: alice")]);
        assert_eq!(state.search_keys.as_ref().map(|keys| keys.len()), Some(1));

        // With the query cleared, a fresh list drops the keys again.
        state.apply_search(String::new());
        state.apply_processes(sample());
        assert!(
            state.search_keys.is_none(),
            "a fresh list with no query must not build keys"
        );
        state.apply_search("ssh".to_string());
        assert!(state.search_keys.is_some());
        // Clearing the data leaves no stale keys behind: with the query still active
        // the empty list is searched at once, so at most an empty key set remains.
        state.clear_data();
        assert!(
            state
                .search_keys
                .as_ref()
                .is_none_or(|keys| keys.is_empty())
        );
    }

    #[test]
    fn filtering_matches_the_original_rule_for_every_query() {
        let processes = sample();
        for query in [
            "",
            "ssh",
            "SSH",
            "root",
            "www",
            "dev",
            "python3",
            "8000",
            "1",
            "31337",
            "worker",
            "[kworker",
            "-d",
            "no such thing",
            " ",
            "nginx: w",
        ] {
            let mut state = ProcessPaneState::new();
            state.apply_processes(processes.clone());
            state.apply_search(query.to_string());
            let normalized = query.trim().to_ascii_lowercase();
            let expected: Vec<u32> = {
                let matching: Vec<RemoteProcess> = processes
                    .iter()
                    .filter(|process| reference_matches(process, &normalized))
                    .cloned()
                    .collect();
                pids(&matching)
            };
            assert_eq!(pids(&state.derived_items()), expected, "query {query:?}");
        }
    }

    #[test]
    fn a_second_list_is_searched_with_its_own_keys() {
        let mut state = ProcessPaneState::new();
        state.apply_processes(sample());
        state.apply_search("sshd".to_string());
        assert_eq!(pids(&state.derived_items()), [100]);

        state.apply_processes(vec![process(7, "alice", "bash", "-bash")]);
        assert!(pids(&state.derived_items()).is_empty());
        state.apply_search("bash".to_string());
        assert_eq!(pids(&state.derived_items()), [7]);
    }
}

#[cfg(test)]
mod sort_tests {
    use super::{compare_processes, sort_processes};
    use crate::models::{RemoteProcessSortDirection, RemoteProcessSortKey};
    use nyaterm_remote_ops::remote_process::RemoteProcess;
    use std::cmp::Ordering;

    fn process(pid: u32, cpu: f64, memory: f64, rss: u64) -> RemoteProcess {
        RemoteProcess {
            pid,
            ppid: 1,
            user: format!("user{}", pid % 2),
            state: "S".to_string(),
            cpu_percent: cpu,
            memory_percent: memory,
            rss_kb: rss,
            vsz_kb: 0,
            elapsed: String::new(),
            command: format!("command{}", pid % 3),
            command_line: String::new(),
        }
    }

    #[test]
    fn nan_metrics_do_not_form_cycles_with_pid_ties() {
        for key in [RemoteProcessSortKey::Cpu, RemoteProcessSortKey::Memory] {
            let mut items = vec![
                process(2, f64::NAN, f64::NAN, 0),
                process(1, 1.0, 1.0, 0),
                process(3, 0.0, 0.0, 0),
            ];
            sort_processes(&mut items, key, RemoteProcessSortDirection::Ascending);
            assert_eq!(
                items.iter().map(|item| item.pid).collect::<Vec<_>>(),
                [3, 1, 2]
            );
            sort_processes(&mut items, key, RemoteProcessSortDirection::Descending);
            assert_eq!(
                items.iter().map(|item| item.pid).collect::<Vec<_>>(),
                [2, 1, 3]
            );
        }
    }

    #[test]
    fn process_sort_modes_obey_ordering_laws_with_non_finite_metrics() {
        let values = [
            -f64::NAN,
            f64::NEG_INFINITY,
            -1.0,
            -0.0,
            0.0,
            1.0,
            f64::INFINITY,
            f64::NAN,
        ];
        let mut items = Vec::new();
        for (cpu_index, cpu) in values.into_iter().enumerate() {
            for (memory_index, memory) in values.into_iter().enumerate() {
                items.push(process(
                    (cpu_index * values.len() + memory_index) as u32,
                    cpu,
                    memory,
                    (memory_index % 3) as u64,
                ));
            }
        }
        items.push(items[0].clone());
        for key in [
            RemoteProcessSortKey::Cpu,
            RemoteProcessSortKey::Memory,
            RemoteProcessSortKey::Pid,
            RemoteProcessSortKey::User,
            RemoteProcessSortKey::Command,
        ] {
            for direction in [
                RemoteProcessSortDirection::Ascending,
                RemoteProcessSortDirection::Descending,
            ] {
                let orders: Vec<Vec<_>> = items
                    .iter()
                    .map(|left| {
                        items
                            .iter()
                            .map(|right| compare_processes(left, right, key, direction))
                            .collect()
                    })
                    .collect();
                for a in 0..items.len() {
                    assert_eq!(orders[a][a], Ordering::Equal);
                    for b in 0..items.len() {
                        assert_eq!(orders[a][b], orders[b][a].reverse());
                        for c in 0..items.len() {
                            if orders[a][b].is_le() && orders[b][c].is_le() {
                                assert!(
                                    orders[a][c].is_le(),
                                    "non-transitive: {key:?}, {direction:?}, {a}/{b}/{c}"
                                );
                            }
                            if orders[a][b] == Ordering::Equal {
                                assert_eq!(orders[a][c], orders[b][c]);
                            }
                        }
                    }
                }
                let mut sorted = items.clone();
                sorted.reverse();
                sort_processes(&mut sorted, key, direction);
                for (index, left) in sorted.iter().enumerate() {
                    for right in &sorted[index..] {
                        assert!(compare_processes(left, right, key, direction).is_le());
                    }
                }
            }
        }
    }

    #[test]
    fn metric_total_order_includes_infinities_and_signed_zero() {
        let values = [
            -f64::NAN,
            f64::NEG_INFINITY,
            -1.0,
            -0.0,
            0.0,
            1.0,
            f64::INFINITY,
            f64::NAN,
        ];
        for key in [RemoteProcessSortKey::Cpu, RemoteProcessSortKey::Memory] {
            let mut items: Vec<_> = values
                .into_iter()
                .enumerate()
                .map(|(index, value)| process(index as u32, value, value, 0))
                .collect();
            items.reverse();
            sort_processes(&mut items, key, RemoteProcessSortDirection::Ascending);
            assert_eq!(
                items.iter().map(|item| item.pid).collect::<Vec<_>>(),
                (0..8).collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn process_metric_ties_keep_secondary_fields_and_pid_order() {
        let mut items = vec![
            process(4, 1.0, 3.0, 1),
            process(3, 1.0, 2.0, 2),
            process(2, 1.0, 2.0, 1),
            process(1, 1.0, 2.0, 1),
        ];
        for key in [RemoteProcessSortKey::Cpu, RemoteProcessSortKey::Memory] {
            sort_processes(&mut items, key, RemoteProcessSortDirection::Ascending);
            assert_eq!(
                items.iter().map(|item| item.pid).collect::<Vec<_>>(),
                [1, 2, 3, 4]
            );
            sort_processes(&mut items, key, RemoteProcessSortDirection::Descending);
            assert_eq!(
                items.iter().map(|item| item.pid).collect::<Vec<_>>(),
                [4, 3, 2, 1]
            );
        }
    }
}
