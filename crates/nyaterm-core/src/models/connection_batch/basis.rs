//! Selection summaries independent of field widgets and localized labels.
use super::{
    ConnectionBatchEdit, ConnectionBatchKind, ConnectionBatchSkipCounts, ConnectionBatchSkipReason,
    RdpBatchFieldGroup, rdp_batch_field_skip_reason,
};
use crate::models::SavedConnection;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BatchValueBasis<T> {
    Inapplicable,
    Mixed,
    Uniform(T),
}

/// Missing values do not participate; the first unequal pair determines Mixed.
pub fn common_value<T: PartialEq>(
    values: impl IntoIterator<Item = Option<T>>,
) -> BatchValueBasis<T> {
    let mut values = values.into_iter().flatten();
    let Some(first) = values.next() else {
        return BatchValueBasis::Inapplicable;
    };
    if values.any(|value| value != first) {
        BatchValueBasis::Mixed
    } else {
        BatchValueBasis::Uniform(first)
    }
}

/// Preserve the batch editor's supported protocol order, independently of pickers.
pub fn connection_kind_counts(
    connections: &[SavedConnection],
    ids: &[String],
) -> Vec<(ConnectionBatchKind, usize)> {
    let by_id: HashMap<_, _> = connections
        .iter()
        .map(|connection| (connection.id.as_str(), connection))
        .collect();
    let mut counts = HashMap::<ConnectionBatchKind, usize>::new();
    for id in ids {
        if let Some(connection) = by_id.get(id.as_str()) {
            *counts
                .entry(ConnectionBatchKind::from_config(&connection.config))
                .or_default() += 1;
        }
    }
    [
        ConnectionBatchKind::Ssh,
        ConnectionBatchKind::LocalTerminal,
        ConnectionBatchKind::Telnet,
        ConnectionBatchKind::Serial,
        ConnectionBatchKind::Rdp,
        ConnectionBatchKind::Vnc,
    ]
    .into_iter()
    .filter_map(|kind| counts.get(&kind).map(|count| (kind, *count)))
    .collect()
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BatchFieldApplicability {
    pub total: usize,
    pub applicable: usize,
    pub skipped_reasons: ConnectionBatchSkipCounts,
}

pub fn rdp_field_basis<T: PartialEq>(
    connections: &[SavedConnection],
    targets: &[(String, ConnectionBatchKind)],
    group: RdpBatchFieldGroup,
    edit: &ConnectionBatchEdit,
    mut project: impl FnMut(&SavedConnection) -> T,
) -> (BatchValueBasis<T>, BatchFieldApplicability) {
    let by_id: HashMap<_, _> = connections
        .iter()
        .map(|connection| (connection.id.as_str(), connection))
        .collect();
    let mut applicability = BatchFieldApplicability::default();
    let mut first = None;
    let mut mixed = false;
    // Count every applicable/skipped member even after the first unequal pair.
    for (id, kind) in targets {
        if *kind != ConnectionBatchKind::Rdp {
            continue;
        }
        applicability.total += 1;
        let connection = by_id.get(id.as_str()).copied();
        let reason = connection.map_or(
            Some(ConnectionBatchSkipReason::UnsupportedProtocol),
            |connection| rdp_batch_field_skip_reason(&connection.config, edit, group),
        );
        if let Some(reason) = reason {
            applicability.skipped_reasons.add(reason, 1);
            continue;
        }
        applicability.applicable += 1;
        let value = project(connection.expect("applicable connection"));
        match &first {
            None => first = Some(value),
            Some(existing) if *existing != value => mixed = true,
            _ => {}
        }
    }
    let basis = if mixed {
        BatchValueBasis::Mixed
    } else {
        first.map_or(BatchValueBasis::Inapplicable, BatchValueBasis::Uniform)
    };
    (basis, applicability)
}

#[cfg(test)]
mod tests {
    use super::{BatchValueBasis, common_value};
    #[test]
    fn missing_fields_do_not_turn_a_uniform_selection_into_a_mixed_one() {
        assert_eq!(
            common_value([None::<u32>, None]),
            BatchValueBasis::Inapplicable
        );
        assert_eq!(
            common_value([None, Some(3), Some(3)]),
            BatchValueBasis::Uniform(3)
        );
        assert_eq!(
            common_value([Some(3), None, Some(4)]),
            BatchValueBasis::Mixed
        );
    }
}
