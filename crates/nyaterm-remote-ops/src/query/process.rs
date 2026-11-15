use crate::remote_process::RemoteProcess;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessSortKey {
    Cpu,
    Memory,
    Pid,
    User,
    Command,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortDirection {
    Ascending,
    Descending,
}

pub fn process_search_key(process: &RemoteProcess) -> String {
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

pub fn sort_processes(
    processes: &mut [RemoteProcess],
    key: ProcessSortKey,
    direction: SortDirection,
) {
    processes.sort_by(|left, right| compare_processes(left, right, key, direction));
}

fn compare_processes(
    left: &RemoteProcess,
    right: &RemoteProcess,
    key: ProcessSortKey,
    direction: SortDirection,
) -> std::cmp::Ordering {
    // Remote metrics may contain NaN. Treating unordered values as equal makes
    // the secondary keys create cycles with otherwise ordered numeric values.
    let ordering = match key {
        ProcessSortKey::Command => left
            .command
            .cmp(&right.command)
            .then_with(|| left.pid.cmp(&right.pid)),
        ProcessSortKey::Memory => left
            .memory_percent
            .total_cmp(&right.memory_percent)
            .then_with(|| left.rss_kb.cmp(&right.rss_kb))
            .then_with(|| left.pid.cmp(&right.pid)),
        ProcessSortKey::Pid => left.pid.cmp(&right.pid),
        ProcessSortKey::User => left
            .user
            .cmp(&right.user)
            .then_with(|| left.pid.cmp(&right.pid)),
        ProcessSortKey::Cpu => left
            .cpu_percent
            .total_cmp(&right.cpu_percent)
            .then_with(|| left.memory_percent.total_cmp(&right.memory_percent))
            .then_with(|| left.pid.cmp(&right.pid)),
    };

    match direction {
        SortDirection::Ascending => ordering,
        SortDirection::Descending => ordering.reverse(),
    }
}

#[cfg(test)]
mod sort_tests {
    use super::{ProcessSortKey, SortDirection};
    use super::{compare_processes, sort_processes};
    use crate::remote_process::RemoteProcess;
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
        for key in [ProcessSortKey::Cpu, ProcessSortKey::Memory] {
            let mut items = vec![
                process(2, f64::NAN, f64::NAN, 0),
                process(1, 1.0, 1.0, 0),
                process(3, 0.0, 0.0, 0),
            ];
            sort_processes(&mut items, key, SortDirection::Ascending);
            assert_eq!(
                items.iter().map(|item| item.pid).collect::<Vec<_>>(),
                [3, 1, 2]
            );
            sort_processes(&mut items, key, SortDirection::Descending);
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
            ProcessSortKey::Cpu,
            ProcessSortKey::Memory,
            ProcessSortKey::Pid,
            ProcessSortKey::User,
            ProcessSortKey::Command,
        ] {
            for direction in [SortDirection::Ascending, SortDirection::Descending] {
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
        for key in [ProcessSortKey::Cpu, ProcessSortKey::Memory] {
            let mut items: Vec<_> = values
                .into_iter()
                .enumerate()
                .map(|(index, value)| process(index as u32, value, value, 0))
                .collect();
            items.reverse();
            sort_processes(&mut items, key, SortDirection::Ascending);
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
        for key in [ProcessSortKey::Cpu, ProcessSortKey::Memory] {
            sort_processes(&mut items, key, SortDirection::Ascending);
            assert_eq!(
                items.iter().map(|item| item.pid).collect::<Vec<_>>(),
                [1, 2, 3, 4]
            );
            sort_processes(&mut items, key, SortDirection::Descending);
            assert_eq!(
                items.iter().map(|item| item.pid).collect::<Vec<_>>(),
                [4, 3, 2, 1]
            );
        }
    }
}
