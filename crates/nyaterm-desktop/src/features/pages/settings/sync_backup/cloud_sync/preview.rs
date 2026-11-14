use std::hash::{Hash, Hasher};

use gpui::{
    AnyElement, AnyView, App, ClickEvent, Context, IntoElement, SharedString, Window, div,
    prelude::*, px, rgb,
};
use nyaterm_ui::{NyaCheckbox, NyaScrollable as _, NyaTooltip};
use rust_i18n::t;

use crate::features::pages::settings::panel::SettingsPanel;
use crate::features::sync::V4ConflictSummary as ConflictSummary;
use crate::theme::ThemePalette;
use crate::widgets::small_button;

/// Candidate previews are capped so text layout stays cheap, but the rendered
/// element width decides where the ellipsis lands. Hovering a preview reveals
/// the complete value.
const CANDIDATE_CHARS: usize = 120;
/// Full values wrap within a fixed width while row labels stay truncated.
const TOOLTIP_MAX_WIDTH: f32 = 420.;
/// Group headers name this many member fields in their hover tooltip before
/// switching to a count.
const GROUP_TOOLTIP_FIELDS: usize = 12;

/// Stable fingerprint of a conflict summary. Staged selections and choices are
/// dropped whenever it changes: a new preview or an applied batch re-derives
/// the conflict list, so old indices would point at different fields.
fn conflict_signature(conflicts: &[ConflictSummary]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for conflict in conflicts {
        conflict.field.hash(&mut hasher);
        conflict.candidate_ids.hash(&mut hasher);
        conflict.operation_ids.hash(&mut hasher);
        conflict.local_candidate.hash(&mut hasher);
    }
    hasher.finish()
}

pub(in crate::features::pages::settings) struct ConflictGroup {
    pub choices: Vec<String>,
    pub candidate_ids: Vec<String>,
    pub conflicts: Vec<ConflictSummary>,
}

/// Conflicts whose candidate sets are identical collapse into one group. In a
/// large sync this routinely reduces hundreds of fields to a few cards.
fn candidate_set(ids: &[String]) -> std::collections::BTreeSet<&str> {
    ids.iter().map(String::as_str).collect()
}

fn group_conflicts(conflicts: &[ConflictSummary]) -> Vec<ConflictGroup> {
    let mut groups: Vec<ConflictGroup> = Vec::new();
    for conflict in conflicts {
        match groups.iter_mut().find(|group| {
            candidate_set(&group.candidate_ids) == candidate_set(&conflict.candidate_ids)
        }) {
            Some(group) => group.conflicts.push(conflict.clone()),
            None => groups.push(ConflictGroup {
                choices: conflict.choices.clone(),
                candidate_ids: conflict.candidate_ids.clone(),
                conflicts: vec![conflict.clone()],
            }),
        }
    }
    groups
}

fn truncated(label: &str, max: usize) -> String {
    if label.chars().count() <= max {
        return label.to_string();
    }
    format!("{}…", label.chars().take(max).collect::<String>())
}

/// Tooltip that reveals a complete sync value or field path on hover. The
/// content wraps inside a fixed maximum width instead of rendering as one
/// extremely wide line.
fn full_text_tooltip(text: String) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    move |window, cx| {
        NyaTooltip::element({
            let text = text.clone();
            move |_, _| {
                div()
                    .max_w(px(TOOLTIP_MAX_WIDTH))
                    .whitespace_normal()
                    .child(text.clone())
            }
        })
        .build(window, cx)
    }
}

/// Hover content for a group header: the fields the group covers. Large groups
/// list a head of their members and count the rest, so a group of hundreds of
/// fields still produces a readable tooltip.
fn group_tooltip_fields(fields: &[String]) -> (Vec<String>, usize) {
    let listed: Vec<String> = fields.iter().take(GROUP_TOOLTIP_FIELDS).cloned().collect();
    let remaining = fields.len().saturating_sub(listed.len());
    (listed, remaining)
}

fn group_fields_tooltip(
    fields: Vec<String>,
) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    move |window, cx| {
        let (listed, remaining) = group_tooltip_fields(&fields);
        NyaTooltip::element(move |_, _| {
            let mut column = div().flex().flex_col().gap_1().max_w(px(TOOLTIP_MAX_WIDTH));
            for field in &listed {
                column = column.child(
                    div()
                        .whitespace_nowrap()
                        .font_family(crate::features::shell::gpui_code_font_family())
                        .child(field.clone()),
                );
            }
            if remaining > 0 {
                column = column.child(div().text_xs().child(
                    t!("settings.syncConflictGroupMoreFields", count = remaining).to_string(),
                ));
            }
            column
        })
        .build(window, cx)
    }
}

/// The per-group "apply to all" control. Instead of the shared small button it
/// highlights itself on hover with an accent border, so the pointer clearly
/// marks which candidate value the click would apply. All colors come from the
/// active theme palette.
fn group_apply_button(
    palette: ThemePalette,
    id: String,
    label: impl Into<SharedString>,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    small_button(palette, id, label, on_click)
}

impl SettingsPanel {
    pub(super) fn cloud_sync_preview(
        &mut self,
        actions_enabled: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = self.theme_palette();
        let Some(summary) = self.cloud_sync.v4_preview.clone() else {
            return div().into_any_element();
        };
        let signature = conflict_signature(&summary.conflicts);
        if self.cloud_sync_conflict_signature != signature {
            self.cloud_sync_conflict_signature = signature;
            self.cloud_sync_conflict_selection.clear();
            self.cloud_sync_conflict_choices.clear();
        }
        let conflicts = summary.conflicts.clone();
        let groups = group_conflicts(&conflicts);
        let selection = self.cloud_sync_conflict_selection.clone();
        let choices = self.cloud_sync_conflict_choices.clone();
        // The list scrolls inside the settings page: give it most of the
        // remaining window height instead of a fixed guess, so the toolbar and
        // the confirm button stay visible on any window size.
        let list_max_height = px((self.pane_height - 320.).clamp(240., 760.));

        let mut content = div().flex().flex_col().gap_3();
        if actions_enabled {
            content = content.child(small_button(
                palette,
                "sync-v3-preview",
                t!("settings.syncV3Preview"),
                cx.listener(|this, _, _, cx| {
                    this.with_app(cx, |app, cx| app.run_cloud_sync_v3_preview(cx));
                }),
            ));
        }
        content = content.child(div().text_sm().child(t!(
            "settings.syncV3PreviewCounts",
            resources = summary.resources,
            conflicts = conflicts.len()
        )));
        if summary.migration {
            content = content.child(div().text_sm().child(t!("settings.syncV3MigrationNotice")));
        }
        if !summary.deferred.is_empty() {
            content = content.child(div().text_sm().child(t!(
                "settings.syncV3Deferred",
                count = summary.deferred.len()
            )));
            for field in &summary.deferred {
                content = content.child(div().text_sm().child(field.clone()));
            }
        }

        if !conflicts.is_empty() {
            let total = conflicts.len();
            let selected_count = selection.len();
            let staged_count = choices.len();
            let all_selected = selected_count == total;

            let mut toolbar = div().flex().flex_wrap().items_center().gap_2();
            toolbar = toolbar.child(
                NyaCheckbox::new("sync-conflict-select-all")
                    .label(t!("settings.syncConflictSelectAll"))
                    .checked(all_selected)
                    .indeterminate(selected_count > 0 && !all_selected)
                    .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                        if *checked {
                            for index in 0..total {
                                this.cloud_sync_conflict_selection.insert(index);
                            }
                        } else {
                            this.cloud_sync_conflict_selection.clear();
                            this.cloud_sync_conflict_choices.clear();
                        }
                        cx.notify();
                    })),
            );
            if actions_enabled {
                let keep_local = conflicts.clone();
                let keep_remote = conflicts.clone();
                toolbar = toolbar
                    .child(small_button(
                        palette,
                        "sync-conflict-clear",
                        t!("settings.syncConflictClearSelection"),
                        cx.listener(|this, _, _, cx| {
                            this.cloud_sync_conflict_selection.clear();
                            this.cloud_sync_conflict_choices.clear();
                            cx.notify();
                        }),
                    ))
                    .child(small_button(
                        palette,
                        "sync-conflict-keep-local",
                        t!("settings.syncConflictKeepLocal"),
                        cx.listener(move |this, _, _, cx| {
                            for conflict in &keep_local {
                                let Some(candidate) = conflict.local_candidate else {
                                    continue;
                                };
                                if this.cloud_sync_conflict_selection.contains(&conflict.index) {
                                    this.cloud_sync_conflict_choices
                                        .insert(conflict.index, candidate);
                                }
                            }
                            cx.notify();
                        }),
                    ))
                    .child(small_button(
                        palette,
                        "sync-conflict-keep-remote",
                        t!("settings.syncConflictKeepRemote"),
                        cx.listener(move |this, _, _, cx| {
                            for conflict in &keep_remote {
                                if !this.cloud_sync_conflict_selection.contains(&conflict.index) {
                                    continue;
                                }
                                // Any candidate other than the proven local one
                                // is a remote value; deleted candidates qualify.
                                if let Some(candidate) = (0..conflict.choices.len())
                                    .find(|index| Some(*index) != conflict.local_candidate)
                                {
                                    this.cloud_sync_conflict_choices
                                        .insert(conflict.index, candidate);
                                }
                            }
                            cx.notify();
                        }),
                    ));
            }
            toolbar = toolbar.child(
                div().text_xs().text_color(rgb(palette.text_muted)).child(
                    t!(
                        "settings.syncConflictSelectionSummary",
                        selected = selected_count,
                        total = total,
                        staged = staged_count
                    )
                    .to_string(),
                ),
            );
            content = content.child(toolbar);

            // Groups are the primary surface; the per-field list is opt-in.
            let rows_visible = self.cloud_sync_conflict_rows_visible;
            content = content.child(small_button(
                palette,
                "sync-conflict-toggle-details",
                if rows_visible {
                    t!("settings.syncConflictHideDetails").to_string()
                } else {
                    t!("settings.syncConflictShowDetails", count = total).to_string()
                },
                cx.listener(|this, _, _, cx| {
                    this.cloud_sync_conflict_rows_visible = !this.cloud_sync_conflict_rows_visible;
                    cx.notify();
                }),
            ));

            let mut group_section = div().flex().flex_col().gap_2();
            for (group_index, group) in groups.iter().enumerate() {
                let member_indices: Vec<usize> = group.conflicts.iter().map(|c| c.index).collect();
                let member_fields: Vec<String> =
                    group.conflicts.iter().map(|c| c.field.clone()).collect();
                let member_count = member_indices.len();
                let checkbox_members = member_indices.clone();
                let all_members = member_indices.iter().all(|index| selection.contains(index));
                let mut card = div()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(palette.border))
                    .bg(rgb(palette.surface_elevated))
                    .p_2()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                NyaCheckbox::new(format!("sync-conflict-group-{group_index}"))
                                    .label(t!("settings.syncConflictSelectGroup"))
                                    .checked(all_members)
                                    .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                                        if *checked {
                                            for index in &checkbox_members {
                                                this.cloud_sync_conflict_selection.insert(*index);
                                            }
                                        } else {
                                            for index in &checkbox_members {
                                                this.cloud_sync_conflict_selection.remove(index);
                                                this.cloud_sync_conflict_choices.remove(index);
                                            }
                                        }
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div()
                                    .id(SharedString::from(format!(
                                        "sync-conflict-group-fields-{group_index}"
                                    )))
                                    .text_xs()
                                    .child(
                                        t!(
                                            "settings.syncConflictGroupTitle",
                                            group = group_index + 1,
                                            count = member_count
                                        )
                                        .to_string(),
                                    )
                                    .tooltip(group_fields_tooltip(member_fields)),
                            ),
                    );
                for (candidate_index, label) in group.choices.iter().enumerate() {
                    let selected_id = &group.candidate_ids[candidate_index];
                    let members = group
                        .conflicts
                        .iter()
                        .filter_map(|conflict| {
                            conflict
                                .candidate_ids
                                .iter()
                                .position(|id| id == selected_id)
                                .map(|candidate| (conflict.index, candidate))
                        })
                        .collect::<Vec<_>>();
                    card = card.child(
                        div()
                            .id(SharedString::from(format!(
                                "sync-conflict-group-row-{group_index}-{candidate_index}"
                            )))
                            .flex()
                            .items_center()
                            .gap_2()
                            .rounded_md()
                            .px_1()
                            // Hovering anywhere on the line — the value or the
                            // button that applies it — lights the whole row up.
                            .hover(|style| style.bg(rgb(palette.hover)))
                            .child(
                                div()
                                    .id(SharedString::from(format!(
                                        "sync-conflict-group-value-{group_index}-{candidate_index}"
                                    )))
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .text_xs()
                                    .text_color(rgb(palette.text_muted))
                                    .child(truncated(label, CANDIDATE_CHARS))
                                    .tooltip(full_text_tooltip(label.clone())),
                            )
                            .child(group_apply_button(
                                palette,
                                format!("sync-conflict-group-{group_index}-{candidate_index}"),
                                t!("settings.syncConflictApplyGroupChoice"),
                                cx.listener(move |this, _, _, cx| {
                                    for (index, candidate) in &members {
                                        this.cloud_sync_conflict_selection.insert(*index);
                                        this.cloud_sync_conflict_choices.insert(*index, *candidate);
                                    }
                                    cx.notify();
                                }),
                            )),
                    );
                }
                group_section = group_section.child(card);
            }
            content = content.child(group_section);

            if rows_visible {
                let mut rows = div().flex().flex_col().gap_2();
                for conflict in &conflicts {
                    let index = conflict.index;
                    let staged = choices.get(&index).copied();
                    let mut row = div()
                        .rounded_md()
                        .border_1()
                        .border_color(rgb(palette.border))
                        .bg(rgb(palette.input))
                        .p_2()
                        .flex()
                        .flex_col()
                        .gap_1();
                    if selection.contains(&index) {
                        row = row.border_color(rgb(palette.accent));
                    }
                    row = row.child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                NyaCheckbox::new(format!("sync-conflict-row-{index}"))
                                    .checked(selection.contains(&index))
                                    .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                                        if *checked {
                                            this.cloud_sync_conflict_selection.insert(index);
                                        } else {
                                            this.cloud_sync_conflict_selection.remove(&index);
                                            this.cloud_sync_conflict_choices.remove(&index);
                                        }
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div()
                                    .id(SharedString::from(format!("sync-conflict-field-{index}")))
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .text_xs()
                                    .font_family(crate::features::shell::gpui_code_font_family())
                                    .child(conflict.field.clone())
                                    .tooltip(full_text_tooltip(conflict.field.clone())),
                            ),
                    );
                    for (candidate_index, label) in conflict.choices.iter().enumerate() {
                        let staged_mark = if staged == Some(candidate_index) {
                            "✓ "
                        } else {
                            ""
                        };
                        row = row.child(small_button(
                            palette,
                            format!("sync-conflict-choice-{index}-{candidate_index}"),
                            format!("{staged_mark}{}", truncated(label, CANDIDATE_CHARS)),
                            cx.listener(move |this, _, _, cx| {
                                // Clicking the staged candidate again clears the
                                // choice so a mistake is reversible without a job.
                                if this.cloud_sync_conflict_choices.get(&index).copied()
                                    == Some(candidate_index)
                                {
                                    this.cloud_sync_conflict_choices.remove(&index);
                                } else {
                                    this.cloud_sync_conflict_choices
                                        .insert(index, candidate_index);
                                    this.cloud_sync_conflict_selection.insert(index);
                                }
                                cx.notify();
                            }),
                        ));
                    }
                    rows = rows.child(row);
                }
                content = content.child(
                    div()
                        .debug_selector(|| "cloud-sync-conflict-scroll".to_string())
                        .max_h(list_max_height)
                        .child(
                            div()
                                .max_h(list_max_height)
                                .overflow_y_scrollbar()
                                .child(rows),
                        ),
                );
            }

            if actions_enabled && staged_count > 0 {
                let decisions: Vec<(usize, usize)> =
                    choices.iter().map(|(c, v)| (*c, *v)).collect();
                content = content.child(small_button(
                    palette,
                    "sync-conflict-apply-staged",
                    t!("settings.syncConflictApplyStaged", count = staged_count),
                    cx.listener(move |this, _, _, cx| {
                        this.with_app(cx, |app, cx| {
                            app.resolve_cloud_sync_v3_preview_batch(decisions.clone(), cx)
                        });
                    }),
                ));
            }
        }

        if actions_enabled {
            content = content.child(small_button(
                palette,
                "sync-v3-apply",
                t!("settings.syncV3Apply"),
                cx.listener(|this, _, _, cx| {
                    this.with_app(cx, |app, cx| app.apply_cloud_sync_v3_preview(cx));
                }),
            ));
        }
        content.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        GROUP_TOOLTIP_FIELDS, conflict_signature, group_conflicts, group_tooltip_fields, truncated,
    };
    use crate::features::sync::V4ConflictSummary as ConflictSummary;

    fn conflict(index: usize, field: &str, choices: &[&str]) -> ConflictSummary {
        ConflictSummary {
            index,
            field: field.to_string(),
            choices: choices.iter().map(|c| (*c).to_string()).collect(),
            candidate_ids: choices.iter().map(|c| (*c).to_string()).collect(),
            operation_ids: choices.iter().map(|c| (*c).to_string()).collect(),
            local_candidate: None,
        }
    }

    #[test]
    fn equal_display_labels_do_not_merge_distinct_values() {
        let label = "x".repeat(160);
        let first = conflict(0, "first", &[&label, "other"]);
        let mut second = conflict(1, "second", &[&label, "other"]);
        second.candidate_ids[0].push_str("-different");
        assert_eq!(group_conflicts(&[first, second]).len(), 2);
    }

    #[test]
    fn reordered_candidates_group_by_value_and_stale_operations_reset_selection() {
        let first = conflict(0, "first", &["one", "two"]);
        let mut second = conflict(1, "second", &["two", "one"]);
        let groups = group_conflicts(&[first.clone(), second.clone()]);
        assert_eq!(groups.len(), 1);
        assert_eq!(
            groups[0].conflicts[1]
                .candidate_ids
                .iter()
                .position(|id| id == &groups[0].candidate_ids[0]),
            Some(1)
        );
        let signature = conflict_signature(&[first.clone(), second.clone()]);
        second.operation_ids[0] = "new operation for the same value".into();
        assert_ne!(signature, conflict_signature(&[first, second]));
    }

    #[test]
    fn identical_candidate_sets_collapse_into_one_group() {
        let conflicts = vec![
            conflict(0, "connection / a / terminal", &["one", "two"]),
            conflict(1, "connection / b / terminal", &["one", "two"]),
            conflict(2, "connection / c / name", &["three"]),
            conflict(3, "connection / d / name", &["one", "two"]),
        ];
        let groups = group_conflicts(&conflicts);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].choices, vec!["one", "two"]);
        assert_eq!(
            groups[0]
                .conflicts
                .iter()
                .map(|c| c.index)
                .collect::<Vec<_>>(),
            vec![0, 1, 3]
        );
        assert_eq!(groups[1].choices, vec!["three"]);
    }

    #[test]
    fn signature_changes_when_conflicts_or_choices_change() {
        let base = vec![conflict(0, "connection / a / name", &["one", "two"])];
        let same = vec![conflict(7, "connection / a / name", &["one", "two"])];
        let different_choices = vec![conflict(0, "connection / a / name", &["one", "three"])];
        let removed = vec![];
        assert_eq!(conflict_signature(&base), conflict_signature(&same));
        assert_ne!(
            conflict_signature(&base),
            conflict_signature(&different_choices)
        );
        assert_ne!(conflict_signature(&base), conflict_signature(&removed));
    }

    #[test]
    fn truncation_keeps_short_values_and_marks_cut_ones() {
        assert_eq!(truncated("abc", 10), "abc");
        let cut = truncated("abcdef", 3);
        assert!(cut.starts_with("abc"));
        assert!(cut.ends_with('…'));
    }

    #[test]
    fn group_tooltip_lists_a_head_and_counts_the_remaining_fields() {
        let fields: Vec<String> = (0..20)
            .map(|index| format!("connection / {index} / terminal"))
            .collect();
        let (listed, remaining) = group_tooltip_fields(&fields);
        assert_eq!(listed.len(), GROUP_TOOLTIP_FIELDS);
        assert_eq!(remaining, 20 - GROUP_TOOLTIP_FIELDS);
        assert_eq!(listed[0], "connection / 0 / terminal");

        let (listed, remaining) = group_tooltip_fields(&fields[..3]);
        assert_eq!(listed.len(), 3);
        assert_eq!(remaining, 0);
    }
}
