use std::collections::{BTreeMap, BTreeSet, HashMap};

use nyaterm_core::ai::{AiAuditLog, sanitize_ai_diagnostic};

pub(in crate::features) const PAGE_SIZE: usize = 50;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::features) enum ActivityOutcome {
    Success,
    Failed,
    Denied,
    Event,
}
impl ActivityOutcome {
    pub fn key(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failed => "failed",
            Self::Denied => "denied",
            Self::Event => "event",
        }
    }
    pub fn from_key(value: &str) -> Option<Self> {
        [Self::Success, Self::Failed, Self::Denied, Self::Event]
            .into_iter()
            .find(|item| item.key() == value)
    }
}

#[derive(Clone, Default, PartialEq, Eq)]
pub(in crate::features) struct ActivityFilter {
    pub client: Option<String>,
    pub target: Option<String>,
    pub outcome: Option<ActivityOutcome>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::features) struct ActivityTarget {
    pub key: String,
    pub id: String,
    pub name: Option<String>,
    pub connection: bool,
}

/// Deliberately excludes command, arguments, output and raw error text, including from old records.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::features) struct ActivityRow {
    pub id: String,
    pub created_at: String,
    pub client: Option<String>,
    pub target: Option<ActivityTarget>,
    pub tool: Option<String>,
    pub event: &'static str,
    pub outcome: ActivityOutcome,
    pub duration_ms: Option<u64>,
}

#[derive(Clone, Default, PartialEq, Eq)]
pub(in crate::features) struct ActivityPage {
    pub rows: Vec<ActivityRow>,
    pub clients: Vec<String>,
    pub targets: Vec<ActivityTarget>,
    pub retained_count: usize,
    pub matched_count: usize,
    pub page: usize,
}

fn outcome(log: &AiAuditLog) -> ActivityOutcome {
    if matches!(
        log.approval_decision.as_deref(),
        Some("deny" | "denied_by_policy" | "denied_before_execution")
    ) {
        ActivityOutcome::Denied
    } else {
        match log.success {
            Some(true) => ActivityOutcome::Success,
            Some(false) => ActivityOutcome::Failed,
            None if log.blocked => ActivityOutcome::Denied,
            None => ActivityOutcome::Event,
        }
    }
}

pub(in crate::features) fn project_activity(
    logs: Vec<AiAuditLog>,
    names: &HashMap<String, String>,
    filter: &ActivityFilter,
    requested_page: usize,
) -> ActivityPage {
    let mut rows: Vec<_> = logs
        .into_iter()
        .filter(|log| {
            log.source.as_deref() == Some("mcp")
                || (log.source.is_none() && log.action.starts_with("mcp."))
        })
        .map(|log| {
            let target = log
                .connection_id
                .as_ref()
                .map(|id| (true, id))
                .or_else(|| log.session_id.as_ref().map(|id| (false, id)))
                .map(|(connection, id)| {
                    let key = format!("{}:{id}", if connection { "connection" } else { "session" });
                    ActivityTarget {
                        name: names
                            .get(&key)
                            .map(|name| sanitize_ai_diagnostic(name, 160)),
                        key,
                        id: sanitize_ai_diagnostic(id, 128),
                        connection,
                    }
                });
            ActivityRow {
                outcome: outcome(&log),
                id: log.id,
                created_at: log.created_at,
                client: log
                    .client
                    .map(|client| sanitize_ai_diagnostic(&client, 128)),
                target,
                tool: log.capability.map(|tool| sanitize_ai_diagnostic(&tool, 80)),
                event: match log.action.as_str() {
                    "mcp.capability" => "capability",
                    "mcp.job_registered" => "jobRegistered",
                    "mcp.interrupt_sent" => "interruptSent",
                    "mcp.remote_exit_confirmed" => "exitConfirmed",
                    _ => "otherEvent",
                },
                duration_ms: log.duration_ms,
            }
        })
        .collect();
    rows.sort_by(|a, b| b.created_at.cmp(&a.created_at).then(b.id.cmp(&a.id)));
    let mut result = ActivityPage {
        clients: rows
            .iter()
            .filter_map(|row| row.client.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
        targets: rows
            .iter()
            .filter_map(|row| row.target.clone())
            .map(|target| (target.key.clone(), target))
            .collect::<BTreeMap<_, _>>()
            .into_values()
            .collect(),
        retained_count: rows.len(),
        ..Default::default()
    };
    rows.retain(|row| {
        filter
            .client
            .as_ref()
            .is_none_or(|value| row.client.as_ref() == Some(value))
            && filter.target.as_ref().is_none_or(|value| {
                row.target
                    .as_ref()
                    .is_some_and(|target| &target.key == value)
            })
            && filter.outcome.is_none_or(|value| row.outcome == value)
    });
    result.matched_count = rows.len();
    result.page = requested_page.min(rows.len().saturating_sub(1) / PAGE_SIZE);
    result.rows = rows
        .into_iter()
        .skip(result.page * PAGE_SIZE)
        .take(PAGE_SIZE)
        .collect();
    result
}

#[cfg(test)]
mod tests {
    use super::{ActivityFilter, ActivityOutcome, PAGE_SIZE, project_activity};
    use nyaterm_core::ai::AiAuditLog;
    use std::collections::HashMap;

    fn record(id: usize) -> AiAuditLog {
        AiAuditLog {
            id: format!("id-{id:04}"),
            action: "mcp.capability".into(),
            source: Some("mcp".into()),
            created_at: "2026-01-01T00:00:00Z".into(),
            client: Some("client".into()),
            connection_id: Some("gone".into()),
            user_input: Some("private input".into()),
            generated_command: Some("private command".into()),
            error: Some("private error".into()),
            ..Default::default()
        }
    }

    #[test]
    fn separates_failed_execution_denial_and_lifecycle_without_private_payloads() {
        let mut failure = record(1);
        failure.success = Some(false);
        failure.blocked = true;
        let mut denial = record(2);
        denial.success = Some(false);
        denial.approval_decision = Some("deny".into());
        let mut event = record(3);
        event.action = "mcp.job_registered".into();
        let page = project_activity(
            vec![failure, denial, event],
            &HashMap::new(),
            &ActivityFilter::default(),
            0,
        );
        assert_eq!(
            page.rows.iter().map(|row| row.outcome).collect::<Vec<_>>(),
            [
                ActivityOutcome::Event,
                ActivityOutcome::Denied,
                ActivityOutcome::Failed
            ]
        );
        assert!(page.rows[0].target.as_ref().unwrap().name.is_none());
        assert!(!format!("{:?}", page.rows).contains("private"));
    }

    #[test]
    fn filters_sources_and_matches_before_paging_and_clamps_removed_pages() {
        let mut logs: Vec<_> = (0..125).map(record).collect();
        logs[0].source = Some("native".into());
        logs[1].source = None; // Legacy MCP stays visible.
        logs[2].source = None;
        logs[2].action = "native.operation".into();
        let filter = ActivityFilter {
            client: Some("client".into()),
            target: Some("connection:gone".into()),
            ..Default::default()
        };
        let page = project_activity(logs.clone(), &HashMap::new(), &filter, 1);
        assert_eq!(page.retained_count, 123);
        assert_eq!(page.matched_count, 123);
        assert_eq!(page.rows.len(), PAGE_SIZE);
        assert_eq!(page.rows[0].id, "id-0074");
        assert_eq!(
            project_activity(logs.clone(), &HashMap::new(), &filter, 999).page,
            2
        );
        let filter = ActivityFilter {
            outcome: Some(ActivityOutcome::Success),
            ..filter
        };
        let page = project_activity(logs, &HashMap::new(), &filter, 999);
        assert_eq!(page.page, 0);
        assert!(page.rows.is_empty());
    }
}
