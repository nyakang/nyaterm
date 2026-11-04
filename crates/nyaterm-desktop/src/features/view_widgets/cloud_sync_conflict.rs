use crate::features::formatting::{compact_id, format_history_timestamp_ms};
use crate::models::CloudSyncConflictState;
use gpui::{AnyElement, IntoElement, div, prelude::*, px};
use nyaterm_ui::{NyaScrollable as _, NyaSelectOption};
use rust_i18n::t;

pub(in crate::features) fn branch_options(
    conflict: &CloudSyncConflictState,
) -> Vec<NyaSelectOption> {
    conflict
        .preview
        .branches
        .iter()
        .filter(|b| b.readable)
        .map(|branch| {
            let device = branch
                .device_name
                .clone()
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| compact_id(&branch.device_id));
            NyaSelectOption::new(
                branch.revision.clone(),
                format!(
                    "{} / {} / {}",
                    device,
                    format_history_timestamp_ms(branch.created_at_ms),
                    compact_id(&branch.revision)
                ),
            )
        })
        .collect()
}

pub(in crate::features) fn branch_details(
    conflict: &CloudSyncConflictState,
    select: impl IntoElement,
) -> AnyElement {
    let preview = &conflict.preview;
    let selected_revision = &conflict.selected_revision;
    let selected = preview
        .branches
        .iter()
        .find(|b| Some(&b.revision) == selected_revision.as_ref());
    let mut branch_details = div().flex().flex_col().gap_2();
    for branch in &preview.branches {
        let device = branch
            .device_name
            .clone()
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| compact_id(&branch.device_id));
        branch_details = branch_details.child(div().text_xs().child(format!(
            "{} / {} / {} / {}",
            device,
            compact_id(&branch.revision),
            format_history_timestamp_ms(branch.created_at_ms),
            if branch.readable {
                t!("settings.syncBranchReadable")
            } else {
                t!("settings.syncBranchUnreadable")
            }
        )));
    }
    if let Some(branch) = selected {
        for difference in &branch.differences {
            let label = t!(format!(
                "settings.syncEntity.{}",
                difference.entity.replace('.', "_")
            ));
            branch_details = branch_details.child(
                div().text_xs().child(
                    t!(
                        "settings.syncBranchDifference",
                        entity = label,
                        added = difference.added,
                        removed = difference.removed,
                        modified = difference.modified
                    )
                    .to_string(),
                ),
            );
        }
        if branch.differences.is_empty() {
            branch_details =
                branch_details.child(div().text_xs().child(t!("settings.syncBranchIdentical")));
        }
    }
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap_2()
        .child(div().text_sm().child(t!("settings.syncChooseBranch")))
        .child(select)
        .child(
            div()
                .debug_selector(|| "cloud-sync-branch-scroll".to_string())
                .max_h(px(260.))
                .child(
                    div()
                        .max_h(px(260.))
                        .overflow_y_scrollbar()
                        .child(branch_details),
                ),
        )
        .child(div().text_xs().child(t!("settings.syncResolveObserved")))
        .into_any_element()
}
