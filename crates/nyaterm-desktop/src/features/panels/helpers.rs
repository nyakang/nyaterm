use std::borrow::Cow;

use gpui::{Context, IntoElement, SharedString, div, prelude::*, px, rgb, svg};
use nyaterm_core::{QuickCommand, QuickCommandCategory, quick_command_category_sibling_order};
use nyaterm_ui::NyaSwitch;
use rust_i18n::t;

use crate::features::{
    NyaTermApp, commands::quick_command_category_label, icons::quick_command_icon,
    text_inputs::TextInputSetup,
};
use crate::models::{QuickCommandEditorField, QuickCommandSortMode};

pub(in crate::features::panels) struct QuickCommandCategoryOption {
    pub id: String,
    pub label: String,
    pub count: usize,
    pub manageable: bool,
    pub depth: usize,
}

pub(in crate::features::panels) fn quick_command_category_options(
    commands: &[QuickCommand],
    categories: &[QuickCommandCategory],
    all_label: impl Into<SharedString>,
    uncategorized_label: impl Into<SharedString>,
) -> Vec<QuickCommandCategoryOption> {
    let all_label: SharedString = all_label.into();
    let uncategorized_label: SharedString = uncategorized_label.into();
    let mut options = vec![QuickCommandCategoryOption {
        id: "all".to_string(),
        label: all_label.to_string(),
        count: commands.len(),
        manageable: false,
        depth: 0,
    }];

    for (category, depth) in ordered_quick_command_categories(categories) {
        let count = commands
            .iter()
            .filter(|command| command.category_id.as_deref() == Some(category.id.as_str()))
            .count();
        options.push(QuickCommandCategoryOption {
            id: category.id.clone(),
            label: category.name.clone(),
            count,
            manageable: true,
            depth,
        });
    }

    let uncategorized = commands
        .iter()
        .filter(|command| {
            command
                .category_id
                .as_deref()
                .unwrap_or_default()
                .is_empty()
        })
        .count();
    options.push(QuickCommandCategoryOption {
        id: "uncategorized".to_string(),
        label: uncategorized_label.to_string(),
        count: uncategorized,
        manageable: false,
        depth: 0,
    });
    options
}

/// Depth-first sidebar order. Sibling order comes from
/// `quick_command_category_sibling_order`, which the group menu's move up/down
/// also uses, so "the row above" and "move up" cannot disagree.
fn ordered_quick_command_categories(
    categories: &[QuickCommandCategory],
) -> Vec<(&QuickCommandCategory, usize)> {
    fn visit<'a>(
        parent_id: Option<&str>,
        depth: usize,
        categories: &'a [QuickCommandCategory],
        visited: &mut std::collections::BTreeSet<String>,
        output: &mut Vec<(&'a QuickCommandCategory, usize)>,
    ) {
        for category in quick_command_category_sibling_order(categories, parent_id) {
            if visited.insert(category.id.clone()) {
                output.push((category, depth));
                visit(
                    Some(&category.id),
                    depth.saturating_add(1),
                    categories,
                    visited,
                    output,
                );
            }
        }
    }

    let mut visited = std::collections::BTreeSet::new();
    let mut output = Vec::with_capacity(categories.len());
    // Roots are `parent_id: None` plus rows whose parent no longer exists, which
    // the sibling-order helper already folds together.
    visit(None, 0, categories, &mut visited, &mut output);
    // A parent cycle leaves rows unreachable from any root; surface them flat
    // rather than dropping them from the sidebar.
    let mut remaining = categories
        .iter()
        .filter(|item| !visited.contains(&item.id))
        .collect::<Vec<_>>();
    remaining.sort_by(|left, right| {
        left.sort_order
            .cmp(&right.sort_order)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
            .then_with(|| left.id.cmp(&right.id))
    });
    for category in remaining {
        if visited.insert(category.id.clone()) {
            output.push((category, 0));
            visit(Some(&category.id), 1, categories, &mut visited, &mut output);
        }
    }
    output
}

pub(in crate::features::panels) fn filtered_quick_commands(
    commands: &[QuickCommand],
    categories: &[QuickCommandCategory],
    query: &str,
    selected_category: &str,
    sort_mode: QuickCommandSortMode,
) -> Vec<QuickCommand> {
    let query = query.trim().to_lowercase();
    let mut filtered = commands
        .iter()
        .filter(|command| match selected_category {
            "all" => true,
            "uncategorized" => command
                .category_id
                .as_deref()
                .unwrap_or_default()
                .trim()
                .is_empty(),
            category_id => command.category_id.as_deref() == Some(category_id),
        })
        .filter(|command| {
            if query.is_empty() {
                return true;
            }
            let category = quick_command_category_label(categories, command);
            command.label.to_lowercase().contains(&query)
                || command.command.to_lowercase().contains(&query)
                || command
                    .description
                    .as_deref()
                    .unwrap_or_default()
                    .to_lowercase()
                    .contains(&query)
                || category.to_lowercase().contains(&query)
        })
        .cloned()
        .collect::<Vec<_>>();

    filtered.sort_by(|left, right| {
        right
            .pinned
            .unwrap_or_default()
            .cmp(&left.pinned.unwrap_or_default())
            .then_with(|| match sort_mode {
                QuickCommandSortMode::Usage => right
                    .use_count
                    .unwrap_or_default()
                    .cmp(&left.use_count.unwrap_or_default())
                    .then_with(|| {
                        right
                            .updated_at
                            .unwrap_or_default()
                            .cmp(&left.updated_at.unwrap_or_default())
                    }),
                QuickCommandSortMode::Name => {
                    left.label.to_lowercase().cmp(&right.label.to_lowercase())
                }
                QuickCommandSortMode::Created => left
                    .created_at
                    .or(left.updated_at)
                    .unwrap_or(u64::MAX)
                    .cmp(&right.created_at.or(right.updated_at).unwrap_or(u64::MAX)),
                QuickCommandSortMode::Custom => left
                    .sort_order
                    .unwrap_or(i32::MAX)
                    .cmp(&right.sort_order.unwrap_or(i32::MAX))
                    .then_with(|| {
                        left.created_at
                            .or(left.updated_at)
                            .unwrap_or(u64::MAX)
                            .cmp(&right.created_at.or(right.updated_at).unwrap_or(u64::MAX))
                    }),
            })
            .then_with(|| left.label.to_lowercase().cmp(&right.label.to_lowercase()))
    });
    filtered
}

/// The command's icon, or a colored dot when it has none.
///
/// `icon_size_px` tracks the view mode, as Tauri's `renderCommandIcon` className
/// does (12px tile, 12.8px compact, 14.4px list). The dot stays 10px everywhere,
/// matching Tauri's fixed `h-2.5 w-2.5`.
pub(in crate::features::panels) fn quick_command_icon_mark(
    palette: crate::theme::ThemePalette,
    icon_tag: Option<&str>,
    color_tag: Option<&str>,
    icon_size_px: f32,
) -> impl IntoElement {
    match icon_tag.and_then(quick_command_icon) {
        Some(def) => crate::features::view_widgets::mono_icon(
            def.path,
            rgb(def.tint(palette).unwrap_or(palette.text)).into(),
            icon_size_px,
        )
        .into_any_element(),
        // No icon chosen: the command still gets its color, as a plain dot.
        None => div()
            .size(px(10.))
            .flex_none()
            .rounded_full()
            .bg(quick_command_color(palette, color_tag))
            .into_any_element(),
    }
}

/// The pin marker, sized with its row.
///
/// Tauri's `MdPushPin` inherits the row's text color at `opacity-60`, so a pinned
/// command reads as a quiet marker rather than a warning.
pub(in crate::features::panels) fn quick_command_pin_mark(
    palette: crate::theme::ThemePalette,
    size_px: f32,
) -> impl IntoElement {
    svg()
        .size(px(size_px))
        .flex_none()
        .opacity(0.6)
        .text_color(rgb(palette.text_muted))
        .path("icons/pin.svg")
}

/// One-line preview of a possibly multi-line command.
///
/// CSS `white-space: nowrap` collapses newlines, so Tauri's `truncate` rows show a
/// multi-line script as one line. GPUI cannot: `shape_text` always splits on a
/// newline regardless of `white_space`, so the text has to be flattened before
/// it reaches a `.truncate()` row or it renders as several clipped lines.
pub(in crate::features::panels) fn quick_command_single_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(in crate::features::panels) fn quick_command_color(
    palette: crate::theme::ThemePalette,
    color_tag: Option<&str>,
) -> gpui::Rgba {
    match color_tag.unwrap_or_default() {
        "red" => rgb(palette.danger),
        "green" => rgb(palette.success),
        "blue" => rgb(palette.link),
        "yellow" => rgb(palette.warning),
        // Not a palette token: `link` is already blue, so routing purple there
        // makes the two tags indistinguishable. Tauri's `bg-purple-500`.
        "purple" => rgb(0xa855f7),
        _ => rgb(palette.text_muted),
    }
}

/// One field of the quick command editor.
pub(in crate::features::panels) struct QuickCommandEditorFieldSpec {
    pub field: QuickCommandEditorField,
    pub label: Cow<'static, str>,
    pub placeholder: Cow<'static, str>,
    pub value: String,
    /// Shown beside the caption, as Tauri shows `errors.label` / `errors.command`.
    pub error: Option<String>,
}

/// A captioned box hosting one of the quick command editor's inputs.
pub(in crate::features::panels) fn quick_command_editor_field(
    app: &mut NyaTermApp,
    spec: QuickCommandEditorFieldSpec,
    cx: &mut Context<NyaTermApp>,
) -> gpui::AnyElement {
    quick_command_editor_input(app, spec, false, None, cx)
}

/// The script box, which is the one that takes newlines.
pub(in crate::features::panels) fn quick_command_editor_script_field(
    app: &mut NyaTermApp,
    spec: QuickCommandEditorFieldSpec,
    raw_send: bool,
    cx: &mut Context<NyaTermApp>,
) -> gpui::AnyElement {
    let toggle = div()
        .debug_selector(|| "quick-command-editor-raw-send-control".into())
        .flex_none()
        .flex()
        .items_center()
        .gap_2()
        .text_xs()
        .text_color(rgb(app.theme_palette().text_muted))
        .child(t!("quickCommands.rawSend"))
        .child(
            NyaSwitch::new("quick-command-editor-raw-send")
                .checked(raw_send)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.toggle_quick_command_editor_raw_send(cx);
                })),
        )
        .into_any_element();
    quick_command_editor_input(app, spec, true, Some(toggle), cx)
}

fn quick_command_editor_input(
    app: &mut NyaTermApp,
    spec: QuickCommandEditorFieldSpec,
    multi_line: bool,
    caption_action: Option<gpui::AnyElement>,
    cx: &mut Context<NyaTermApp>,
) -> gpui::AnyElement {
    let QuickCommandEditorFieldSpec {
        field,
        label,
        placeholder,
        value,
        error,
    } = spec;
    let palette = app.theme_palette();
    let input_id = format!(
        "quick-command.editor.{}",
        quick_command_editor_field_key(field)
    );
    let input = if multi_line {
        app.text_input_box_with_language(
            input_id,
            &value,
            TextInputSetup::code(placeholder),
            "bash".into(),
            cx,
        )
        .into_any_element()
    } else {
        app.text_input_box(
            input_id,
            &value,
            TextInputSetup::placeholder(placeholder),
            cx,
        )
        .into_any_element()
    };
    div()
        .min_w_0()
        .when(multi_line, |this| this.flex_1())
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_2()
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(palette.text_muted))
                        .child(label),
                )
                .child(
                    div()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap_2()
                        .when_some(error, |this, error| {
                            this.child(
                                div()
                                    .min_w_0()
                                    .text_size(px(11.))
                                    .text_color(rgb(palette.danger))
                                    .truncate()
                                    .child(error),
                            )
                        })
                        .children(caption_action),
                ),
        )
        .child(input)
        .into_any_element()
}

/// The stable part of a quick command field's input id.
pub(in crate::features::panels) fn quick_command_editor_field_key(
    field: QuickCommandEditorField,
) -> &'static str {
    match field {
        QuickCommandEditorField::Label => "label",
        QuickCommandEditorField::Command => "command",
        QuickCommandEditorField::Category => "category",
        QuickCommandEditorField::Description => "description",
    }
}

pub(in crate::features::panels) fn terminal_action_prompt_text(
    text: &str,
    max_chars: usize,
) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    let tail = trimmed
        .chars()
        .rev()
        .take(max_chars)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    format!("[truncated to last {max_chars} chars]\n{tail}")
}

#[cfg(test)]
mod tests {
    use super::quick_command_single_line;

    #[test]
    fn single_line_collapses_a_multi_line_script_the_way_css_nowrap_does() {
        assert_eq!(
            quick_command_single_line("# depth arg\nsh -c 'find \"$dir\"'"),
            "# depth arg sh -c 'find \"$dir\"'"
        );
    }

    #[test]
    fn single_line_collapses_tabs_and_runs_of_spaces() {
        assert_eq!(
            quick_command_single_line("iostat\t-dxm   1\r\n"),
            "iostat -dxm 1"
        );
    }

    #[test]
    fn single_line_leaves_an_already_flat_command_alone() {
        assert_eq!(quick_command_single_line("stty cols"), "stty cols");
    }

    #[test]
    fn single_line_of_blank_input_is_empty() {
        assert_eq!(quick_command_single_line("  \n\t "), "");
    }
}
