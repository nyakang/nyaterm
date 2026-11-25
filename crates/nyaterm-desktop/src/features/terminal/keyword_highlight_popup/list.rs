//! Rule list of the keyword highlight editor.
//!
//! The popup shows the session rules and the global catalogue in a single
//! gpui-kit list: two sections, the component's own search box, native
//! selection and drag-and-drop reordering. Dragging a rule onto the other
//! group moves it there.

use std::collections::HashSet;

use gpui::{
    App, AppContext as _, ClickEvent, Context, Div, DragMoveEvent, IntoElement, ParentElement as _,
    Render, RenderOnce, SharedString, Stateful, Styled as _, Task, WeakEntity, Window, div,
    prelude::*, px, rgb, rgba,
};
use nyaterm_core::KeywordHighlightRule;
use nyaterm_ui::{
    IconName, NyaIndexPath as IndexPath, NyaListDelegate as ListDelegate,
    NyaListState as ListState, NyaSelectable as Selectable, NyaSwitch,
};
use rust_i18n::t;

use super::{KeywordHighlightWindow, RuleScope, popup_icon_button};
use crate::theme::ThemePalette;

/// Height of one rule row. The list measures a single row and reuses that
/// height for every entry, so all rows have to be exactly this tall.
const ROW_HEIGHT: f32 = 32.;

/// Section index of the session rules.
pub(super) const SESSION_SECTION: usize = 0;
/// Section index of the global rules.
pub(super) const GLOBAL_SECTION: usize = 1;

/// Where a dragged rule lands relative to the row under the pointer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum DropPosition {
    Before,
    After,
}

impl DropPosition {
    /// The insertion gap this position resolves to for `row`.
    fn gap(self, row: usize) -> usize {
        match self {
            Self::Before => row,
            Self::After => row + 1,
        }
    }
}

/// Payload carried by a rule drag.
#[derive(Clone)]
pub(super) struct KeywordDrag {
    pub(super) scope: RuleScope,
    pub(super) id: String,
    pub(super) name: String,
}

/// Preview shown while a rule is being dragged.
struct KeywordDragPreview {
    label: String,
}

impl Render for KeywordDragPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .rounded_md()
            .border_1()
            .border_color(rgb(0x378add))
            .bg(rgb(0x1f2937))
            .text_xs()
            .text_color(rgb(0xf1f5f9))
            .child(self.label.clone())
    }
}

/// One rule row of the list.
///
/// The editor owns the selection state, so the list's own `selected` flag is
/// ignored: the row renders whichever rule the editor currently highlights.
#[derive(IntoElement)]
pub(super) struct KeywordRuleItem {
    base: Stateful<Div>,
    palette: ThemePalette,
    label: String,
    enabled: bool,
    dirty: bool,
    placeholder: bool,
    selected: bool,
    drop_position: Option<DropPosition>,
    controls_enabled: bool,
    switch_id: SharedString,
    delete_id: SharedString,
    on_toggle: Option<Box<dyn Fn(&bool, &mut Window, &mut App) + 'static>>,
    on_delete: Option<Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>>,
}

impl KeywordRuleItem {
    /// A rule row with its drag handle, state dot, name and inline controls.
    #[allow(clippy::too_many_arguments)]
    fn rule(
        base: Stateful<Div>,
        palette: ThemePalette,
        rule: &KeywordHighlightRule,
        dirty: bool,
        selected: bool,
        drop_position: Option<DropPosition>,
        controls_enabled: bool,
        on_toggle: Box<dyn Fn(&bool, &mut Window, &mut App) + 'static>,
        on_delete: Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>,
    ) -> Self {
        let id = rule.id.clone();
        let label = if rule.name.trim().is_empty() {
            t!("settings.keywordHighlightNewRule").to_string()
        } else {
            rule.name.clone()
        };
        Self {
            base,
            palette,
            label,
            enabled: rule.enabled,
            dirty,
            placeholder: false,
            selected,
            drop_position,
            controls_enabled,
            switch_id: SharedString::from(format!("keyword-switch-{id}")),
            delete_id: SharedString::from(format!("keyword-delete-{id}")),
            on_toggle: Some(on_toggle),
            on_delete: Some(on_delete),
        }
    }

    /// Placeholder shown when a group has no rule to display. It keeps the
    /// group header (and its add button) visible and doubles as the drop
    /// target that receives a rule dragged into the empty group.
    fn placeholder(
        base: Stateful<Div>,
        palette: ThemePalette,
        section: usize,
        searching: bool,
        drop_position: Option<DropPosition>,
    ) -> Self {
        let label = if searching {
            t!("terminalCtx.ruleSearchEmpty").to_string()
        } else {
            t!("terminalCtx.ruleGroupEmpty").to_string()
        };
        Self {
            base,
            palette,
            label,
            enabled: false,
            dirty: false,
            placeholder: true,
            selected: false,
            drop_position,
            controls_enabled: false,
            switch_id: SharedString::from(format!("keyword-empty-switch-{section}")),
            delete_id: SharedString::from(format!("keyword-empty-delete-{section}")),
            on_toggle: None,
            on_delete: None,
        }
    }
}

impl Selectable for KeywordRuleItem {
    /// The editor owns the selection, so the list's flag is not adopted.
    fn selected(self, _selected: bool) -> Self {
        self
    }

    fn is_selected(&self) -> bool {
        self.selected
    }
}

impl RenderOnce for KeywordRuleItem {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let palette = self.palette;
        let selected = self.selected;
        let drop_position = self.drop_position;
        let dirty = self.dirty;

        let mut row = self
            .base
            .h(px(ROW_HEIGHT))
            .flex()
            .items_center()
            .gap_2()
            .rounded_md()
            .border_1()
            .border_color(if selected {
                rgba((palette.primary << 8) | 0x33)
            } else {
                rgba(0)
            })
            .bg(if selected {
                rgba((palette.primary << 8) | 0x1f)
            } else {
                rgba(0)
            })
            .hover(move |this| {
                this.bg(if selected {
                    rgba((palette.primary << 8) | 0x29)
                } else {
                    rgb(palette.hover)
                })
            });

        if let Some(position) = drop_position {
            let line = div()
                .absolute()
                .left_0()
                .right_0()
                .h(px(2.))
                .rounded_full()
                .bg(rgb(palette.link));
            row = row.child(match position {
                DropPosition::Before => line.top(px(0.)),
                DropPosition::After => line.bottom(px(0.)),
            });
        }

        if self.placeholder {
            return row
                .px_2()
                .text_xs()
                .text_color(rgb(palette.text_dimmed))
                .child(self.label)
                .into_any_element();
        }

        let enabled = self.enabled;
        let label = self.label;
        let switch_id = self.switch_id;
        let delete_id = self.delete_id;
        let controls_enabled = self.controls_enabled;
        let on_toggle = self.on_toggle;
        let on_delete = self.on_delete;

        row.pl_2()
            .child(drag_handle(palette))
            .child(
                div()
                    .size(px(7.))
                    .rounded_full()
                    .flex_none()
                    .bg(rgb(if enabled {
                        palette.success
                    } else {
                        palette.text_dimmed
                    })),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .overflow_hidden()
                    .flex()
                    .items_center()
                    .gap_1()
                    .text_xs()
                    .text_color(if selected {
                        rgb(palette.text)
                    } else {
                        rgb(palette.text_muted)
                    })
                    // A draft is marked next to the rule it belongs to.
                    .when(dirty, |this| {
                        this.child(
                            div()
                                .text_color(rgb(palette.warning))
                                .font_weight(gpui::FontWeight(600.))
                                .child("*"),
                        )
                    })
                    .child(label),
            )
            .child(
                div().flex_none().child(
                    NyaSwitch::new(switch_id)
                        .checked(enabled)
                        .disabled(!controls_enabled)
                        .on_click(move |checked, window, cx| {
                            if let Some(on_toggle) = on_toggle.as_ref() {
                                on_toggle(checked, window, cx);
                            }
                        }),
                ),
            )
            .child(div().flex_none().pr_2().child(popup_icon_button(
                palette,
                delete_id,
                IconName::Trash,
                t!("common.delete"),
                controls_enabled,
                true,
                move |event, window, cx| {
                    if let Some(on_delete) = on_delete.as_ref() {
                        on_delete(event, window, cx);
                    }
                },
            )))
            .into_any_element()
    }
}

/// The little grip that marks a row as draggable.
fn drag_handle(palette: ThemePalette) -> impl IntoElement {
    div()
        .w(px(8.))
        .h(px(12.))
        .flex_none()
        .flex()
        .justify_between()
        .child(
            div()
                .w(px(2.))
                .h(px(12.))
                .rounded_full()
                .bg(rgb(palette.text_dimmed)),
        )
        .child(
            div()
                .w(px(2.))
                .h(px(12.))
                .rounded_full()
                .bg(rgb(palette.text_dimmed)),
        )
}

/// The list delegate backing the rule list.
///
/// It owns a snapshot of both rule groups, the filtered row indexes and the
/// pending drop target. Every mutation is handed back to the editor, which
/// owns the rules themselves.
pub(super) struct KeywordListDelegate {
    window: WeakEntity<KeywordHighlightWindow>,
    session: Vec<KeywordHighlightRule>,
    global: Vec<KeywordHighlightRule>,
    session_rows: Vec<usize>,
    global_rows: Vec<usize>,
    dirty: HashSet<String>,
    selected: Option<String>,
    drop_target: Option<(IndexPath, DropPosition)>,
    query: String,
    palette: ThemePalette,
    controls_enabled: bool,
}

impl KeywordListDelegate {
    pub(super) fn new(window: WeakEntity<KeywordHighlightWindow>, palette: ThemePalette) -> Self {
        Self {
            window,
            session: Vec::new(),
            global: Vec::new(),
            session_rows: Vec::new(),
            global_rows: Vec::new(),
            dirty: HashSet::new(),
            selected: None,
            drop_target: None,
            query: String::new(),
            palette,
            controls_enabled: false,
        }
    }

    /// Replace the snapshot with the editor's current state.
    pub(super) fn set_snapshot(
        &mut self,
        session: Vec<KeywordHighlightRule>,
        global: Vec<KeywordHighlightRule>,
        dirty: HashSet<String>,
        selected: Option<String>,
        palette: ThemePalette,
        controls_enabled: bool,
    ) {
        self.session = session;
        self.global = global;
        self.dirty = dirty;
        self.selected = selected;
        self.palette = palette;
        self.controls_enabled = controls_enabled;
        self.refilter();
    }

    fn refilter(&mut self) {
        let query = self.query.trim().to_lowercase();
        self.session_rows = matching_rows(&self.session, &query);
        self.global_rows = matching_rows(&self.global, &query);
    }

    fn rules(&self, section: usize) -> &Vec<KeywordHighlightRule> {
        if section == SESSION_SECTION {
            &self.session
        } else {
            &self.global
        }
    }

    fn rows(&self, section: usize) -> &Vec<usize> {
        if section == SESSION_SECTION {
            &self.session_rows
        } else {
            &self.global_rows
        }
    }

    fn rule_at(&self, ix: IndexPath) -> Option<&KeywordHighlightRule> {
        let rows = self.rows(ix.section);
        rows.get(ix.row)
            .and_then(|&index| self.rules(ix.section).get(index))
    }

    /// The list position of a rule, if the current filter still shows it.
    pub(super) fn index_of(&self, id: &str) -> Option<IndexPath> {
        for section in [SESSION_SECTION, GLOBAL_SECTION] {
            let rules = self.rules(section);
            for (row, &index) in self.rows(section).iter().enumerate() {
                if rules[index].id == id {
                    return Some(IndexPath::new(row).section(section));
                }
            }
        }
        None
    }

    /// Record the pending drop target, returns true when it changed.
    pub(super) fn update_drop_target(
        &mut self,
        ix: IndexPath,
        position: Option<DropPosition>,
    ) -> bool {
        match position {
            Some(position) => {
                if self.drop_target != Some((ix, position)) {
                    self.drop_target = Some((ix, position));
                    true
                } else {
                    false
                }
            }
            None => {
                if self.drop_target.is_some_and(|(target, _)| target == ix) {
                    self.drop_target = None;
                    true
                } else {
                    false
                }
            }
        }
    }

    /// The drop marker a row has to paint, if any.
    pub(super) fn drop_position_for(&self, ix: IndexPath) -> Option<DropPosition> {
        let (target, position) = self.drop_target?;
        if target.section != ix.section {
            return None;
        }
        let count = self.rows(ix.section).len();
        if ix.row >= count {
            // The placeholder of an empty group receives the rule at the end
            // of that group.
            return Some(DropPosition::After);
        }
        let gap = position.gap(target.row);
        if gap == ix.row {
            Some(DropPosition::Before)
        } else if gap == count && ix.row + 1 == count {
            Some(DropPosition::After)
        } else {
            None
        }
    }

    /// The rule a drop on `ix` would land next to, if the pending drop target
    /// matches that row.
    ///
    /// The anchor is `None` when the target group has no visible rule: the
    /// dropped rule is then appended to that group.
    pub(super) fn pending_drop(&self, ix: IndexPath) -> Option<(Option<String>, DropPosition)> {
        let (target, position) = self.drop_target?;
        if target != ix {
            return None;
        }
        match self.rule_at(target) {
            Some(rule) => Some((Some(rule.id.clone()), position)),
            // The group holds no rule, so there is nothing to anchor to.
            None => Some((None, DropPosition::After)),
        }
    }
}

/// Indexes of the rules that match the active search query.
fn matching_rows(rules: &[KeywordHighlightRule], query: &str) -> Vec<usize> {
    rules
        .iter()
        .enumerate()
        .filter(|(_, rule)| {
            query.is_empty()
                || rule.name.to_lowercase().contains(query)
                || rule
                    .patterns
                    .iter()
                    .any(|pattern| pattern.to_lowercase().contains(query))
        })
        .map(|(index, _)| index)
        .collect()
}

impl ListDelegate for KeywordListDelegate {
    type Item = KeywordRuleItem;

    fn sections_count(&self, _: &App) -> usize {
        2
    }

    fn items_count(&self, section: usize, _: &App) -> usize {
        // An empty group still renders a placeholder row so that its header
        // (and the add button in it) stays reachable.
        self.rows(section).len().max(1)
    }

    fn perform_search(
        &mut self,
        query: &str,
        _: &mut Window,
        _: &mut Context<ListState<Self>>,
    ) -> Task<()> {
        self.query = query.to_string();
        self.refilter();
        Task::ready(())
    }

    fn render_item(
        &mut self,
        ix: IndexPath,
        _: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Option<Self::Item> {
        let palette = self.palette;
        let controls_enabled = self.controls_enabled;
        let Some(rule) = self.rule_at(ix).cloned() else {
            // An empty group still accepts a rule that is dragged into it.
            let base = self.empty_group_row(ix, cx);
            let drop_position = self.drop_position_for(ix);
            return Some(KeywordRuleItem::placeholder(
                base,
                palette,
                ix.section,
                !self.query.trim().is_empty(),
                drop_position,
            ));
        };
        let id = rule.id.clone();
        let dirty = self.dirty.contains(&id);
        let selected = self.selected.as_deref() == Some(id.as_str());
        let drop_position = self.drop_position_for(ix);
        let scope = if ix.section == SESSION_SECTION {
            RuleScope::Session
        } else {
            RuleScope::Global
        };

        let toggle_window = self.window.clone();
        let toggle_id = id.clone();
        let on_toggle = Box::new(move |checked: &bool, _window: &mut Window, cx: &mut App| {
            cx.stop_propagation();
            toggle_window
                .update(cx, |editor, cx| {
                    editor.set_rule_enabled(scope, &toggle_id, *checked, cx);
                })
                .ok();
        });

        let delete_window = self.window.clone();
        let delete_id = id.clone();
        let on_delete = Box::new(
            move |_event: &ClickEvent, _window: &mut Window, cx: &mut App| {
                cx.stop_propagation();
                delete_window
                    .update(cx, |editor, cx| {
                        editor.remove_rule(scope, &delete_id, cx);
                    })
                    .ok();
            },
        );

        let base = self.draggable_row(ix, scope, &id, &rule.name, cx);
        Some(KeywordRuleItem::rule(
            base,
            palette,
            &rule,
            dirty,
            selected,
            drop_position,
            controls_enabled,
            on_toggle,
            on_delete,
        ))
    }

    fn render_section_header(
        &mut self,
        section: usize,
        _: &mut Window,
        _: &mut Context<ListState<Self>>,
    ) -> Option<impl IntoElement> {
        let palette = self.palette;
        let controls_enabled = self.controls_enabled;
        let count = self.rows(section).len();
        let title = if section == SESSION_SECTION {
            t!("terminalCtx.sessionRules").to_string()
        } else {
            t!("terminalCtx.globalRules").to_string()
        };
        let label = format!(
            "{} {}",
            title,
            t!("terminalCtx.ruleGroupCount", count = count)
        );
        let scope = if section == SESSION_SECTION {
            RuleScope::Session
        } else {
            RuleScope::Global
        };

        let add_window = self.window.clone();
        let add = popup_icon_button(
            palette,
            format!("keyword-group-add-{section}"),
            IconName::Plus,
            t!("common.add"),
            controls_enabled,
            false,
            move |_, _, cx| {
                add_window
                    .update(cx, |editor, cx| {
                        editor.add_rule(scope, cx);
                    })
                    .ok();
            },
        );

        let clear_window = self.window.clone();
        let has_session_rules = !self.session.is_empty();
        let clear = (section == SESSION_SECTION).then(|| {
            popup_icon_button(
                palette,
                "keyword-group-clear",
                IconName::Trash,
                t!("terminalCtx.ruleClearAll"),
                controls_enabled && has_session_rules,
                true,
                move |_, _, cx| {
                    clear_window
                        .update(cx, |editor, cx| {
                            editor.clear_session_rules(cx);
                        })
                        .ok();
                },
            )
        });

        Some(
            div()
                .h(px(26.))
                .flex()
                .items_center()
                .justify_between()
                .px_2()
                .child(
                    div()
                        .text_xs()
                        .font_weight(gpui::FontWeight(500.))
                        .text_color(rgb(palette.text_muted))
                        .child(label),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(add)
                        .when_some(clear, |this, clear| this.child(clear)),
                ),
        )
    }

    fn render_empty(
        &mut self,
        _: &mut Window,
        _: &mut Context<ListState<Self>>,
    ) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .text_xs()
            .text_color(rgb(self.palette.text_dimmed))
            .child(t!("terminalCtx.ruleSearchEmpty"))
    }

    fn set_selected_index(
        &mut self,
        ix: Option<IndexPath>,
        _: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) {
        let id = ix
            .and_then(|ix| self.rule_at(ix))
            .map(|rule| rule.id.clone());
        if id == self.selected {
            return;
        }
        self.selected = id.clone();
        let Some(id) = id else {
            return;
        };
        self.window
            .update(cx, |editor, cx| {
                editor.select_rule(id, cx);
            })
            .ok();
    }

    fn confirm(&mut self, _secondary: bool, _: &mut Window, cx: &mut Context<ListState<Self>>) {
        // Enter on a row opens the detail panel.
        self.window
            .update(cx, |editor, cx| {
                editor.open_detail(cx);
            })
            .ok();
    }
}

impl KeywordListDelegate {
    /// Build the draggable row element for a rule.
    fn draggable_row(
        &self,
        ix: IndexPath,
        scope: RuleScope,
        id: &str,
        name: &str,
        cx: &mut Context<ListState<KeywordListDelegate>>,
    ) -> Stateful<Div> {
        let window = self.window.clone();
        let list = cx.entity().downgrade();
        let drag = KeywordDrag {
            scope,
            id: id.to_string(),
            name: name.to_string(),
        };
        div()
            .id(SharedString::from(format!(
                "keyword-row-{}-{}",
                ix.section, ix.row
            )))
            .on_drag(drag, move |payload, _, _, cx| {
                // A drag can end without a drop, so the marker of a previous
                // drag has to go when a new one starts.
                list.update(cx, |list, cx| {
                    list.delegate_mut().drop_target = None;
                    cx.notify();
                })
                .ok();
                cx.new(|_| KeywordDragPreview {
                    label: payload.name.clone(),
                })
            })
            .on_drag_move(
                cx.listener(move |state, event: &DragMoveEvent<KeywordDrag>, _, cx| {
                    let bounds = event.bounds;
                    let (from_scope, from_id) = {
                        let dragging = event.drag(cx);
                        (dragging.scope, dragging.id.clone())
                    };
                    let position = if bounds.contains(&event.event.position) {
                        let position = if event.event.position.y < bounds.center().y {
                            DropPosition::Before
                        } else {
                            DropPosition::After
                        };
                        // The gaps right before and right after the dragged row
                        // would not move it at all.
                        let gap = position.gap(ix.row);
                        let noop = from_scope == scope
                            && state
                                .delegate()
                                .index_of(&from_id)
                                .map(|from| {
                                    from.section == ix.section
                                        && (gap == from.row || gap == from.row + 1)
                                })
                                .unwrap_or(false);
                        if noop { None } else { Some(position) }
                    } else {
                        None
                    };
                    if state.delegate_mut().update_drop_target(ix, position) {
                        cx.notify();
                    }
                }),
            )
            .on_drop(cx.listener(move |state, drag: &KeywordDrag, _, cx| {
                let target = state.delegate().pending_drop(ix);
                let Some((target_id, position)) = target else {
                    return;
                };
                let target_scope = if ix.section == SESSION_SECTION {
                    RuleScope::Session
                } else {
                    RuleScope::Global
                };
                state.delegate_mut().drop_target = None;
                cx.notify();
                window
                    .update(cx, |editor, cx| {
                        editor.move_rule(
                            drag.scope,
                            drag.id.clone(),
                            target_scope,
                            target_id,
                            position,
                            cx,
                        );
                    })
                    .ok();
            }))
    }

    /// Build the placeholder row of a group without any visible rule.
    ///
    /// The placeholder is a drop target, so a rule can be dragged into an
    /// empty group. There is no anchor rule in that case, the dropped rule
    /// simply becomes the first entry of the group.
    fn empty_group_row(
        &self,
        ix: IndexPath,
        cx: &mut Context<ListState<KeywordListDelegate>>,
    ) -> Stateful<Div> {
        let window = self.window.clone();
        let scope = if ix.section == SESSION_SECTION {
            RuleScope::Session
        } else {
            RuleScope::Global
        };
        div()
            .id(SharedString::from(format!(
                "keyword-empty-{}-{}",
                ix.section, ix.row
            )))
            .on_drag_move(
                cx.listener(move |state, event: &DragMoveEvent<KeywordDrag>, _, cx| {
                    let bounds = event.bounds;
                    // The group holds no rule, so every position inside the
                    // placeholder means the same thing: the rule joins the group.
                    let position = if bounds.contains(&event.event.position) {
                        Some(DropPosition::After)
                    } else {
                        None
                    };
                    if state.delegate_mut().update_drop_target(ix, position) {
                        cx.notify();
                    }
                }),
            )
            .on_drop(cx.listener(move |state, drag: &KeywordDrag, _, cx| {
                let target = state.delegate().pending_drop(ix);
                let Some((target_id, position)) = target else {
                    return;
                };
                state.delegate_mut().drop_target = None;
                cx.notify();
                window
                    .update(cx, |editor, cx| {
                        editor.move_rule(
                            drag.scope,
                            drag.id.clone(),
                            scope,
                            target_id,
                            position,
                            cx,
                        );
                    })
                    .ok();
            }))
    }
}
