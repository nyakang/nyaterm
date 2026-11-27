//! Session-scoped editor. Drafts and input widgets belong to the child entity;
//! the application only owns its lifetime and applies validated changes.
mod draft;
mod list;
mod session_draft;

use self::session_draft::{MatchInput, SessionValidation};

use self::draft::RuleDraftIndex;
use self::list::{DropPosition, KeywordListDelegate};
use crate::features::NyaTermApp;
use crate::features::view_widgets::{
    ChildWindowChrome, ChildWindowCloseHandler, ChildWindowSpec, child_window_header,
    child_window_options, child_window_root, focus_child_window_shell_if_idle,
    keyword_match_option_toggles,
};
use crate::models::KeywordHighlightEditorField;
use crate::models::KeywordMatchOption;
use crate::theme::ThemePalette;
use gpui::{
    AnyElement, App, AppContext as _, ClickEvent, Context, Entity, FocusHandle, Hsla, IntoElement,
    KeyDownEvent, Render, ScrollHandle, SharedString, Subscription, Task, Window, div, prelude::*,
    px, rgb, rgba,
};
use nyaterm_core::KeywordHighlightRule;
use nyaterm_ui::{
    ChildWindowSlot, ColorSelect, IconName, NyaButton, NyaColorPickerEvent, NyaColorPickerState,
    NyaIcon, NyaInputEvent, NyaInputShell, NyaInputState, NyaList, NyaListState as ListState,
    NyaScrollable, NyaTooltip, NyaWindowHandle, activate_child_window, nya_root,
};
use rust_i18n::t;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Duration;

#[derive(Default)]
pub(super) struct KeywordHighlightPopupState {
    window: ChildWindowSlot,
    editor: Option<Entity<KeywordHighlightWindow>>,
}

/// The group a rule belongs to. The editor shows both groups at once, so the
/// scope only describes where a rule lives, not which tab is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RuleScope {
    Session,
    Global,
}

/// Colour-select controls owned by the popup window. Keyed by
/// `{rule_id}:{field}` so a control survives refreshes for the same rule.
#[derive(Default)]
struct PopupColorPickerControls {
    fields: HashMap<String, (Entity<NyaColorPickerState>, Subscription, String)>,
}

impl PopupColorPickerControls {
    const FIELDS: [KeywordHighlightEditorField; 4] = [
        KeywordHighlightEditorField::ColorDark,
        KeywordHighlightEditorField::BgDark,
        KeywordHighlightEditorField::ColorLight,
        KeywordHighlightEditorField::BgLight,
    ];

    fn control_key(rule_id: &str, field: KeywordHighlightEditorField) -> String {
        format!("{}:{}", rule_id, field.input_key())
    }

    fn color_of(rule: &KeywordHighlightRule, field: KeywordHighlightEditorField) -> &str {
        match field {
            KeywordHighlightEditorField::ColorDark => &rule.color_dark,
            KeywordHighlightEditorField::BgDark => &rule.bg_color_dark,
            KeywordHighlightEditorField::ColorLight => &rule.color_light,
            KeywordHighlightEditorField::BgLight => &rule.bg_color_light,
            _ => "",
        }
    }

    fn featured_colors(is_dark: bool) -> Vec<Hsla> {
        nyaterm_core::keyword_highlight_color_palette(is_dark)
            .iter()
            .filter_map(|value| nyaterm_core::themes::parse_color(value))
            .map(|bytes| gpui::Hsla::from(rgba(u32::from_be_bytes(bytes))))
            .collect()
    }

    /// Create/update/discard picker states for the currently selected rule.
    fn sync(
        &mut self,
        rule: Option<&KeywordHighlightRule>,
        window: &mut Window,
        cx: &mut Context<KeywordHighlightWindow>,
    ) {
        let mut live = std::collections::HashSet::new();
        if let Some(rule) = rule {
            for field in Self::FIELDS {
                let key = Self::control_key(&rule.id, field);
                live.insert(key.clone());
                let value = Self::color_of(rule, field).to_string();
                if let Some((picker, _, previous)) = self.fields.get_mut(&key) {
                    if *previous != value {
                        Self::apply_value(picker, &value, window, cx);
                        *previous = value;
                    }
                    continue;
                }
                let picker = cx.new(|cx| {
                    let mut state = NyaColorPickerState::new(window, cx);
                    if let Some(bytes) = nyaterm_core::themes::parse_color(&value) {
                        state = state.default_value(rgba(u32::from_be_bytes(bytes)));
                    }
                    state
                });
                let rule_id = rule.id.clone();
                let subscription = cx.subscribe(&picker, move |this, _, event, cx| {
                    let NyaColorPickerEvent::Change(color) = event;
                    let value = match color {
                        Some(color) => {
                            let c = gpui::Rgba::from(*color);
                            let channel = |v: f32| (v * 255.).round().clamp(0., 255.) as u8;
                            format!(
                                "#{:02x}{:02x}{:02x}",
                                channel(c.r),
                                channel(c.g),
                                channel(c.b)
                            )
                        }
                        None => String::new(),
                    };
                    this.set_color_value(&rule_id, field, value, cx);
                });
                self.fields.insert(key, (picker, subscription, value));
            }
        }
        let stale: Vec<String> = self
            .fields
            .keys()
            .filter(|key| !live.contains(*key))
            .cloned()
            .collect();
        for key in stale {
            self.fields.remove(&key);
        }
    }

    fn apply_value(
        picker: &Entity<NyaColorPickerState>,
        value: &str,
        window: &mut Window,
        cx: &mut Context<KeywordHighlightWindow>,
    ) {
        picker.update(cx, |picker, cx| {
            if let Some(bytes) = nyaterm_core::themes::parse_color(value) {
                picker.set_value(rgba(u32::from_be_bytes(bytes)), window, cx);
            } else {
                picker.clear_value(window, cx);
            }
        });
    }

    fn get(
        &self,
        rule_id: &str,
        field: KeywordHighlightEditorField,
    ) -> Option<&Entity<NyaColorPickerState>> {
        self.fields
            .get(&Self::control_key(rule_id, field))
            .map(|(picker, _, _)| picker)
    }

    fn set_cached_value(&mut self, rule_id: &str, field: KeywordHighlightEditorField, value: &str) {
        if let Some((_, _, previous)) = self.fields.get_mut(&Self::control_key(rule_id, field)) {
            *previous = value.to_string();
        }
    }
}

/// Whether the relative order of rules changed (pure additions/removals do
/// not count). Used so reordering participates in the dirty state and save.
fn order_changed(base: &[KeywordHighlightRule], draft: &[KeywordHighlightRule]) -> bool {
    let base_ids: Vec<&str> = base
        .iter()
        .map(|r| r.id.as_str())
        .filter(|id| draft.iter().any(|r| r.id == *id))
        .collect();
    let draft_ids: Vec<&str> = draft
        .iter()
        .map(|r| r.id.as_str())
        .filter(|id| base.iter().any(|r| r.id == *id))
        .collect();
    base_ids != draft_ids
}

/// Apply the draft's ordering to a merged rule set. Rules added concurrently
/// (absent from the draft) keep their relative order at the end.
fn reorder_merged(
    mut merged: Vec<KeywordHighlightRule>,
    draft: &[KeywordHighlightRule],
) -> Vec<KeywordHighlightRule> {
    let positions: HashMap<&str, usize> = draft
        .iter()
        .enumerate()
        .map(|(index, rule)| (rule.id.as_str(), index))
        .collect();
    merged.sort_by_key(|rule| {
        positions
            .get(rule.id.as_str())
            .copied()
            .unwrap_or(usize::MAX)
    });
    merged
}

/// Three-way merge changes only IDs edited by this draft. Ordering of existing
/// rules is preserved; independently added rules are never discarded.
fn merge_rules(
    base: &[KeywordHighlightRule],
    draft: &[KeywordHighlightRule],
    current: &[KeywordHighlightRule],
) -> Result<Vec<KeywordHighlightRule>, String> {
    let before: HashMap<&str, &KeywordHighlightRule> =
        base.iter().map(|r| (r.id.as_str(), r)).collect();
    let after: HashMap<&str, &KeywordHighlightRule> =
        draft.iter().map(|r| (r.id.as_str(), r)).collect();
    let now: HashMap<&str, &KeywordHighlightRule> =
        current.iter().map(|r| (r.id.as_str(), r)).collect();
    let mut changes = HashMap::new();
    for id in before.keys().chain(after.keys()) {
        let old = before.get(id).copied();
        let new = after.get(id).copied();
        if old == new {
            continue;
        }
        let latest = now.get(id).copied();
        if latest != old && latest != new {
            return Err(t!("terminalCtx.ruleConflict").to_string());
        }
        changes.insert(*id, new);
    }
    let mut merged: Vec<KeywordHighlightRule> = Vec::with_capacity(current.len() + draft.len());
    for rule in current {
        match changes.get(rule.id.as_str()) {
            // The draft replaces the rule in place, so the order of the
            // surrounding rules stays untouched.
            Some(Some(replacement)) => merged.push((*replacement).clone()),
            Some(None) => {}
            None => merged.push(rule.clone()),
        }
    }
    for rule in draft {
        // A rule that only exists in the draft is appended.
        if !now.contains_key(rule.id.as_str()) && changes.contains_key(rule.id.as_str()) {
            merged.push(rule.clone());
        }
    }
    Ok(merged)
}

impl NyaTermApp {
    /// Open the keyword highlight editor, or focus the window that is already
    /// open for this terminal.
    pub(in crate::features) fn open_keyword_highlight_popup(
        &mut self,
        session_id: String,
        prefill: String,
        cx: &mut Context<Self>,
    ) {
        if let Some(handle) = self.terminal.keyword_popup.window.handle() {
            activate_child_window(
                &cx.entity(),
                handle,
                |app: &mut NyaTermApp| Some(&mut app.terminal.keyword_popup.window),
                cx,
            );
            return;
        }
        if !self.terminal.keyword_popup.window.begin_open() {
            return;
        }
        let app = cx.entity();
        cx.defer(move |cx| open_editor(app, session_id, prefill, cx));
    }
}

fn open_editor(app: Entity<NyaTermApp>, session_id: String, prefill: String, cx: &mut App) {
    let spec = ChildWindowSpec::modal_editor(
        t!("terminalCtx.keywordHighlightTitle").to_string(),
        880.,
        600.,
    )
    .min_size(640., 460.);
    let chrome = spec.chrome();
    let options = child_window_options(&spec, app.read(cx).shell.main_window(), cx);
    let view_app = app.clone();
    let result: anyhow::Result<NyaWindowHandle> = cx.open_window(options, move |window, cx| {
        let editor = cx.new(|cx| {
            KeywordHighlightWindow::new(
                view_app.clone(),
                session_id,
                prefill,
                chrome,
                Some(window),
                cx,
            )
        });
        view_app.update(cx, |app, _| {
            app.terminal.keyword_popup.editor = Some(editor.clone())
        });
        let weak = editor.downgrade();
        window.on_window_should_close(cx, move |_, cx| {
            weak.update(cx, |editor, cx| editor.request_close(cx))
                .unwrap_or(true)
        });
        cx.new(|cx| nya_root(editor, window, cx))
    });
    app.update(cx, |app, _| match result {
        Ok(handle) => app.terminal.keyword_popup.window.finish_open(handle),
        Err(_) => {
            app.terminal.keyword_popup.window.fail_open();
            app.terminal.keyword_popup.editor = None;
        }
    });
}

#[derive(Clone, PartialEq)]
struct ListSnapshotIdentity {
    revision: u64,
    selected: Option<String>,
    saving: bool,
    session_alive: bool,
    palette: ThemePalette,
}

struct KeywordHighlightWindow {
    app: Entity<NyaTermApp>,
    session_id: String,
    /// Global baseline used for conflict-aware persistence. Session edits are
    /// separately validated before publication to the terminal.
    base_global: Vec<KeywordHighlightRule>,
    /// Draft of the global catalogue.
    global: Vec<KeywordHighlightRule>,
    /// Session draft; only validated matching inputs reach the terminal.
    session: Vec<KeywordHighlightRule>,
    session_validation: SessionValidation,
    session_tasks: HashMap<String, Task<()>>,
    pending_save: Option<bool>,
    list_revision: u64,
    list_synced: Option<ListSnapshotIdentity>,
    list: Option<Entity<ListState<KeywordListDelegate>>>,
    selected: Option<String>,
    indexes: [RuleDraftIndex; 2],
    inputs: Vec<Entity<NyaInputState>>,
    color_pickers: PopupColorPickerControls,
    detail_scroll: ScrollHandle,
    focus: FocusHandle,
    chrome: ChildWindowChrome,
    palette: ThemePalette,
    session_alive: bool,
    appearance: (String, u16),
    detail: bool,
    error: Option<String>,
    confirm_close: bool,
    saving: bool,
    validation_generation: u64,
    focus_pattern: bool,
    closed: bool,
    _subscriptions: Vec<Subscription>,
}

impl KeywordHighlightWindow {
    /// Width of one colour select in the popup. Text and background rows use
    /// the same width so their fields line up.
    const COLOR_SELECT_WIDTH: f32 = 260.;
    /// Width of the label column in front of a colour select. It fits the
    /// longest translated label ("Background color") on a single line.
    const COLOR_LABEL_WIDTH: f32 = 100.;
    /// Width of the colour column: label, gap, select, gap and the background
    /// clear button. The theme preview starts right after it.
    const COLOR_CONTROL_WIDTH: f32 = 100. + 8. + 260. + 8. + 28.;

    fn new(
        app: Entity<NyaTermApp>,
        session_id: String,
        prefill: String,
        chrome: ChildWindowChrome,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) -> Self {
        let root = app.read(cx);
        let global = root.settings.keyword_config().rules.clone();
        let session = root
            .session_keyword_rules(&session_id)
            .unwrap_or_default()
            .to_vec();
        let palette = root.theme_palette();
        let session_alive = root.terminal.view.views.contains_key(&session_id);
        let appearance = (
            root.settings.summary().ui_font_family.clone(),
            root.settings.summary().ui_font_size,
        );
        let inputs = (0..2)
            .map(|index| {
                cx.new(|cx| {
                    let input = NyaInputState::new(cx, String::new());
                    if index == 1 {
                        input.multi_line(Some(4))
                    } else {
                        input
                    }
                })
            })
            .collect::<Vec<_>>();
        let mut subscriptions = Vec::new();
        for (index, input) in inputs.iter().enumerate() {
            subscriptions.push(cx.subscribe(input, move |this, _, event, cx| {
                if let NyaInputEvent::Changed(text) = event {
                    this.edit_field(index, text.clone(), cx);
                }
            }));
        }
        // The root owns appearance/session lifetime. Project only those values;
        // unrelated root notifications never invalidate this window.
        subscriptions.push(cx.observe(&app, |this, app, cx| {
            let root = app.read(cx);
            let palette = root.theme_palette();
            let alive = root.terminal.view.views.contains_key(&this.session_id);
            let appearance = (
                root.settings.summary().ui_font_family.clone(),
                root.settings.summary().ui_font_size,
            );
            if this.palette != palette
                || this.session_alive != alive
                || this.appearance != appearance
            {
                this.palette = palette;
                this.session_alive = alive;
                if !alive {
                    this.session_tasks.clear();
                    this.pending_save = None;
                    this.saving = false;
                }
                this.appearance = appearance;
                cx.notify();
            }
        }));

        // The list needs a window for its search field, which is only
        // available once the child window exists.
        let mut list = None;
        if let Some(window) = window {
            let delegate = KeywordListDelegate::new(cx.entity().downgrade(), palette);
            list = Some(cx.new(|cx| ListState::new(delegate, window, cx).searchable(true)));
        }

        let mut this = Self {
            app,
            session_id,
            base_global: global.clone(),
            global,
            session_validation: SessionValidation::from_applied(&session),
            session,
            session_tasks: HashMap::new(),
            pending_save: None,
            list_revision: 0,
            list_synced: None,
            list,
            selected: None,
            indexes: Default::default(),
            inputs,
            color_pickers: PopupColorPickerControls::default(),
            detail_scroll: ScrollHandle::new(),
            focus: cx.focus_handle(),
            chrome,
            palette,
            session_alive,
            appearance,
            detail: false,
            error: None,
            confirm_close: false,
            saving: false,
            validation_generation: 0,
            focus_pattern: false,
            closed: false,
            _subscriptions: subscriptions,
        };
        if let Some(rule) = prefill_rule_from_text(prefill.clone()) {
            // Reuse an existing session rule with identical text content instead
            // of prefilling a second rule for the same selection: a quick color
            // may already have created one, and saving both would duplicate it.
            let key = nyaterm_core::keyword_highlight_content_key(&prefill);
            let existing = key.as_ref().and_then(|key| {
                this.session.iter().find(|candidate| {
                    nyaterm_core::keyword_highlight_rule_content_key(candidate) == *key
                })
            });
            if let Some(existing) = existing {
                this.selected = Some(existing.id.clone());
                this.detail = true;
            } else {
                this.selected = Some(rule.id.clone());
                this.session.push(rule);
                this.detail = true;
            }
        }
        if this.selected.is_none()
            && let Some(first) = this.session.first().cloned()
        {
            this.selected = Some(first.id);
            this.detail = true;
        }
        this.indexes[0].rebuild(&[], &this.session);
        this.indexes[1].rebuild(&this.base_global, &this.global);
        this.commit_session(cx);
        this.seed_inputs(cx);
        this
    }

    fn rules(&self, scope: RuleScope) -> &Vec<KeywordHighlightRule> {
        match scope {
            RuleScope::Session => &self.session,
            RuleScope::Global => &self.global,
        }
    }

    fn rules_mut(&mut self, scope: RuleScope) -> &mut Vec<KeywordHighlightRule> {
        self.list_revision = self.list_revision.wrapping_add(1);
        match scope {
            RuleScope::Session => &mut self.session,
            RuleScope::Global => &mut self.global,
        }
    }

    /// The group a rule currently lives in.
    fn scope_of(&self, id: &str) -> RuleScope {
        if self.global.iter().any(|rule| rule.id == id) {
            RuleScope::Global
        } else {
            RuleScope::Session
        }
    }

    /// The group the current selection belongs to.
    fn selected_scope(&self) -> RuleScope {
        match self.selected.as_ref() {
            Some(id) => self.scope_of(id),
            None => RuleScope::Session,
        }
    }

    fn selected_rule(&self) -> Option<&KeywordHighlightRule> {
        let id = self.selected.as_ref()?;
        self.session
            .iter()
            .chain(self.global.iter())
            .find(|rule| rule.id == *id)
    }

    /// Global edits and session inputs still awaiting acceptance need a close guard.
    /// Valid session edits have already been applied and are not rolled back.
    fn dirty(&self) -> bool {
        self.indexes[1].dirty()
            || order_changed(&self.base_global, &self.global)
            || self.session_validation.has_unapplied()
    }

    fn dirty_ids(&self) -> HashSet<String> {
        self.indexes[1]
            .changed_ids()
            .iter()
            .cloned()
            .chain(
                self.session
                    .iter()
                    .filter(|rule| !self.session_validation.valid(&rule.id))
                    .map(|rule| rule.id.clone()),
            )
            .collect()
    }

    fn seed_inputs(&mut self, cx: &mut Context<Self>) {
        let rule = self
            .selected_rule()
            .cloned()
            .unwrap_or_else(empty_popup_rule);
        let values = [rule.name, rule.patterns.join("\n")];
        let disabled = self.selected_rule().is_none() || self.saving;
        for (input, value) in self.inputs.iter().zip(values) {
            input.update(cx, |state, cx| {
                state.set_content_silent(&value, cx);
                state.set_disabled(disabled, cx);
            });
        }
    }

    /// Push the editor's state into the list so it renders the current rules.
    fn sync_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(list) = self.list.clone() else {
            return;
        };
        let identity = ListSnapshotIdentity {
            revision: self.list_revision,
            selected: self.selected.clone(),
            saving: self.saving,
            session_alive: self.session_alive,
            palette: self.palette,
        };
        if self.list_synced.as_ref() == Some(&identity) {
            return;
        }
        self.list_synced = Some(identity);
        let session = self.session.clone();
        let global = self.global.clone();
        let dirty = self.dirty_ids();
        let selected = self.selected.clone();
        let palette = self.palette;
        let controls_enabled = !self.saving && self.session_alive;
        list.update(cx, |list, cx| {
            let index = {
                let delegate = list.delegate_mut();
                delegate.set_snapshot(
                    session,
                    global,
                    dirty,
                    selected.clone(),
                    palette,
                    controls_enabled,
                );
                selected.as_ref().and_then(|id| delegate.index_of(id))
            };
            match index {
                Some(index) => list.set_selected_index(Some(index), window, cx),
                None => {
                    if selected.is_none() {
                        list.set_selected_index(None, window, cx);
                    }
                }
            }
        });
    }

    fn select_rule(&mut self, id: String, cx: &mut Context<Self>) {
        if self.selected.as_deref() == Some(id.as_str()) {
            return;
        }
        self.selected = Some(id);
        self.detail = true;
        self.seed_inputs(cx);
        cx.notify();
    }

    fn open_detail(&mut self, cx: &mut Context<Self>) {
        if self.selected.is_none() {
            return;
        }
        self.detail = true;
        cx.notify();
    }

    /// The rules in the order the list shows them: session group first.
    fn flattened_rules(&self) -> Vec<String> {
        self.session
            .iter()
            .chain(self.global.iter())
            .map(|rule| rule.id.clone())
            .collect()
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let order = self.flattened_rules();
        if order.is_empty() {
            return;
        }
        let current = self
            .selected
            .as_ref()
            .and_then(|id| order.iter().position(|rule| rule == id))
            .unwrap_or(0);
        let next = (current as isize + delta).clamp(0, order.len() as isize - 1) as usize;
        if next == current {
            return;
        }
        let id = order[next].clone();
        self.select_rule(id, cx);
    }

    fn move_selection_to_edge(&mut self, last: bool, cx: &mut Context<Self>) {
        let order = self.flattened_rules();
        let target = if last { order.last() } else { order.first() };
        let Some(id) = target else {
            return;
        };
        let id = id.clone();
        self.select_rule(id, cx);
    }

    /// Move the selected rule one slot up or down inside its own group.
    fn move_selected(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        let Some(id) = self.selected.clone() else {
            return;
        };
        let scope = self.selected_scope();
        let position = self.rules(scope).iter().position(|rule| rule.id == id);
        let Some(position) = position else {
            return;
        };
        let next = position as isize + delta;
        if next < 0 || next >= self.rules(scope).len() as isize {
            return;
        }
        self.rules_mut(scope).swap(position, next as usize);
        self.after_change(scope, &id, cx);
        cx.notify();
    }

    /// Record a change and push it to the right place: session rules are
    /// written to the session right away, global rules stay in the draft.
    fn after_change(&mut self, scope: RuleScope, _id: &str, cx: &mut Context<Self>) {
        match scope {
            RuleScope::Session => self.commit_session(cx),
            RuleScope::Global => {
                // Rebuild instead of a single-id update: adding or removing a
                // rule changes the id set, and the draft marker has to follow.
                self.indexes[1].rebuild(&self.base_global, &self.global);
                self.error = None;
            }
        }
    }

    fn session_error(&self) -> Option<String> {
        self.session_validation
            .error(&self.session)
            .map(|(rule, error)| {
                t!(
                    "terminalCtx.invalidRulePattern",
                    name = rule.name,
                    line = error.line,
                    error = error.message
                )
                .to_string()
            })
    }

    fn start_session_validation(
        &mut self,
        id: String,
        generation: u64,
        input: MatchInput,
        delay: Duration,
        cx: &mut Context<Self>,
    ) {
        let task_id = id.clone();
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            let error = cx.background_spawn(async move { input.validate() }).await;
            let _ = this.update(cx, |this, cx| {
                if this.closed
                    || !this.session_alive
                    || !this.session_validation.finish(&id, generation, error)
                {
                    return;
                }
                this.commit_session(cx);
                if !this.session_validation.has_pending()
                    && let Some(close_after) = this.pending_save.take()
                {
                    this.saving = false;
                    this.seed_inputs(cx);
                    this.persist_global(close_after, cx);
                }
                cx.notify();
            });
        });
        self.session_tasks.insert(task_id, task);
    }

    /// Keep invalid input in the editor and publish only accepted rules.
    /// Validation is per matching input, independent of names, colours and order.
    fn commit_session(&mut self, cx: &mut Context<Self>) {
        if self.closed || !self.session_alive {
            return;
        }
        let jobs = self.session_validation.reconcile(&self.session);
        let ids: HashSet<_> = self.session.iter().map(|rule| rule.id.as_str()).collect();
        self.session_tasks.retain(|id, _| ids.contains(id.as_str()));
        for (id, generation, input) in jobs {
            self.start_session_validation(id, generation, input, Duration::from_millis(200), cx);
        }
        let applied = self
            .app
            .read(cx)
            .session_keyword_rules(&self.session_id)
            .unwrap_or_default();
        let applied_by_id: HashMap<_, _> = applied
            .iter()
            .map(|rule| (rule.id.as_str(), rule))
            .collect();
        let rules: Vec<_> = self
            .session
            .iter()
            .filter_map(|rule| {
                if self.session_validation.valid(&rule.id) {
                    Some(rule.clone())
                } else {
                    applied_by_id
                        .get(rule.id.as_str())
                        .map(|rule| (*rule).clone())
                }
            })
            .collect();
        if rules != applied {
            let owner = self.session_id.clone();
            self.app.update(cx, |app, cx| {
                app.set_session_keyword_rules(&owner, rules, cx);
            });
        }
        self.list_revision = self.list_revision.wrapping_add(1);
    }

    fn add_rule(&mut self, scope: RuleScope, cx: &mut Context<Self>) {
        if self.saving || !self.session_alive {
            return;
        }
        let rule = new_popup_rule();
        let id = rule.id.clone();
        self.rules_mut(scope).push(rule);
        self.after_change(scope, &id, cx);
        self.selected = Some(id);
        self.detail = true;
        self.seed_inputs(cx);
        cx.notify();
    }

    fn remove_rule(&mut self, scope: RuleScope, id: &str, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        let position = self.rules(scope).iter().position(|rule| rule.id == id);
        let Some(position) = position else {
            return;
        };
        self.rules_mut(scope).remove(position);
        if self.selected.as_deref() == Some(id) {
            self.selected = None;
            self.detail = false;
        }
        self.after_change(scope, id, cx);
        self.seed_inputs(cx);
        cx.notify();
    }

    fn set_rule_enabled(
        &mut self,
        scope: RuleScope,
        id: &str,
        checked: bool,
        cx: &mut Context<Self>,
    ) {
        if self.saving {
            return;
        }
        let position = self.rules(scope).iter().position(|rule| rule.id == id);
        let Some(position) = position else {
            return;
        };
        self.rules_mut(scope)[position].enabled = checked;
        self.after_change(scope, id, cx);
        cx.notify();
    }

    /// Toggle one of the matching options of the selected rule.
    fn toggle_match_option(&mut self, option: KeywordMatchOption, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        let Some(id) = self.selected.clone() else {
            return;
        };
        let scope = self.scope_of(&id);
        let position = self.rules(scope).iter().position(|rule| rule.id == id);
        let Some(position) = position else {
            return;
        };
        let enabled = !option.is_enabled(&self.rules(scope)[position]);
        option.apply(&mut self.rules_mut(scope)[position], enabled);
        self.after_change(scope, &id, cx);
        cx.notify();
    }

    fn set_color_value(
        &mut self,
        rule_id: &str,
        field: KeywordHighlightEditorField,
        value: String,
        cx: &mut Context<Self>,
    ) {
        if self.saving {
            return;
        }
        let scope = self.scope_of(rule_id);
        let position = self.rules(scope).iter().position(|rule| rule.id == rule_id);
        let Some(position) = position else {
            return;
        };
        let rule = &mut self.rules_mut(scope)[position];
        match field {
            KeywordHighlightEditorField::ColorDark => rule.color_dark = value,
            KeywordHighlightEditorField::ColorLight => rule.color_light = value,
            KeywordHighlightEditorField::BgDark => rule.bg_color_dark = value,
            KeywordHighlightEditorField::BgLight => rule.bg_color_light = value,
            _ => return,
        }
        self.after_change(scope, rule_id, cx);
        cx.notify();
    }

    fn edit_field(&mut self, field: usize, text: String, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        let Some(id) = self.selected.clone() else {
            return;
        };
        let scope = self.selected_scope();
        let position = self.rules(scope).iter().position(|rule| rule.id == id);
        let Some(position) = position else {
            return;
        };
        let rule = &mut self.rules_mut(scope)[position];
        match field {
            0 => rule.name = text,
            1 => rule.patterns = text.lines().map(str::to_string).collect(),
            _ => return,
        }
        self.after_change(scope, &id, cx);
        cx.notify();
    }

    /// Move a rule inside its group. Global drafts and live session rules must
    /// never participate in a partially committed cross-group move.
    ///
    /// `to_id` is the rule the moved rule is placed next to. `None` means the
    /// target group has no rule to anchor to, so the rule is appended to it.
    fn move_rule(
        &mut self,
        from_scope: RuleScope,
        from_id: String,
        to_scope: RuleScope,
        to_id: Option<String>,
        position: DropPosition,
        cx: &mut Context<Self>,
    ) {
        if self.saving {
            return;
        }
        if from_scope != to_scope {
            return;
        }
        if Some(&from_id) == to_id.as_ref() {
            return;
        }
        let source = self
            .rules(from_scope)
            .iter()
            .position(|rule| rule.id == from_id);
        let Some(source) = source else {
            return;
        };
        let rule = self.rules_mut(from_scope).remove(source);
        let target = self.rules_mut(to_scope);
        let index = match &to_id {
            Some(to_id) => {
                let index = target
                    .iter()
                    .position(|candidate| candidate.id == *to_id)
                    .unwrap_or(target.len());
                let index = if position == DropPosition::After {
                    index + 1
                } else {
                    index
                };
                index.min(target.len())
            }
            // The target group is empty, so the rule becomes its only entry.
            None => target.len(),
        };
        target.insert(index, rule);
        self.selected = Some(from_id.clone());
        self.detail = true;
        self.after_change(from_scope, &from_id, cx);
        if from_scope != to_scope {
            self.after_change(to_scope, &from_id, cx);
        }
        self.seed_inputs(cx);
        cx.notify();
    }

    fn clear_session_rules(&mut self, cx: &mut Context<Self>) {
        if self.saving || !self.session_alive {
            return;
        }
        self.session.clear();
        self.list_revision = self.list_revision.wrapping_add(1);
        self.selected = None;
        self.detail = false;
        self.commit_session(cx);
        self.seed_inputs(cx);
        cx.notify();
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        let app = self.app.read(cx);
        self.global = app.settings.keyword_config().rules.clone();
        self.session = app
            .terminal
            .view
            .views
            .get(&self.session_id)
            .map(|view| view.keyword_highlights.rules().to_vec())
            .unwrap_or_default();
        self.session_tasks.clear();
        self.session_validation = SessionValidation::from_applied(&self.session);
        self.list_revision = self.list_revision.wrapping_add(1);
        self.base_global = self.global.clone();
        self.error = None;
        self.indexes[0].rebuild(&[], &self.session);
        self.indexes[1].rebuild(&self.base_global, &self.global);
        self.seed_inputs(cx);
        cx.notify();
    }

    fn request_close(&mut self, cx: &mut Context<Self>) -> bool {
        if self.saving {
            return false;
        }
        if self.dirty() {
            self.confirm_close = true;
            // The draft lives in the global group, so show it before asking
            // the user to confirm the close.
            if self.selected_scope() != RuleScope::Global
                && let Some(id) = self
                    .global
                    .iter()
                    .find(|rule| self.indexes[1].changed_ids().contains(&rule.id))
                    .map(|rule| rule.id.clone())
            {
                self.selected = Some(id);
                self.detail = true;
                self.seed_inputs(cx);
            }
            cx.notify();
            return false;
        }
        self.close(cx);
        true
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        self.closed = true;
        self.session_tasks.clear();
        self.pending_save = None;
        self.app.update(cx, |app, cx| {
            app.terminal.keyword_popup = KeywordHighlightPopupState::default();
            cx.notify();
        });
        cx.notify();
    }

    /// Persist the global catalogue and close the editor.
    fn save(&mut self, cx: &mut Context<Self>) {
        self.persist_global(true, cx);
    }

    /// Persist the global catalogue and keep the editor open, so the user can
    /// keep working on the rules without reopening the popup.
    fn apply(&mut self, cx: &mut Context<Self>) {
        self.persist_global(false, cx);
    }

    /// Write the global draft to the store.
    ///
    /// `close_after` tells the editor whether the window closes once the store
    /// accepted the change.
    fn persist_global(&mut self, close_after: bool, cx: &mut Context<Self>) {
        if self.saving || self.closed {
            return;
        }
        if let Some((rule, _)) = self.session_validation.error(&self.session) {
            self.selected = Some(rule.id.clone());
            self.detail = true;
            self.focus_pattern = true;
            self.seed_inputs(cx);
            cx.notify();
            return;
        }
        if self.session_validation.has_pending() {
            self.pending_save = Some(close_after);
            self.saving = true;
            self.seed_inputs(cx);
            for (id, generation, input) in self.session_validation.pending() {
                self.start_session_validation(id, generation, input, Duration::ZERO, cx);
            }
            cx.notify();
            return;
        }
        let before: HashMap<_, _> = self
            .base_global
            .iter()
            .map(|rule| (rule.id.as_str(), rule))
            .collect();
        let mut pending = Vec::new();
        for rule in &self.global {
            // The match options decide how a pattern is compiled, so a rule
            // that only switched them still has to be validated: a literal
            // pattern becomes an invalid regex the moment "use regex" is on.
            if before.get(rule.id.as_str()).is_none_or(|old| {
                old.patterns != rule.patterns
                    || old.case_sensitive != rule.case_sensitive
                    || old.whole_word != rule.whole_word
                    || old.use_regex != rule.use_regex
            }) {
                pending.push((
                    rule.id.clone(),
                    rule.name.clone(),
                    rule.patterns.clone(),
                    rule.case_sensitive,
                    rule.whole_word,
                    rule.use_regex,
                ));
            }
        }
        self.saving = true;
        self.validation_generation = self.validation_generation.wrapping_add(1);
        let generation = self.validation_generation;
        self.seed_inputs(cx);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let error = cx
                .background_spawn(async move {
                    pending.into_iter().find_map(
                        |(id, name, patterns, case_sensitive, whole_word, use_regex)| {
                            nyaterm_terminal_gpui::keywords::validation::validate_keyword_highlight_patterns(
                                &patterns,
                                case_sensitive,
                                whole_word,
                                use_regex,
                            )
                            .into_iter()
                            .next()
                            .map(|error| (id, name, error))
                        },
                    )
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.closed || this.validation_generation != generation {
                    return;
                }
                this.saving = false;
                if let Some((id, name, error)) = error {
                    this.selected = Some(id);
                    this.detail = true;
                    this.focus_pattern = true;
                    this.error = Some(
                        t!("terminalCtx.invalidRulePattern", name = name, line = error.line, error = error.message)
                            .to_string(),
                    );
                    this.seed_inputs(cx);
                    cx.notify();
                } else {
                    this.apply_validated_save(close_after, cx);
                    if !this.saving {
                        this.seed_inputs(cx);
                    }
                }
            });
        })
        .detach();
    }

    fn apply_validated_save(&mut self, close_after: bool, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        for rule in self.global.iter_mut() {
            for color in [&mut rule.color_dark, &mut rule.color_light] {
                if parse_popup_swatch(color.trim()).is_none() {
                    self.error = Some(t!("terminalCtx.invalidRuleColor").to_string());
                    cx.notify();
                    return;
                }
                *color = normalize_popup_color(color);
            }
            for color in [&mut rule.bg_color_dark, &mut rule.bg_color_light] {
                if color.trim().is_empty() {
                    color.clear();
                    continue;
                }
                if parse_popup_swatch(color.trim()).is_none() {
                    self.error = Some(t!("terminalCtx.invalidRuleColor").to_string());
                    cx.notify();
                    return;
                }
                *color = normalize_popup_color(color);
            }
        }
        let root = self.app.read(cx);
        let merged = merge_rules(
            &self.base_global,
            &self.global,
            &root.settings.keyword_config().rules,
        );
        let mut global = match merged {
            Ok(value) => value,
            Err(error) => {
                self.error = Some(error);
                cx.notify();
                return;
            }
        };
        if order_changed(&self.base_global, &self.global) {
            global = reorder_merged(global, &self.global);
        }
        let mut config = root.settings.keyword_config().clone();
        let submission_base = config.clone();
        let changed = config.rules != global;
        config.rules = global;
        let weak = cx.entity().downgrade();
        self.saving = true;
        self.seed_inputs(cx);
        cx.notify();
        if !changed {
            self.adopt_applied_rules(config.rules.clone(), cx);
            if close_after {
                self.close(cx);
            }
            return;
        }
        self.app.update(cx, |app, cx| {
            app.submit_store_request(
                0,
                nyaterm_store::store_mutation(nyaterm_store::StoreDomain::Settings, move |store| {
                    store.save_keyword_highlights(&config)
                }),
                move |app, event, cx| {
                    match event.outcome {
                        Ok(config) => {
                            if app.settings.keyword_config() != &submission_base {
                                // A concurrent settings save owns the new configuration.
                                // Restore that value on disk rather than publishing this stale completion.
                                app.save_keyword_highlights(cx);
                                let _ = weak.update(cx, |editor, cx| {
                                    editor.saving = false;
                                    editor.error = Some(t!("terminalCtx.ruleConflict").to_string());
                                    editor.seed_inputs(cx);
                                    cx.notify();
                                });
                                return;
                            }
                            app.settings
                                .replace_keyword_highlight_rules(config.rules.clone());
                            app.settings.finish_keyword_persistence(&config);
                            app.invalidate_paint_theme_caches();
                            app.refresh_visible_terminal_surfaces(cx);
                            if close_after {
                                app.terminal.keyword_popup = KeywordHighlightPopupState::default();
                            }
                            let _ = weak.update(cx, |editor, cx| {
                                if close_after {
                                    editor.closed = true;
                                } else {
                                    // The applied rules are the new baseline, so
                                    // the draft markers and the unsaved notice
                                    // disappear while the editor stays open.
                                    editor.adopt_applied_rules(config.rules.clone(), cx);
                                }
                                cx.notify();
                            });
                        }
                        Err(error) => {
                            let _ = weak.update(cx, |editor, cx| {
                                editor.saving = false;
                                editor.error = Some(error.to_string());
                                editor.seed_inputs(cx);
                                cx.notify();
                            });
                        }
                    }
                    cx.notify();
                },
                cx,
            );
        });
    }

    /// Take the persisted rules as the new baseline.
    ///
    /// "Apply" writes the draft to the store and keeps the editor open. The
    /// rules that are now on disk are no longer a draft, so the per-rule
    /// markers and the unsaved notice have to disappear.
    fn adopt_applied_rules(&mut self, applied: Vec<KeywordHighlightRule>, cx: &mut Context<Self>) {
        self.global = applied.clone();
        self.base_global = applied;
        self.indexes[1].rebuild(&self.base_global, &self.global);
        self.error = None;
        self.confirm_close = false;
        self.saving = false;
        self.list_revision = self.list_revision.wrapping_add(1);
        self.seed_inputs(cx);
        cx.notify();
    }

    fn color_row(
        &self,
        rule: &KeywordHighlightRule,
        field: KeywordHighlightEditorField,
        label: SharedString,
        is_dark: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = self.palette;
        let is_background = matches!(
            field,
            KeywordHighlightEditorField::BgDark | KeywordHighlightEditorField::BgLight
        );
        let value = PopupColorPickerControls::color_of(rule, field);
        let interactive = !self.saving && self.color_pickers.get(&rule.id, field).is_some();
        let control: AnyElement = if interactive {
            let picker = self.color_pickers.get(&rule.id, field).unwrap();
            let placeholder: SharedString = if is_background {
                t!("settings.keywordHighlightNoBackground").into()
            } else {
                "#rrggbb".into()
            };
            ColorSelect::new(picker)
                .w(px(Self::COLOR_SELECT_WIDTH))
                .featured_colors(PopupColorPickerControls::featured_colors(is_dark))
                .placeholder(placeholder)
                .accessibility_label(label.clone())
                .into_any_element()
        } else {
            div()
                .h(px(32.))
                .w(px(Self::COLOR_SELECT_WIDTH))
                .px_2()
                .flex()
                .items_center()
                .rounded_sm()
                .border_1()
                .border_color(rgb(palette.border))
                .bg(rgb(palette.input))
                .text_xs()
                .text_color(rgb(palette.text_muted))
                .child(if value.trim().is_empty() {
                    " ".to_string()
                } else {
                    value.to_string()
                })
                .into_any_element()
        };
        let mut row = div().flex().items_center().gap_2().child(
            div()
                .w(px(Self::COLOR_LABEL_WIDTH))
                .flex_none()
                .text_xs()
                .whitespace_nowrap()
                .text_color(rgb(palette.text_muted))
                .child(label),
        );
        if is_background && !value.trim().is_empty() && !self.saving {
            let rule_id = rule.id.clone();
            row = row.child(control).child(popup_icon_button(
                palette,
                format!("keyword-bg-clear-{}", field.input_key()),
                IconName::Close,
                t!("settings.keywordHighlightClearBackground"),
                true,
                true,
                cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    let id = rule_id.clone();
                    this.set_color_value(&id, field, String::new(), cx);
                    if let Some(picker) = this.color_pickers.get(&id, field) {
                        picker.update(cx, |picker, cx| picker.clear_value(window, cx));
                    }
                    this.color_pickers.set_cached_value(&id, field, "");
                }),
            ));
        } else {
            row = row.child(control);
        }
        row.into_any_element()
    }

    /// Combined preview of a single theme: the sample text painted with that
    /// theme's text colour over its background colour.
    ///
    /// The preview lives inside the theme block it belongs to, so the light
    /// palette is previewed by the light row and the dark palette by the dark
    /// row.
    fn preview_panel(&self, rule: &KeywordHighlightRule, is_dark: bool) -> AnyElement {
        let palette = self.palette;
        let (text_value, bg_value, fallback) = if is_dark {
            (
                rule.color_dark.as_str(),
                rule.bg_color_dark.as_str(),
                0x79c0ff,
            )
        } else {
            (
                rule.color_light.as_str(),
                rule.bg_color_light.as_str(),
                0x0969da,
            )
        };
        let text = parse_popup_swatch(text_value).unwrap_or(fallback);
        let bg = parse_popup_swatch(bg_value).unwrap_or(palette.input);
        div()
            .flex_1()
            .min_w(px(120.))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .rounded_md()
            .border_1()
            .border_color(rgb(palette.border))
            .bg(rgb(bg))
            .text_color(rgb(text))
            .font_family(crate::features::shell::gpui_code_font_family())
            .px_2()
            .py_1()
            .text_xs()
            .text_center()
            .child(t!("settings.keywordHighlightPreviewSample"))
            .into_any_element()
    }

    fn detail(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(rule) = self.selected_rule() else {
            return div()
                .child(t!("terminalCtx.keywordHighlightSelectHint"))
                .into_any_element();
        };
        let palette = self.palette;
        let mut form = div().id("keyword-details").flex().flex_col().gap_3().p_2();
        form = form.child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(palette.text_muted))
                        .child(t!("settings.keywordHighlightRuleName")),
                )
                .child(NyaInputShell::new("keyword-field-0", &self.inputs[0]).height(px(32.))),
        );
        form = form.child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(palette.text_muted))
                        .child(t!("settings.keywordHighlightRulePatterns")),
                )
                .child({
                    let editor = cx.entity().downgrade();
                    NyaInputShell::new("keyword-field-1", &self.inputs[1])
                        .multi_line()
                        .height(px(120.))
                        .bottom_right(keyword_match_option_toggles(
                            palette,
                            rule.case_sensitive,
                            rule.whole_word,
                            rule.use_regex,
                            Rc::new(
                                move |option: KeywordMatchOption,
                                      _window: &mut Window,
                                      cx: &mut App| {
                                    editor
                                        .update(cx, |editor, cx| {
                                            editor.toggle_match_option(option, cx);
                                        })
                                        .ok();
                                },
                            ),
                        ))
                }),
        );
        for (title, is_dark) in [
            (t!("settings.keywordHighlightLightPalette"), false),
            (t!("settings.keywordHighlightDarkPalette"), true),
        ] {
            form = form.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .font_weight(gpui::FontWeight(600.))
                            .text_color(rgb(palette.text_muted))
                            .child(title),
                    )
                    // The colour rows stack on the left and the preview of the
                    // very same theme sits beside them, spanning both rows.
                    .child(
                        div()
                            .flex()
                            .items_stretch()
                            .gap_3()
                            .child(
                                div()
                                    .w(px(Self::COLOR_CONTROL_WIDTH))
                                    .flex_none()
                                    .flex()
                                    .flex_col()
                                    .gap_2()
                                    .child(self.color_row(
                                        rule,
                                        if is_dark {
                                            KeywordHighlightEditorField::ColorDark
                                        } else {
                                            KeywordHighlightEditorField::ColorLight
                                        },
                                        t!("settings.keywordHighlightTextColor").into(),
                                        is_dark,
                                        cx,
                                    ))
                                    .child(self.color_row(
                                        rule,
                                        if is_dark {
                                            KeywordHighlightEditorField::BgDark
                                        } else {
                                            KeywordHighlightEditorField::BgLight
                                        },
                                        t!("settings.keywordHighlightBackgroundColor").into(),
                                        is_dark,
                                        cx,
                                    )),
                            )
                            .child(self.preview_panel(rule, is_dark)),
                    ),
            );
        }
        div()
            .relative()
            .h_full()
            .min_h_0()
            .child(
                form.h_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.detail_scroll),
            )
            .vertical_scrollbar(&self.detail_scroll)
            .into_any_element()
    }

    /// Keyboard handling for the rule list.
    fn on_list_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input_focused = self
            .inputs
            .iter()
            .any(|input| input.read(cx).focus_handle().is_focused(window));
        if input_focused {
            return;
        }
        let alt = event.keystroke.modifiers.alt;
        match event.keystroke.key.as_str() {
            "up" => {
                if alt {
                    self.move_selected(-1, cx);
                } else {
                    self.move_selection(-1, cx);
                }
            }
            "down" => {
                if alt {
                    self.move_selected(1, cx);
                } else {
                    self.move_selection(1, cx);
                }
            }
            "home" => self.move_selection_to_edge(false, cx),
            "end" => self.move_selection_to_edge(true, cx),
            "delete" => {
                if let Some(id) = self.selected.clone() {
                    let scope = self.selected_scope();
                    self.remove_rule(scope, &id, cx);
                }
            }
            _ => return,
        }
        cx.stop_propagation();
    }
}

impl Render for KeywordHighlightWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.closed {
            window.defer(cx, |window, _| window.remove_window());
            return div().into_any_element();
        }
        let weak = cx.entity().downgrade();
        let on_close: ChildWindowCloseHandler = Rc::new(move |window, cx| {
            if weak
                .update(cx, |editor, cx| editor.request_close(cx))
                .unwrap_or(true)
            {
                window.remove_window();
            }
        });
        let header_close = on_close.clone();
        let palette = self.palette;
        focus_child_window_shell_if_idle(&self.focus, window, cx);
        if std::mem::take(&mut self.focus_pattern) {
            let focus = self.inputs[1].read(cx).focus_handle();
            let scroll = self.detail_scroll.clone();
            let editor = cx.entity().downgrade();
            // The diagnostic changes the form's viewport. Wait for its new
            // bounds before revealing the pattern field, then repaint.
            window.on_next_frame(move |window, cx| {
                scroll.scroll_to_top_of_item(1);
                window.focus(&focus, cx);
                let _ = editor.update(cx, |_, cx| cx.notify());
            });
        }
        let selected_for_pickers = self.selected_rule().cloned();
        self.color_pickers
            .sync(selected_for_pickers.as_ref(), window, cx);
        self.sync_list(window, cx);
        let root = self.app.read(cx);
        let font = root.gpui_ui_font().font();
        let font_size = root.settings.summary().ui_font_size.clamp(12, 24) as f32;
        let wide = f32::from(window.viewport_size().width) >= 760.;
        let mut body = div().flex().gap_3().flex_1().min_h_0();
        if wide || !self.detail {
            body = body.child(
                div()
                    .when(wide, |d| d.w(px(280.)).flex_none())
                    .when(!wide, |d| d.flex_1())
                    .min_h_0()
                    .on_key_down(cx.listener(|this, event, window, cx| {
                        this.on_list_key_down(event, window, cx);
                    }))
                    .child(self.list(cx)),
            );
        }
        if wide || self.detail {
            body = body.child(div().flex_1().min_w_0().min_h_0().child(self.detail(cx)));
        }
        // The footer is always visible: it carries the hint about the list
        // interactions on the left and the action row on the right.
        let actions_disabled = self.saving || !self.dirty() || !self.session_alive;
        let mut footer_leading = div()
            .flex()
            .items_center()
            .gap_3()
            .flex_1()
            .min_w_0()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .text_xs()
                    .text_color(rgb(palette.text_dimmed))
                    .child(t!(
                        "terminalCtx.ruleListHint",
                        shortcuts = t!("terminalCtx.ruleHintShortcuts").to_string(),
                        drag = t!("terminalCtx.ruleHintDrag").to_string(),
                    ))
                    .child(t!("terminalCtx.ruleHintPriority")),
            );
        if (self.error.is_some() || self.session_error().is_some()) && !self.saving {
            footer_leading = footer_leading.child(
                NyaButton::new("keyword-reload", t!("terminalCtx.ruleReload"))
                    .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
            );
        }
        // A close attempt with unsaved edits is reported next to the cancel
        // button instead of adding a second pair of buttons to the footer.
        let unsaved_notice = if self.confirm_close {
            Some(unsaved_changes_notice(
                palette,
                t!("terminalCtx.ruleUnsavedChanges"),
                t!("terminalCtx.ruleDiscard"),
                cx.listener(|this, _, window, cx| {
                    this.close(cx);
                    window.remove_window();
                }),
            ))
        } else {
            None
        };
        let actions = div()
            .flex()
            .items_center()
            .gap_2()
            .flex_none()
            .when_some(unsaved_notice, |d, notice| d.child(notice))
            .child(popup_footer_button(
                palette,
                "keyword-cancel",
                t!("common.cancel"),
                false,
                self.saving,
                cx.listener(|this, _, window, cx| {
                    if this.confirm_close {
                        // The second cancel keeps the draft and stays open.
                        this.confirm_close = false;
                        cx.notify();
                        return;
                    }
                    if this.request_close(cx) {
                        window.remove_window();
                    }
                }),
            ))
            // "Apply" writes the rules without closing the editor, so the
            // user can keep editing them right away.
            .child(popup_footer_button(
                palette,
                "keyword-apply",
                t!("common.apply"),
                false,
                actions_disabled,
                cx.listener(|this, _, _, cx| this.apply(cx)),
            ))
            .child(popup_footer_button(
                palette,
                "keyword-save",
                t!("common.save"),
                true,
                actions_disabled,
                cx.listener(|this, _, _, cx| this.save(cx)),
            ));
        let footer = div()
            .flex()
            .gap_2()
            .items_center()
            .justify_between()
            .flex_none()
            .child(footer_leading)
            .child(actions);
        let title = t!("terminalCtx.keywordHighlightTitle").to_string();
        child_window_root(&self.focus, true, on_close)
            .bg(rgb(palette.bg))
            .font(font)
            .text_size(px(font_size))
            .text_color(rgb(palette.text))
            .child(child_window_header(
                palette,
                title,
                None,
                self.chrome,
                window,
                cx,
                move |_, window, cx| header_close(window, cx),
            ))
            .child(
                div()
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .flex_1()
                    .min_h_0()
                    .when(!self.session_alive, |d| {
                        d.child(t!("terminalCtx.ruleSessionClosed"))
                    })
                    .when_some(
                        self.session_error().or_else(|| self.error.clone()),
                        |d, error| {
                            d.child(
                                div()
                                    .w_full()
                                    .min_w_0()
                                    .max_h(px(96.))
                                    .flex_shrink_0()
                                    .overflow_y_scrollbar()
                                    .text_color(rgb(palette.danger))
                                    .child(error),
                            )
                        },
                    )
                    .child(body)
                    .child(footer),
            )
            .into_any_element()
    }
}

impl KeywordHighlightWindow {
    /// The rule list with its search box and the two rule groups.
    fn list(&self, _cx: &mut Context<Self>) -> AnyElement {
        let Some(list) = self.list.as_ref() else {
            return div().into_any_element();
        };
        let palette = self.palette;
        NyaList::new(list)
            .search_placeholder(t!("terminalCtx.ruleSearch"))
            .h_full()
            .min_h_0()
            .rounded_md()
            .border_1()
            .border_color(rgb(palette.border))
            .bg(rgb(palette.surface))
            .into_any_element()
    }
}

fn new_popup_rule() -> KeywordHighlightRule {
    KeywordHighlightRule {
        id: format!("kh-{}", uuid::Uuid::new_v4()),
        name: t!("settings.keywordHighlightNewRule").to_string(),
        patterns: Vec::new(),
        color_dark: "#79c0ff".into(),
        color_light: "#0969da".into(),
        bg_color_dark: String::new(),
        bg_color_light: String::new(),
        enabled: true,
        case_sensitive: false,
        whole_word: false,
        use_regex: true,
    }
}

fn empty_popup_rule() -> KeywordHighlightRule {
    KeywordHighlightRule {
        id: String::new(),
        name: String::new(),
        patterns: Vec::new(),
        color_dark: "#79c0ff".into(),
        color_light: "#0969da".into(),
        bg_color_dark: String::new(),
        bg_color_light: String::new(),
        enabled: true,
        case_sensitive: false,
        whole_word: false,
        use_regex: true,
    }
}

/// Build a session rule from a terminal selection: name is the first non-empty
/// line, patterns are the trimmed non-empty lines.
fn prefill_rule_from_text(selection: String) -> Option<KeywordHighlightRule> {
    let lines: Vec<String> = selection
        .lines()
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty())
        .collect();
    if lines.is_empty() {
        return None;
    }
    let name = lines.first().cloned().unwrap_or_default();
    Some(KeywordHighlightRule {
        id: format!("kh-{}", uuid::Uuid::new_v4()),
        name: if name.chars().count() > 40 {
            name.chars().take(40).collect()
        } else {
            name
        },
        patterns: lines.iter().map(|line| regex::escape(line)).collect(),
        color_dark: "#79c0ff".into(),
        color_light: "#0969da".into(),
        bg_color_dark: String::new(),
        bg_color_light: String::new(),
        enabled: true,
        case_sensitive: false,
        whole_word: false,
        use_regex: true,
    })
}

fn normalize_popup_color(value: &str) -> String {
    let value = value.trim();
    let digits = value.strip_prefix('#').unwrap_or(value);
    let mut normalized = String::from("#");
    normalized.extend(
        digits
            .chars()
            .filter(char::is_ascii_hexdigit)
            .take(6)
            .map(|ch| ch.to_ascii_lowercase()),
    );
    normalized
}

fn parse_popup_swatch(value: &str) -> Option<u32> {
    let digits = value.strip_prefix('#').unwrap_or(value);
    if digits.len() != 6 {
        return None;
    }
    u32::from_str_radix(digits, 16).ok()
}

/// Footer button styled like the settings panel's cancel/confirm pair: an
/// outlined secondary button and a filled primary button, both 28px tall.
fn popup_footer_button(
    palette: ThemePalette,
    id: &'static str,
    label: impl Into<SharedString>,
    primary: bool,
    disabled: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let label: SharedString = label.into();
    let button = div()
        .id(id)
        .h(px(28.))
        .min_w(px(64.))
        .px_4()
        .flex()
        .items_center()
        .justify_center()
        .rounded_md()
        .text_size(px(11.));
    let button = if primary {
        let fill = if disabled {
            palette.text_dimmed
        } else {
            palette.link
        };
        button
            .bg(rgb(fill))
            .font_weight(gpui::FontWeight(600.))
            .text_color(rgb(palette.text_on_fill(fill)))
            .when(!disabled, |this| this.cursor_pointer().on_click(on_click))
    } else {
        button
            .border_1()
            .border_color(rgb(palette.border))
            .text_color(if disabled {
                rgb(palette.text_dimmed)
            } else {
                rgb(palette.text)
            })
            .when(!disabled, |this| {
                this.cursor_pointer()
                    .hover(move |this| this.bg(rgb(palette.hover)))
                    .on_click(on_click)
            })
    };
    button.child(label)
}

/// Warning notice shown next to the cancel button when a close attempt would
/// drop unsaved edits. Clicking it discards the draft and closes the window.
fn unsaved_changes_notice(
    palette: ThemePalette,
    label: impl Into<SharedString>,
    tooltip: impl Into<SharedString>,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let label: SharedString = label.into();
    let tooltip: SharedString = tooltip.into();
    div()
        .id("keyword-unsaved-notice")
        .h(px(28.))
        .px_3()
        .flex()
        .items_center()
        .gap_1()
        .rounded_md()
        .border_1()
        .border_color(rgba((palette.warning << 8) | 0x66))
        .bg(rgba((palette.warning << 8) | 0x1f))
        .text_size(px(11.))
        .text_color(rgb(palette.warning))
        .cursor_pointer()
        .hover(move |this| this.bg(rgba((palette.warning << 8) | 0x33)))
        .tooltip(move |window, cx| NyaTooltip::new(tooltip.clone()).build(window, cx))
        .child(
            NyaIcon::new(IconName::CircleAlert)
                .size(px(13.))
                .text_color(rgb(palette.warning)),
        )
        .child(label)
        .on_click(on_click)
}

/// Compact ghost icon button used in the rule list and the group headers.
#[allow(clippy::too_many_arguments)]
fn popup_icon_button(
    palette: ThemePalette,
    id: impl Into<SharedString>,
    icon: IconName,
    tooltip: impl Into<SharedString>,
    enabled: bool,
    danger: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let tooltip: SharedString = tooltip.into();
    let idle = if danger {
        palette.danger
    } else {
        palette.text_muted
    };
    let hover_bg = if danger {
        rgba((palette.danger << 8) | 0x20)
    } else {
        rgba((palette.hover << 8) | 0x40)
    };
    div()
        .id(id.into())
        .size(px(28.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_sm()
        .text_color(rgb(idle))
        .child(NyaIcon::new(icon).size(px(14.)).text_color(rgb(idle)))
        .tooltip(move |window, cx| NyaTooltip::new(tooltip.clone()).build(window, cx))
        .when(enabled, |this| {
            this.cursor_pointer()
                .hover(move |this| this.bg(hover_bg))
                .on_click(on_click)
        })
        .when(!enabled, |this| this.opacity(0.4))
}

#[cfg(test)]
mod tests {
    use super::{
        KeywordHighlightWindow, KeywordMatchOption, RuleScope, merge_rules, new_popup_rule,
        order_changed, prefill_rule_from_text,
    };
    use crate::features::NyaTermApp;
    use crate::features::test_support::app_with_visible_local_session;
    use crate::features::view_widgets::ChildWindowSpec;
    use crate::test_support::{TestAppCleanup, TestConfigDir};
    use gpui::{AppContext as _, TestAppContext};

    /// rust-i18n expands every catalog entry into one lazy initializer, and
    /// that initializer does not fit on a test thread's stack. The app
    /// preloads the catalog on a worker with a large stack; the tests have to
    /// do the same before the first translation.
    fn preload_catalog() {
        crate::preload_i18n().unwrap();
    }

    /// Build an editor without a child window. The list needs a window for its
    /// search field, so the tests drive the model directly.
    fn editor_for(
        cx: &mut TestAppContext,
        app: gpui::Entity<NyaTermApp>,
        session_id: &str,
        prefill: &str,
    ) -> gpui::Entity<KeywordHighlightWindow> {
        let editor = cx.new(|cx| {
            KeywordHighlightWindow::new(
                app,
                session_id.into(),
                prefill.into(),
                ChildWindowSpec::modal_editor("test".into(), 880., 600.).chrome(),
                None,
                cx,
            )
        });
        settle_session(cx);
        editor
    }

    fn settle_session(cx: &mut TestAppContext) {
        cx.run_until_parked();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(200));
        cx.run_until_parked();
    }

    #[test]
    fn reading_prefill_escapes_selected_text_as_literal_patterns() {
        preload_catalog();
        for text in ["192.168.1.1", "[ERROR]", "[", r"C:/work/file", "中文(a+b)"] {
            let rule = prefill_rule_from_text(text.into()).unwrap();
            assert_eq!(rule.patterns, vec![regex::escape(text)]);
            let matcher = regex::Regex::new(&format!("^(?:{})$", rule.patterns[0])).unwrap();
            assert!(matcher.is_match(text));
        }
        let rule = prefill_rule_from_text("192.168.1.1".into()).unwrap();
        assert!(
            !regex::Regex::new(&rule.patterns[0])
                .unwrap()
                .is_match("192x168x1x1")
        );
    }

    #[test]
    fn regression_prefill_discards_empty_lines() {
        preload_catalog();
        let rule = prefill_rule_from_text("  ERROR \n\n warning ".into()).unwrap();
        assert_eq!(rule.patterns, vec!["ERROR", "warning"]);
        assert!(prefill_rule_from_text(" \n ".into()).is_none());
    }

    #[test]
    fn opening_editor_for_existing_content_reuses_rule_instead_of_prefilling_a_second() {
        preload_catalog();
        let dir = TestConfigDir::new("nyaterm-highlight-prefill-dedup");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "local");

        // A quick color already created a session rule for this text.
        let mut existing = prefill_rule_from_text("ERROR".into()).unwrap();
        existing.id = "kh-quick-existing".into();
        existing.color_dark = "#f44747".into();
        cx.update_entity(&app, |app, cx| {
            app.set_session_keyword_rules("local", vec![existing.clone()], cx);
        });

        // Opening the editor with the same selection must reuse that rule.
        let editor = editor_for(&mut cx, app, "local", "ERROR");
        editor.read_with(&cx, |editor, _| {
            assert_eq!(editor.session.len(), 1, "prefill added a duplicate rule");
            assert_eq!(editor.session[0].id, "kh-quick-existing");
            assert_eq!(
                editor.selected.as_deref(),
                Some("kh-quick-existing"),
                "existing rule was not selected"
            );
        });
    }

    /// Session rules are applied to the session as soon as they change, so
    /// the editor never holds a session draft.
    #[test]
    fn session_rule_changes_reach_the_session_without_saving() {
        preload_catalog();
        let dir = TestConfigDir::new("nyaterm-highlight-session-immediate");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "local");
        let editor = editor_for(&mut cx, app.clone(), "local", "");

        editor.update(&mut cx, |editor, cx| {
            editor.add_rule(RuleScope::Session, cx);
            let id = editor.selected.clone().unwrap();
            editor.edit_field(0, "session rule".into(), cx);
            editor.edit_field(1, "alpha\nbeta".into(), cx);
            editor.set_rule_enabled(RuleScope::Session, &id, false, cx);
        });

        settle_session(&mut cx);
        app.read_with(&cx, |app, _| {
            let rules = app.session_keyword_rules("local").unwrap();
            assert_eq!(rules.len(), 1);
            assert_eq!(rules[0].name, "session rule");
            assert_eq!(rules[0].patterns, vec!["alpha", "beta"]);
            assert!(!rules[0].enabled);
        });
        editor.read_with(&cx, |editor, _| {
            // Once validation accepts a session edit, there is no pending draft.
            assert!(!editor.dirty());
        });
    }

    /// Invalid input remains editable while the last applied state survives.
    #[test]
    fn invalid_session_pattern_stays_in_draft_without_replacing_applied_rules() {
        preload_catalog();
        let dir = TestConfigDir::new("nyaterm-highlight-session-invalid");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "local");
        let editor = editor_for(&mut cx, app.clone(), "local", "ERROR");

        editor.update(&mut cx, |editor, cx| {
            editor.edit_field(1, "[".into(), cx);
        });

        settle_session(&mut cx);
        editor.update(&mut cx, |editor, cx| {
            editor.edit_field(0, "renamed".into(), cx)
        });
        editor.read_with(&cx, |editor, _| {
            assert!(editor.session_error().is_some());
            assert_eq!(editor.session[0].patterns, vec!["["]);
            assert!(editor.dirty());
        });
        app.read_with(&cx, |app, _| {
            let rules = app.session_keyword_rules("local").unwrap();
            assert_eq!(rules[0].patterns, vec!["ERROR"]);
        });
    }

    /// Global rules keep a draft. The draft is reported per rule, not on the
    /// window title.
    #[test]
    fn global_rule_edits_mark_only_the_changed_rule_as_draft() {
        preload_catalog();
        let dir = TestConfigDir::new("nyaterm-highlight-global-draft");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "local");
        let mut first = new_popup_rule();
        first.id = "global-a".into();
        first.name = "alpha".into();
        let mut second = new_popup_rule();
        second.id = "global-b".into();
        second.name = "beta".into();
        app.update(&mut cx, |app, _| {
            app.settings
                .replace_keyword_highlight_rules(vec![first.clone(), second.clone()])
        });

        let editor = editor_for(&mut cx, app, "local", "");
        editor.update(&mut cx, |editor, cx| {
            editor.select_rule("global-a".into(), cx);
            editor.edit_field(0, "alpha edited".into(), cx);
        });

        editor.read_with(&cx, |editor, _| {
            assert!(editor.dirty());
            let dirty = editor.dirty_ids();
            assert!(dirty.contains("global-a"));
            assert!(!dirty.contains("global-b"));
        });
    }

    /// A matching option toggled on a global rule belongs to that rule's
    /// draft and keeps the other options untouched.
    #[test]
    fn toggling_a_match_option_updates_the_rule_and_its_draft() {
        preload_catalog();
        let dir = TestConfigDir::new("nyaterm-highlight-match-option");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "local");
        let mut rule = new_popup_rule();
        rule.id = "global-opt".into();
        rule.name = "opt".into();
        app.update(&mut cx, |app, _| {
            app.settings
                .replace_keyword_highlight_rules(vec![rule.clone()])
        });

        let editor = editor_for(&mut cx, app, "local", "");
        editor.update(&mut cx, |editor, cx| {
            editor.select_rule("global-opt".into(), cx);
            editor.toggle_match_option(KeywordMatchOption::CaseSensitive, cx);
        });

        editor.read_with(&cx, |editor, _| {
            assert!(editor.global[0].case_sensitive);
            assert!(!editor.global[0].whole_word);
            assert!(editor.global[0].use_regex);
            assert!(editor.dirty());
            assert!(editor.dirty_ids().contains("global-opt"));
        });

        // Toggling the same option again clears it and drops the draft.
        editor.update(&mut cx, |editor, cx| {
            editor.toggle_match_option(KeywordMatchOption::CaseSensitive, cx);
        });
        editor.read_with(&cx, |editor, _| {
            assert!(!editor.global[0].case_sensitive);
            assert!(!editor.dirty());
        });
    }

    /// A matching option toggled on a session rule reaches the session right
    /// away, just like any other session edit.
    #[test]
    fn toggling_a_session_match_option_reaches_the_session() {
        preload_catalog();
        let dir = TestConfigDir::new("nyaterm-highlight-session-option");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "local");
        let editor = editor_for(&mut cx, app.clone(), "local", "");

        editor.update(&mut cx, |editor, cx| {
            editor.add_rule(RuleScope::Session, cx);
            editor.edit_field(1, "alpha".into(), cx);
            let id = editor.selected.clone().unwrap();
            editor.toggle_match_option(KeywordMatchOption::WholeWord, cx);
            editor.set_rule_enabled(RuleScope::Session, &id, true, cx);
        });

        settle_session(&mut cx);
        app.read_with(&cx, |app, _| {
            let rules = app.session_keyword_rules("local").unwrap();
            assert_eq!(rules.len(), 1);
            assert!(rules[0].whole_word);
            assert!(!rules[0].case_sensitive);
            assert!(rules[0].use_regex);
        });
    }

    /// A new global rule is a draft on its own, even though the rules both
    /// sides share stayed identical.
    #[test]
    fn adding_a_global_rule_creates_a_draft() {
        preload_catalog();
        let dir = TestConfigDir::new("nyaterm-highlight-global-add");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "local");
        let editor = editor_for(&mut cx, app, "local", "");

        editor.update(&mut cx, |editor, cx| {
            editor.add_rule(RuleScope::Global, cx);
        });

        editor.read_with(&cx, |editor, _| {
            assert!(editor.dirty());
            assert_eq!(editor.global.len(), 1);
            assert!(editor.dirty_ids().contains(&editor.global[0].id));
        });
    }

    /// "Apply" writes the global rules and keeps the editor open, so the
    /// rules that are now on disk stop being a draft.
    #[test]
    fn applying_the_global_rules_clears_the_draft_without_closing() {
        preload_catalog();
        let dir = TestConfigDir::new("nyaterm-highlight-apply-draft");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "local");
        let editor = editor_for(&mut cx, app, "local", "");

        editor.update(&mut cx, |editor, cx| {
            editor.add_rule(RuleScope::Global, cx);
            editor.edit_field(0, "global rule".into(), cx);
            editor.edit_field(1, "alpha\nbeta".into(), cx);
            assert!(editor.dirty(), "the edited rule is a draft");
            // The store accepted the rules, so they become the new baseline.
            editor.adopt_applied_rules(editor.global.clone(), cx);
        });

        editor.read_with(&cx, |editor, _| {
            assert!(!editor.closed, "applying must not close the editor");
            assert!(!editor.dirty(), "the applied rules are no longer a draft");
            assert!(editor.dirty_ids().is_empty());
        });
    }

    /// "Apply" reaches the store and leaves the editor open, unlike "Save",
    /// which closes it.
    #[test]
    fn applying_the_global_rules_keeps_the_editor_open() {
        preload_catalog();
        let dir = TestConfigDir::new("nyaterm-highlight-apply-store");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "local");
        let editor = editor_for(&mut cx, app.clone(), "local", "");

        editor.update(&mut cx, |editor, cx| {
            editor.add_rule(RuleScope::Global, cx);
            editor.edit_field(0, "applied rule".into(), cx);
            editor.edit_field(1, "alpha".into(), cx);
        });
        editor.update(&mut cx, |editor, cx| editor.apply(cx));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            cx.run_until_parked();
            if editor.read_with(&cx, |editor, _| !editor.saving) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "keyword persistence did not finish"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }

        editor.read_with(&cx, |editor, cx| {
            assert!(!editor.closed, "applying must not close the editor");
            assert!(
                editor
                    .inputs
                    .iter()
                    .all(|input| !input.read(cx).is_disabled())
            );
            assert!(!editor.dirty(), "the applied rules are no longer a draft");
        });
        app.read_with(&cx, |app, _| {
            assert!(
                app.settings
                    .keyword_config()
                    .rules
                    .iter()
                    .any(|rule| rule.name == "applied rule"),
                "the applied rule did not reach the settings"
            );
        });
    }

    #[test]
    fn apply_with_no_effective_change_adopts_current_baseline_and_enables_inputs() {
        preload_catalog();
        let dir = TestConfigDir::new("nyaterm-highlight-noop-apply");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "local");
        let mut rule = new_popup_rule();
        rule.patterns = vec!["ERROR".into()];
        app.update(&mut cx, |app, _| {
            app.settings.replace_keyword_highlight_rules(vec![rule])
        });
        let editor = editor_for(&mut cx, app.clone(), "local", "");
        editor.update(&mut cx, |editor, cx| {
            editor.select_rule(editor.global[0].id.clone(), cx);
            editor.edit_field(0, "same concurrent edit".into(), cx);
            app.update(cx, |app, _| {
                app.settings
                    .replace_keyword_highlight_rules(editor.global.clone())
            });
            editor.apply(cx);
        });
        for _ in 0..8 {
            cx.run_until_parked();
        }
        editor.read_with(&cx, |editor, cx| {
            assert!(!editor.dirty());
            assert!(!editor.saving);
            assert!(!editor.closed);
            assert!(
                editor
                    .inputs
                    .iter()
                    .all(|input| !input.read(cx).is_disabled())
            );
        });
    }

    #[test]
    fn cross_group_moves_leave_both_draft_and_applied_rules_unchanged() {
        preload_catalog();
        for prefill in ["", "ERROR"] {
            let dir = TestConfigDir::new("nyaterm-highlight-reject-cross-group");
            let mut cx = TestAppContext::single();
            let _cleanup = TestAppCleanup::new(&cx);
            let app = app_with_visible_local_session(&mut cx, dir.path(), "local");
            let mut global = new_popup_rule();
            global.id = "global-a".into();
            app.update(&mut cx, |app, _| {
                app.settings
                    .replace_keyword_highlight_rules(vec![global.clone()])
            });
            let editor = editor_for(&mut cx, app.clone(), "local", prefill);
            editor.update(&mut cx, |editor, cx| {
                let before = editor.session.clone();
                editor.move_rule(
                    RuleScope::Global,
                    global.id.clone(),
                    RuleScope::Session,
                    before.first().map(|rule| rule.id.clone()),
                    super::list::DropPosition::After,
                    cx,
                );
                if let Some(rule) = before.first() {
                    editor.move_rule(
                        RuleScope::Session,
                        rule.id.clone(),
                        RuleScope::Global,
                        Some(global.id.clone()),
                        super::list::DropPosition::Before,
                        cx,
                    );
                }
                assert_eq!(editor.global, vec![global]);
                assert_eq!(editor.session, before);
                assert!(!editor.dirty());
                assert_eq!(
                    editor
                        .app
                        .read(cx)
                        .session_keyword_rules("local")
                        .unwrap_or_default(),
                    before
                );
            });
        }
    }

    #[test]
    fn invalid_draft_blocks_save_and_requires_discard_confirmation() {
        preload_catalog();
        let dir = TestConfigDir::new("nyaterm-highlight-invalid-close");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "local");
        let editor = editor_for(&mut cx, app.clone(), "local", "ERROR");
        editor.update(&mut cx, |editor, cx| {
            editor.edit_field(1, "[".into(), cx);
            editor.save(cx);
        });
        settle_session(&mut cx);
        editor.update(&mut cx, |editor, cx| {
            assert!(!editor.closed);
            assert!(!editor.saving);
            assert!(!editor.inputs[1].read(cx).is_disabled());
            assert!(!editor.request_close(cx));
            assert!(editor.confirm_close);
            editor.close(cx);
        });
        settle_session(&mut cx);
        app.read_with(&cx, |app, _| {
            assert_eq!(
                app.session_keyword_rules("local").unwrap()[0].patterns,
                vec!["ERROR"]
            )
        });
    }

    #[test]
    fn rapid_session_edits_publish_only_the_latest_valid_input() {
        preload_catalog();
        let dir = TestConfigDir::new("nyaterm-highlight-debounce");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "local");
        let editor = editor_for(&mut cx, app.clone(), "local", "ERROR");
        editor.update(&mut cx, |editor, cx| {
            editor.edit_field(1, "[".into(), cx);
            editor.edit_field(1, "WARN".into(), cx);
            assert!(editor.session_validation.has_pending());
            assert_eq!(editor.session_tasks.len(), 1);
        });
        app.read_with(&cx, |app, _| {
            assert_eq!(
                app.session_keyword_rules("local").unwrap()[0].patterns,
                vec!["ERROR"]
            )
        });
        settle_session(&mut cx);
        app.read_with(&cx, |app, _| {
            assert_eq!(
                app.session_keyword_rules("local").unwrap()[0].patterns,
                vec!["WARN"]
            )
        });
        editor.read_with(&cx, |editor, _| assert!(!editor.dirty()));
    }

    /// The list of an empty group resolves to a drop target without an anchor.
    #[test]
    fn empty_group_placeholder_is_a_drop_target() {
        preload_catalog();
        let dir = TestConfigDir::new("nyaterm-highlight-empty-drop");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "local");
        let editor = editor_for(&mut cx, app.clone(), "local", "");
        let palette = app.read_with(&cx, |app, _| app.theme_palette());
        let mut delegate = super::list::KeywordListDelegate::new(editor.downgrade(), palette);
        delegate.set_snapshot(
            Vec::new(),
            Vec::new(),
            std::collections::HashSet::new(),
            None,
            palette,
            true,
        );

        let ix = nyaterm_ui::NyaIndexPath::new(0).section(super::list::SESSION_SECTION);
        // Nothing is being dragged, so there is no pending drop.
        assert!(delegate.pending_drop(ix).is_none());

        // Once a drag hovers the placeholder, the group is the drop target.
        assert!(delegate.update_drop_target(ix, Some(super::list::DropPosition::After)));
        let (anchor, position) = delegate.pending_drop(ix).expect("drop target");
        assert!(anchor.is_none(), "an empty group has no anchor rule");
        assert_eq!(position, super::list::DropPosition::After);
        // The placeholder paints the insertion marker.
        assert_eq!(
            delegate.drop_position_for(ix),
            Some(super::list::DropPosition::After)
        );
    }

    /// Reordering inside a group is part of the draft, even when no rule
    /// content changed.
    #[test]
    fn reordering_global_rules_marks_the_draft_dirty() {
        preload_catalog();
        let dir = TestConfigDir::new("nyaterm-highlight-reorder");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "local");
        let mut first = new_popup_rule();
        first.id = "global-a".into();
        let mut second = new_popup_rule();
        second.id = "global-b".into();
        app.update(&mut cx, |app, _| {
            app.settings
                .replace_keyword_highlight_rules(vec![first.clone(), second.clone()])
        });
        let editor = editor_for(&mut cx, app, "local", "");

        editor.update(&mut cx, |editor, cx| {
            editor.select_rule("global-a".into(), cx);
            editor.move_selected(1, cx);
        });

        editor.read_with(&cx, |editor, _| {
            assert_eq!(editor.global[0].id, "global-b");
            assert!(editor.dirty());
        });
    }

    #[test]
    fn regression_rule_merge_preserves_concurrent_additions_and_rejects_conflicting_edits() {
        preload_catalog();
        let mut a = new_popup_rule();
        a.name = "base".into();
        let mut edited = a.clone();
        edited.name = "draft".into();
        let added = new_popup_rule();
        let merged =
            merge_rules(&[a.clone()], &[edited.clone()], &[a.clone(), added.clone()]).unwrap();
        assert_eq!(merged, vec![edited.clone(), added]);
        let mut concurrent = a.clone();
        concurrent.name = "external".into();
        assert!(merge_rules(&[a.clone()], &[edited], &[concurrent]).is_err());
        assert!(merge_rules(&[a.clone()], &[], &[]).unwrap().is_empty());
        assert!(merge_rules(&[], std::slice::from_ref(&a), std::slice::from_ref(&a)).is_ok());
    }

    #[test]
    fn order_changes_are_detected_only_for_shared_rules() {
        preload_catalog();
        let mut a = new_popup_rule();
        a.id = "a".into();
        let mut b = new_popup_rule();
        b.id = "b".into();
        assert!(!order_changed(
            &[a.clone(), b.clone()],
            &[a.clone(), b.clone()]
        ));
        assert!(order_changed(
            &[a.clone(), b.clone()],
            &[b.clone(), a.clone()]
        ));
        // A rule that only exists in the draft does not change the shared order.
        let mut added = new_popup_rule();
        added.id = "c".into();
        assert!(!order_changed(
            &[a.clone(), b.clone()],
            &[a.clone(), b.clone(), added]
        ));
    }
}

#[cfg(test)]
mod native_probe {
    use super::KeywordHighlightWindow;
    use crate::entities::{OverlayStore, StartupRestoreStore, UiStoreHandles};
    use crate::features::NyaTermApp;
    use crate::features::view_widgets::ChildWindowSpec;
    use crate::models::SessionLaunchConfig;
    use crate::test_support::TestConfigDir;
    use gpui::{App, AppContext as _, Bounds, Entity, WindowBounds, WindowOptions, px, size};
    use nyaterm_core::{AppRuntime, RuntimeMode};
    use nyaterm_transport::LocalSessionConfig;
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::time::Duration;

    /// Render the keyword highlight editor into PNGs so the two rule groups,
    /// the per-rule draft marker and the footer can be inspected.
    #[test]
    #[ignore = "native popup visual fixture; run alone with NYATERM_KEYWORD_PROBE_OUTPUT"]
    fn keyword_highlight_popup_native_probe() {
        crate::preload_i18n().unwrap();
        let output = std::env::var("NYATERM_KEYWORD_PROBE_OUTPUT").unwrap();
        // The catalog language can be chosen to check how the footer hint
        // wraps in a wider language.
        let locale =
            std::env::var("NYATERM_KEYWORD_PROBE_LOCALE").unwrap_or_else(|_| "en".to_string());
        let root = TestConfigDir::new("nyaterm-keyword-popup-probe");
        let path = root.path().to_path_buf();
        gpui_platform::application()
            // The probe renders with the very asset source the application
            // uses. gpui-kit's complete catalog would hide an icon that the
            // app cannot resolve at runtime, and the button would only look
            // blank in the real shell.
            .with_assets(crate::test_support::ProbeAssets::new())
            .run(move |cx: &mut App| {
                let runtime = AppRuntime::from_parts_for_test(
                    RuntimeMode::Portable,
                    path.clone(),
                    path.join("config"),
                    path.join("logs"),
                    path.join("cache"),
                    None,
                );
                let stores = UiStoreHandles {
                    startup_restore: cx.new(|_| StartupRestoreStore::default()),
                    overlays: cx.new(|_| OverlayStore::default()),
                };
                let app = cx.new(|cx| NyaTermApp::new(runtime, stores, cx));
                app.update(cx, |app, cx| {
                    app.session.register_session_metadata(
                        "local",
                        crate::models::SessionRuntimeMetadata {
                            confirmed_ssh_runtime_mode: None,
                            pending_ssh_startup_command: None,
                            ssh_config: None,
                            ssh_multiplex_key: None,
                            source_connection_id: None,
                            ai_execution_profile: nyaterm_core::AiExecutionProfile::Posix,
                            launch_config: SessionLaunchConfig::Local(LocalSessionConfig {
                                name: "Local session".into(),
                                ..LocalSessionConfig::default()
                            }),
                            disconnected: false,
                        },
                    );
                    app.session.select_active_session("local");
                    app.terminal
                        .seed_session_view("local".to_string(), String::new(), "UTF-8");
                    app.shell.show_workspace();

                    let mut session_rule = super::new_popup_rule();
                    session_rule.id = "kh-session-1".into();
                    session_rule.name = "Errors".into();
                    session_rule.patterns = vec!["ERROR".into()];
                    let mut second = super::new_popup_rule();
                    second.id = "kh-session-2".into();
                    second.name = "Warnings".into();
                    second.patterns = vec!["WARN".into()];
                    second.enabled = false;
                    app.set_session_keyword_rules(
                        "local",
                        vec![session_rule.clone(), second.clone()],
                        cx,
                    );

                    let mut global = super::new_popup_rule();
                    global.id = "kh-global-1".into();
                    global.name = "Build targets".into();
                    global.patterns = vec!["cargo build".into()];
                    let mut global_second = super::new_popup_rule();
                    global_second.id = "kh-global-2".into();
                    global_second.name = "Git branches".into();
                    global_second.patterns = vec!["feature/".into()];
                    app.settings
                        .replace_keyword_highlight_rules(vec![global.clone(), global_second]);
                    app.sync_component_theme(cx);
                    // The app applies the language of its settings, so the probe
                    // switches the catalog afterwards.
                    nyaterm_i18n::apply_locale(&locale);
                });

                let editor_slot: Rc<RefCell<Option<Entity<KeywordHighlightWindow>>>> =
                    Rc::new(RefCell::new(None));
                cx.spawn(async move |cx| {
                    let slot = editor_slot.clone();
                    let window = cx.update(|cx| {
                        let app = app.clone();
                        cx.open_window(
                            WindowOptions {
                                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                                    None,
                                    size(px(880.), px(600.)),
                                    cx,
                                ))),
                                show: true,
                                focus: false,
                                ..Default::default()
                            },
                            move |window, cx| {
                                let chrome = ChildWindowSpec::modal_editor(
                                    "Keyword Highlight".into(),
                                    880.,
                                    600.,
                                )
                                .chrome();
                                let editor = cx.new(|cx| {
                                    KeywordHighlightWindow::new(
                                        app.clone(),
                                        "local".into(),
                                        String::new(),
                                        chrome,
                                        Some(window),
                                        cx,
                                    )
                                });
                                *slot.borrow_mut() = Some(editor.clone());
                                cx.new(|cx| nyaterm_ui::nya_root(editor, window, cx))
                            },
                        )
                        .unwrap()
                    });
                    cx.background_executor()
                        .timer(Duration::from_millis(400))
                        .await;

                    // The session group is selected first, so the footer is hidden.
                    let image = cx
                        .update_window(window.into(), |_, window, _| {
                            window.render_to_image().unwrap()
                        })
                        .unwrap();
                    let file = std::path::Path::new(&output).join("keyword-session-group.png");
                    cx.background_executor()
                        .spawn(async move {
                            image.save(file).unwrap();
                        })
                        .await;

                    // Selecting a global rule shows the draft marker and the footer.
                    let editor = editor_slot.borrow().clone();
                    if let Some(editor) = editor {
                        editor.update(cx, |editor, cx| {
                            editor.select_rule("kh-global-1".into(), cx);
                            editor.edit_field(0, "Build targets (edited)".into(), cx);
                        });
                    }
                    cx.background_executor()
                        .timer(Duration::from_millis(400))
                        .await;
                    let image = cx
                        .update_window(window.into(), |_, window, _| {
                            window.render_to_image().unwrap()
                        })
                        .unwrap();
                    let file = std::path::Path::new(&output).join("keyword-global-group.png");
                    cx.background_executor()
                        .spawn(async move {
                            image.save(file).unwrap();
                        })
                        .await;

                    // An empty session group has to accept a dragged rule, so its
                    // placeholder marks the insertion point.
                    let editor = editor_slot.borrow().clone();
                    if let Some(editor) = editor {
                        editor.update(cx, |editor, cx| {
                            editor.clear_session_rules(cx);
                        });
                    }
                    cx.background_executor()
                        .timer(Duration::from_millis(400))
                        .await;
                    let editor = editor_slot.borrow().clone();
                    if let Some(editor) = editor {
                        editor.update(cx, |editor, cx| {
                            let Some(list) = editor.list.as_ref() else {
                                return;
                            };
                            list.update(cx, |list, cx| {
                                list.delegate_mut().update_drop_target(
                                    nyaterm_ui::NyaIndexPath::new(0)
                                        .section(super::list::SESSION_SECTION),
                                    Some(super::list::DropPosition::After),
                                );
                                cx.notify();
                            });
                        });
                    }
                    cx.background_executor()
                        .timer(Duration::from_millis(400))
                        .await;
                    let image = cx
                        .update_window(window.into(), |_, window, _| {
                            window.render_to_image().unwrap()
                        })
                        .unwrap();
                    let file = std::path::Path::new(&output).join("keyword-empty-session-drop.png");
                    cx.background_executor()
                        .spawn(async move {
                            image.save(file).unwrap();
                        })
                        .await;

                    cx.update(|cx| cx.quit());
                })
                .detach();
            });
        drop(root);
    }
}
