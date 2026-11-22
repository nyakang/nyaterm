//! Session-scoped editor. Drafts and input widgets belong to the child entity;
//! the application only owns its lifetime and applies validated changes.
mod draft;

use self::draft::{RuleDraftIndex, RuleSummary};
use crate::features::NyaTermApp;
use crate::features::view_widgets::{
    ChildWindowChrome, ChildWindowCloseHandler, ChildWindowSpec, child_window_header,
    child_window_options, child_window_root, focus_child_window_shell_if_idle,
};
use crate::theme::ThemePalette;
use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, IntoElement, Render,
    ScrollHandle, Subscription, UniformListScrollHandle, Window, div, prelude::*, px, rgb,
    uniform_list,
};
use nyaterm_core::KeywordHighlightRule;
use nyaterm_ui::{
    ChildWindowSlot, NyaButton, NyaButtonVariant, NyaInputEvent, NyaInputShell, NyaInputState,
    NyaScrollable, NyaSwitch, NyaWindowHandle, activate_child_window, nya_root,
};
use rust_i18n::t;
use std::{collections::HashMap, rc::Rc};

#[derive(Default)]
pub(super) struct KeywordHighlightPopupState {
    window: ChildWindowSlot,
    editor: Option<Entity<KeywordHighlightWindow>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RuleScope {
    Session,
    Global,
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
    let mut merged = Vec::with_capacity(current.len() + draft.len());
    for rule in current {
        match changes.get(rule.id.as_str()) {
            Some(Some(replacement)) => merged.push((*replacement).clone()),
            Some(None) => {}
            None => merged.push(rule.clone()),
        }
    }
    for rule in draft {
        if !now.contains_key(rule.id.as_str()) && changes.contains_key(rule.id.as_str()) {
            merged.push(rule.clone());
        }
    }
    Ok(merged)
}

impl NyaTermApp {
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
            KeywordHighlightWindow::new(view_app.clone(), session_id, prefill, chrome, cx)
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

struct KeywordHighlightWindow {
    app: Entity<NyaTermApp>,
    session_id: String,
    base_global: Vec<KeywordHighlightRule>,
    base_session: Vec<KeywordHighlightRule>,
    global: Vec<KeywordHighlightRule>,
    session: Vec<KeywordHighlightRule>,
    scope: RuleScope,
    selected: [Option<String>; 2],
    indexes: [RuleDraftIndex; 2],
    summaries: Vec<RuleSummary>,
    summary_positions: HashMap<String, usize>,
    visible: Vec<usize>,
    search_text: String,
    search: Entity<NyaInputState>,
    inputs: Vec<Entity<NyaInputState>>,
    scroll: [UniformListScrollHandle; 2],
    detail_scroll: ScrollHandle,
    focus: FocusHandle,
    list_focus: FocusHandle,
    chrome: ChildWindowChrome,
    palette: ThemePalette,
    session_alive: bool,
    appearance: (String, u16),
    session_name: String,
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
    fn new(
        app: Entity<NyaTermApp>,
        session_id: String,
        prefill: String,
        chrome: ChildWindowChrome,
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
        let session_name = root
            .session
            .display_name(&session_id)
            .unwrap_or_else(|| session_id.clone());
        let search = cx.new(|cx| {
            NyaInputState::new(cx, String::new()).placeholder(t!("terminalCtx.ruleSearch"))
        });
        let inputs = (0..4)
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
        let mut subscriptions = vec![cx.subscribe(&search, |this, _, event, cx| {
            if let NyaInputEvent::Changed(text) = event {
                this.search_text = text.to_lowercase();
                this.filter_list();
                cx.notify();
            }
        })];
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
            let session_name = root
                .session
                .display_name(&this.session_id)
                .unwrap_or_else(|| this.session_id.clone());
            if this.palette != palette
                || this.session_alive != alive
                || this.appearance != appearance
                || this.session_name != session_name
            {
                this.palette = palette;
                this.session_alive = alive;
                this.appearance = appearance;
                this.session_name = session_name;
                cx.notify();
            }
        }));
        let mut this = Self {
            app,
            session_id,
            base_global: global.clone(),
            base_session: session.clone(),
            global,
            session,
            scope: RuleScope::Session,
            selected: [None, None],
            indexes: Default::default(),
            summaries: Vec::new(),
            summary_positions: HashMap::new(),
            visible: Vec::new(),
            search_text: String::new(),
            search,
            inputs,
            scroll: [
                UniformListScrollHandle::new(),
                UniformListScrollHandle::new(),
            ],
            detail_scroll: ScrollHandle::new(),
            focus: cx.focus_handle(),
            list_focus: cx.focus_handle(),
            chrome,
            palette,
            session_alive,
            appearance,
            session_name,
            detail: false,
            error: None,
            confirm_close: false,
            saving: false,
            validation_generation: 0,
            focus_pattern: false,
            closed: false,
            _subscriptions: subscriptions,
        };
        if let Some(rule) = prefill_rule_from_text(prefill) {
            this.selected[0] = Some(rule.id.clone());
            this.session.push(rule);
            this.detail = true;
        }
        this.refresh_list();
        this.seed_inputs(cx);
        this
    }
    fn index(&self) -> usize {
        if self.scope == RuleScope::Session {
            0
        } else {
            1
        }
    }
    fn dirty(&self) -> bool {
        self.indexes.iter().any(RuleDraftIndex::dirty)
    }
    fn filter_list(&mut self) {
        self.visible = self
            .summaries
            .iter()
            .enumerate()
            .filter(|(_, rule)| rule.search.contains(&self.search_text))
            .map(|(index, _)| index)
            .collect();
    }
    fn refresh_list(&mut self) {
        self.indexes[0].rebuild(&self.base_session, &self.session);
        self.indexes[1].rebuild(&self.base_global, &self.global);
        self.summaries.clear();
        for global in &self.global {
            let (rule, source) = if self.scope == RuleScope::Global {
                (global, "terminalCtx.globalRules")
            } else if let Some(rule) = self.indexes[0].get(&self.session, &global.id) {
                (rule, "terminalCtx.ruleOverride")
            } else {
                (global, "terminalCtx.ruleInherited")
            };
            self.summaries.push(RuleSummary::new(rule, source));
        }
        if self.scope == RuleScope::Session {
            for rule in &self.session {
                if !self.indexes[1].contains(&rule.id) {
                    self.summaries
                        .push(RuleSummary::new(rule, "terminalCtx.sessionRules"));
                }
            }
        }
        self.summary_positions = self
            .summaries
            .iter()
            .enumerate()
            .map(|(i, r)| (r.id.clone(), i))
            .collect();
        self.filter_list();
    }
    fn refresh_rule(&mut self, id: &str, summary_changed: bool) {
        if self.scope == RuleScope::Session {
            self.indexes[0].update(&self.base_session, &self.session, id);
        } else {
            self.indexes[1].update(&self.base_global, &self.global, id);
        }
        if summary_changed && let Some(&index) = self.summary_positions.get(id) {
            let source = self.summaries[index].source;
            if let Some(rule) = self.selected_rule() {
                self.summaries[index] = RuleSummary::new(rule, source);
            }
            self.filter_list();
        }
    }
    fn selected_rule(&self) -> Option<&KeywordHighlightRule> {
        let id = self.selected[self.index()].as_ref()?;
        if self.scope == RuleScope::Session
            && let Some(rule) = self.indexes[0].get(&self.session, id)
        {
            return Some(rule);
        }
        self.indexes[1].get(&self.global, id)
    }
    fn inherited(&self) -> bool {
        self.scope == RuleScope::Session
            && self
                .selected_rule()
                .is_some_and(|r| !self.indexes[0].contains(&r.id))
    }
    fn seed_inputs(&mut self, cx: &mut Context<Self>) {
        let rule = self
            .selected_rule()
            .cloned()
            .unwrap_or_else(empty_popup_rule);
        let values = [
            rule.name,
            rule.patterns.join("\n"),
            rule.color_dark,
            rule.color_light,
        ];
        let disabled = self.selected_rule().is_none() || self.inherited() || self.saving;
        for (input, value) in self.inputs.iter().zip(values) {
            input.update(cx, |state, cx| {
                state.set_content_silent(&value, cx);
                state.set_disabled(disabled, cx);
            });
        }
    }
    fn edit_field(&mut self, field: usize, text: String, cx: &mut Context<Self>) {
        if self.saving || self.inherited() {
            return;
        }
        let Some(id) = self.selected[self.index()].clone() else {
            return;
        };
        let Some(position) = self.indexes[self.index()].position(&id) else {
            return;
        };
        let rules = if self.scope == RuleScope::Session {
            &mut self.session
        } else {
            &mut self.global
        };
        let rule = &mut rules[position];
        match field {
            0 => rule.name = text,
            1 => rule.patterns = text.lines().map(str::to_string).collect(),
            2 => rule.color_dark = text,
            _ => rule.color_light = text,
        }
        self.error = None;
        self.refresh_rule(&id, field == 0);
        cx.notify();
    }
    fn select(&mut self, id: String, cx: &mut Context<Self>) {
        let index = self.index();
        self.selected[index] = Some(id);
        self.detail = true;
        self.seed_inputs(cx);
        cx.notify();
    }
    fn change_scope(&mut self, scope: RuleScope, cx: &mut Context<Self>) {
        self.scope = scope;
        self.detail = false;
        self.refresh_list();
        self.seed_inputs(cx);
        cx.notify();
    }
    fn override_rule(&mut self, cx: &mut Context<Self>) {
        if self.inherited()
            && let Some(rule) = self.selected_rule().cloned()
        {
            self.session.push(rule);
        }
        self.refresh_list();
        self.seed_inputs(cx);
        cx.notify();
    }
    fn remove_rule(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.selected[self.index()].clone() else {
            return;
        };
        if self.scope == RuleScope::Session {
            self.session.retain(|r| r.id != id);
        } else {
            self.global.retain(|r| r.id != id);
        }
        self.refresh_list();
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
        self.base_global = self.global.clone();
        self.base_session = self.session.clone();
        self.error = None;
        self.refresh_list();
        self.seed_inputs(cx);
        cx.notify();
    }
    fn request_close(&mut self, cx: &mut Context<Self>) -> bool {
        if self.saving {
            return false;
        }
        if self.dirty() {
            self.confirm_close = true;
            cx.notify();
            return false;
        }
        self.close(cx);
        true
    }
    fn close(&mut self, cx: &mut Context<Self>) {
        self.closed = true;
        self.app.update(cx, |app, cx| {
            app.terminal.keyword_popup = KeywordHighlightPopupState::default();
            cx.notify();
        });
        cx.notify();
    }
    fn save(&mut self, cx: &mut Context<Self>) {
        if self.saving || self.closed {
            return;
        }
        let mut pending = Vec::new();
        for (scope, base, rules) in [
            (RuleScope::Session, &self.base_session, &self.session),
            (RuleScope::Global, &self.base_global, &self.global),
        ] {
            let before: HashMap<_, _> = base.iter().map(|rule| (rule.id.as_str(), rule)).collect();
            for rule in rules {
                if before
                    .get(rule.id.as_str())
                    .is_none_or(|old| old.patterns != rule.patterns)
                {
                    pending.push((
                        scope,
                        rule.id.clone(),
                        rule.name.clone(),
                        rule.patterns.clone(),
                    ));
                }
            }
        }
        self.saving = true;
        self.validation_generation = self.validation_generation.wrapping_add(1);
        let generation = self.validation_generation;
        self.seed_inputs(cx);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let error = cx.background_spawn(async move {
                pending.into_iter().find_map(|(scope, id, name, patterns)| {
                    nyaterm_terminal_gpui::keywords::validation::validate_keyword_highlight_patterns(&patterns)
                        .into_iter().next().map(|error| (scope, id, name, error))
                })
            }).await;
            let _ = this.update(cx, |this, cx| {
                if this.closed || this.validation_generation != generation { return; }
                this.saving = false;
                if let Some((scope, id, name, error)) = error {
                    this.scope = scope;
                    this.selected[this.index()] = Some(id);
                    this.detail = true;
                    this.focus_pattern = true;
                    this.search_text.clear();
                    this.search.update(cx, |input, cx| input.set_content_silent("", cx));
                    this.refresh_list();
                    this.error = Some(t!("terminalCtx.invalidRulePattern", name = name, line = error.line, error = error.message).to_string());
                    this.seed_inputs(cx);
                    cx.notify();
                } else {
                    this.apply_validated_save(cx);
                    if !this.saving { this.seed_inputs(cx); }
                }
            });
        }).detach();
    }

    fn apply_validated_save(&mut self, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        for rule in self.global.iter_mut().chain(self.session.iter_mut()) {
            for color in [&mut rule.color_dark, &mut rule.color_light] {
                if parse_popup_swatch(color.trim()).is_none() {
                    self.error = Some(t!("terminalCtx.invalidRuleColor").to_string());
                    cx.notify();
                    return;
                }
                *color = normalize_popup_color(color);
            }
        }
        self.refresh_list();
        let root = self.app.read(cx);
        let Some(view) = root.terminal.view.views.get(&self.session_id) else {
            self.error = Some(t!("terminalCtx.ruleSessionClosed").to_string());
            cx.notify();
            return;
        };
        let merged = merge_rules(
            &self.base_global,
            &self.global,
            &root.settings.keyword_config().rules,
        )
        .and_then(|global| {
            merge_rules(
                &self.base_session,
                &self.session,
                view.keyword_highlights.rules(),
            )
            .map(|session| (global, session))
        });
        let (global, session) = match merged {
            Ok(value) => value,
            Err(error) => {
                self.error = Some(error);
                cx.notify();
                return;
            }
        };
        let mut config = root.settings.keyword_config().clone();
        let submission_base = config.clone();
        let session_base = view.keyword_highlights.rules().to_vec();
        let changed = config.rules != global;
        config.rules = global;
        let owner = self.session_id.clone();
        let weak = cx.entity().downgrade();
        self.saving = true;
        self.seed_inputs(cx);
        cx.notify();
        if !changed {
            self.app.update(cx, |app, cx| {
                app.set_session_keyword_rules(&owner, session, cx);
            });
            self.close(cx);
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
                                    editor.refresh_list();
                                    editor.error = Some(t!("terminalCtx.ruleConflict").to_string());
                                    editor.seed_inputs(cx);
                                    cx.notify();
                                });
                                return;
                            }
                            app.settings
                                .replace_keyword_highlight_rules(config.rules.clone());
                            app.settings.finish_keyword_persistence(&config);
                            let session_unchanged =
                                app.terminal.view.views.get(&owner).is_some_and(|view| {
                                    view.keyword_highlights.rules() == session_base
                                });
                            if !session_unchanged {
                                let _ = weak.update(cx, |editor, cx| {
                                    editor.base_global = config.rules.clone();
                                    editor.global = config.rules;
                                    editor.saving = false;
                                    editor.refresh_list();
                                    editor.error = Some(t!("terminalCtx.ruleConflict").to_string());
                                    editor.seed_inputs(cx);
                                    cx.notify();
                                });
                                app.invalidate_paint_theme_caches();
                                app.refresh_visible_terminal_surfaces(cx);
                                cx.notify();
                                return;
                            }
                            app.invalidate_paint_theme_caches();
                            app.refresh_visible_terminal_surfaces(cx);
                            app.set_session_keyword_rules(&owner, session, cx);
                            app.terminal.keyword_popup = KeywordHighlightPopupState::default();
                            let _ = weak.update(cx, |editor, cx| {
                                editor.closed = true;
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
    fn row(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let rule = &self.summaries[self.visible[index]];
        let id = rule.id.clone();
        let source = t!(rule.source);
        NyaButton::new(
            format!("keyword-row-{id}"),
            format!(
                "{} {} · {}",
                if rule.enabled { "●" } else { "○" },
                rule.name,
                source
            ),
        )
        .variant(NyaButtonVariant::Ghost)
        .full_width()
        .height(px(40.))
        .label_left()
        .tooltip(rule.name.clone())
        .selected(self.selected[self.index()].as_deref() == Some(id.as_str()))
        .disabled(self.saving)
        .on_click(cx.listener(move |this, _, window, cx| {
            this.select(id.clone(), cx);
            window.focus(&this.list_focus, cx);
        }))
        .into_any_element()
    }
    fn list(&self, cx: &mut Context<Self>) -> AnyElement {
        let scroll = self.scroll[self.index()].clone();
        let list = uniform_list(
            "keyword-rule-list",
            self.visible.len(),
            cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                range.map(|index| this.row(index, cx)).collect::<Vec<_>>()
            }),
        )
        .track_scroll(&scroll)
        .flex_1()
        .min_h_0()
        .w_full();
        div()
            .id("keyword-list-keyboard")
            .track_focus(&self.list_focus)
            .tab_index(0)
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if !this.list_focus.is_focused(window)
                    || this.saving
                    || this.visible.is_empty()
                    || event.keystroke.modifiers != gpui::Modifiers::default()
                {
                    return;
                }
                let selected = this.selected[this.index()].as_ref();
                let current = this
                    .visible
                    .iter()
                    .position(|&index| Some(&this.summaries[index].id) == selected)
                    .unwrap_or(0);
                let next = match event.keystroke.key.as_str() {
                    "up" => current.saturating_sub(1),
                    "down" => (current + 1).min(this.visible.len() - 1),
                    "home" => 0,
                    "end" => this.visible.len() - 1,
                    "enter" => {
                        this.detail = true;
                        cx.notify();
                        cx.stop_propagation();
                        return;
                    }
                    _ => return,
                };
                let id = this.summaries[this.visible[next]].id.clone();
                this.select(id, cx);
                if f32::from(window.viewport_size().width) < 760. {
                    this.detail = false;
                }
                this.scroll[this.index()].scroll_to_item(next, gpui::ScrollStrategy::Nearest);
                cx.stop_propagation();
            }))
            .flex()
            .flex_col()
            .gap_2()
            .h_full()
            .min_h_0()
            .child(NyaInputShell::new("keyword-search", &self.search))
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .child(list)
                    .vertical_scrollbar(&scroll),
            )
            .child(
                NyaButton::new("keyword-add", t!("common.add"))
                    .disabled(self.saving)
                    .on_click(cx.listener(|this, _, _, cx| {
                        let rule = new_popup_rule();
                        let id = rule.id.clone();
                        if this.scope == RuleScope::Session {
                            this.session.push(rule);
                        } else {
                            this.global.push(rule);
                        }
                        this.refresh_list();
                        this.select(id, cx);
                    })),
            )
            .into_any_element()
    }
    fn detail(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(rule) = self.selected_rule() else {
            return div()
                .child(t!("terminalCtx.keywordHighlightSelectHint"))
                .into_any_element();
        };
        let inherited = self.inherited();
        let labels = [
            t!("settings.keywordHighlightRuleName"),
            t!("settings.keywordHighlightRulePatterns"),
            t!("settings.keywordHighlightDarkPalette"),
            t!("settings.keywordHighlightLightPalette"),
        ];
        let mut form = div().id("keyword-details").flex().flex_col().gap_3().p_2();
        for (index, label) in labels.into_iter().enumerate() {
            let input = NyaInputShell::new(format!("keyword-field-{index}"), &self.inputs[index]);
            let input = if index == 1 {
                input.multi_line().height(px(120.))
            } else {
                input.height(px(32.))
            };
            let mut field = div().flex().flex_col().gap_1().child(label).child(input);
            if index >= 2 {
                field = field.child(
                    div().flex().flex_wrap().gap_1().children(
                        nyaterm_core::keyword_highlight_color_palette(index == 2)
                            .iter()
                            .enumerate()
                            .map(|(swatch, color)| {
                                let color = (*color).to_string();
                                NyaButton::new(
                                    format!("keyword-color-{index}-{swatch}"),
                                    color.clone(),
                                )
                                .compact()
                                .content(
                                    div()
                                        .size(px(18.))
                                        .rounded_sm()
                                        .bg(rgb(parse_popup_swatch(&color).unwrap_or(0))),
                                )
                                .tooltip(color.clone())
                                .disabled(self.saving || inherited)
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.edit_field(index, color.clone(), cx);
                                        this.inputs[index].update(cx, |input, cx| {
                                            input.set_content_silent(&color, cx)
                                        });
                                    },
                                ))
                            }),
                    ),
                );
            }
            form = form.child(field);
        }
        let enabled = rule.enabled;
        form = form.child(
            div()
                .flex()
                .gap_2()
                .items_center()
                .child(t!("dialog.enabled"))
                .child(
                    NyaSwitch::new("keyword-enabled")
                        .checked(enabled)
                        .disabled(self.saving || inherited)
                        .on_click(cx.listener(|this, checked: &bool, _, cx| {
                            let Some(id) = this.selected[this.index()].clone() else {
                                return;
                            };
                            let rules = if this.scope == RuleScope::Session {
                                &mut this.session
                            } else {
                                &mut this.global
                            };
                            if let Some(rule) = rules.iter_mut().find(|r| r.id == id) {
                                rule.enabled = *checked;
                            }
                            this.refresh_rule(&id, true);
                            cx.notify();
                        })),
                ),
        );
        if inherited {
            form = form.child(
                NyaButton::new("keyword-override", t!("terminalCtx.ruleCreateOverride"))
                    .disabled(self.saving)
                    .on_click(cx.listener(|this, _, _, cx| this.override_rule(cx))),
            );
        } else {
            let restore = self.scope == RuleScope::Session && self.indexes[1].contains(&rule.id);
            form = form.child(
                NyaButton::new(
                    "keyword-delete",
                    if restore {
                        t!("terminalCtx.ruleRestore")
                    } else {
                        t!("common.delete")
                    },
                )
                .disabled(self.saving)
                .on_click(cx.listener(|this, _, _, cx| this.remove_rule(cx))),
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
                    .child(self.list(cx)),
            );
        }
        if wide || self.detail {
            body = body.child(div().flex_1().min_w_0().min_h_0().child(self.detail(cx)));
        }
        let mut footer = div()
            .flex()
            .gap_2()
            .flex_wrap()
            .items_center()
            .flex_none()
            .child(
                NyaButton::new("keyword-save", t!("common.save"))
                    .disabled(self.saving || !self.dirty() || !self.session_alive)
                    .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
            )
            .child(
                NyaButton::new("keyword-cancel", t!("common.cancel"))
                    .disabled(self.saving)
                    .on_click(cx.listener(|this, _, window, cx| {
                        if this.request_close(cx) {
                            window.remove_window();
                        }
                    })),
            );
        if self.error.is_some() && !self.saving {
            footer = footer.child(
                NyaButton::new("keyword-reload", t!("terminalCtx.ruleReload"))
                    .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
            );
        }
        if self.confirm_close {
            footer = footer
                .child(
                    NyaButton::new("keyword-discard", t!("terminalCtx.ruleDiscard")).on_click(
                        cx.listener(|this, _, window, cx| {
                            this.close(cx);
                            window.remove_window();
                        }),
                    ),
                )
                .child(
                    NyaButton::new("keyword-continue", t!("terminalCtx.ruleContinue")).on_click(
                        cx.listener(|this, _, _, cx| {
                            this.confirm_close = false;
                            cx.notify();
                        }),
                    ),
                );
        }
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
                    .child(format!(
                        "{}{}",
                        self.session_name,
                        if self.dirty() { " *" } else { "" }
                    ))
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                NyaButton::new("keyword-session", t!("terminalCtx.sessionRules"))
                                    .selected(self.scope == RuleScope::Session)
                                    .disabled(self.saving)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.change_scope(RuleScope::Session, cx)
                                    })),
                            )
                            .child(
                                NyaButton::new("keyword-global", t!("terminalCtx.globalRules"))
                                    .selected(self.scope == RuleScope::Global)
                                    .disabled(self.saving)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.change_scope(RuleScope::Global, cx)
                                    })),
                            )
                            .when(!wide && self.detail, |d| {
                                d.child(
                                    NyaButton::new("keyword-back", t!("terminalCtx.ruleBack"))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.detail = false;
                                            cx.notify();
                                        })),
                                )
                            }),
                    )
                    .when(self.scope == RuleScope::Global, |d| {
                        d.child(t!("terminalCtx.ruleGlobalImpact"))
                    })
                    .when(!self.session_alive, |d| {
                        d.child(t!("terminalCtx.ruleSessionClosed"))
                    })
                    .when_some(self.error.clone(), |d, error| {
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
                    })
                    .child(body)
                    .child(footer),
            )
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
        enabled: true,
    }
}

fn empty_popup_rule() -> KeywordHighlightRule {
    KeywordHighlightRule {
        id: String::new(),
        name: String::new(),
        patterns: Vec::new(),
        color_dark: "#79c0ff".into(),
        color_light: "#0969da".into(),
        enabled: true,
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
        enabled: true,
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

#[cfg(test)]
mod tests {
    use super::{
        KeywordHighlightWindow, RuleScope, merge_rules, new_popup_rule, prefill_rule_from_text,
    };
    use crate::features::test_support::app_with_visible_local_session;
    use crate::features::view_widgets::ChildWindowSpec;
    use crate::test_support::{TestAppCleanup, TestConfigDir};
    use gpui::{AppContext as _, TestAppContext};

    #[test]
    fn reading_prefill_escapes_selected_text_as_literal_patterns() {
        for text in ["192.168.1.1", "[ERROR]", "[", r"C:\work\file", "中文(a+b)"] {
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
    fn reading_invalid_pattern_keeps_draft_and_identifies_rule_without_applying_it() {
        let dir = TestConfigDir::new("nyaterm-highlight-validation");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "local");
        let editor = cx.new(|cx| {
            KeywordHighlightWindow::new(
                app.clone(),
                "local".into(),
                "valid".into(),
                ChildWindowSpec::modal_editor("test".into(), 880., 600.).chrome(),
                cx,
            )
        });
        editor.update(&mut cx, |editor, cx| {
            editor.edit_field(1, "valid\n[".into(), cx);
            editor.save(cx);
            assert!(editor.saving);
        });
        cx.run_until_parked();
        editor.read_with(&cx, |editor, _| {
            assert!(!editor.saving);
            assert!(!editor.closed);
            assert!(editor.error.is_some());
            assert!(editor.dirty());
            assert_eq!(editor.selected_rule().unwrap().patterns, vec!["valid", "["]);
        });
        app.read_with(&cx, |app, _| {
            assert!(app.session_keyword_rules("local").unwrap().is_empty())
        });
    }

    #[test]
    fn reading_validation_preserves_concurrent_additions_and_unchanged_legacy_patterns() {
        let dir = TestConfigDir::new("nyaterm-highlight-validation-merge");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "local");
        let mut legacy = new_popup_rule();
        legacy.patterns = vec!["[".into()];
        app.update(&mut cx, |app, _| {
            app.settings
                .replace_keyword_highlight_rules(vec![legacy.clone()])
        });
        let editor = cx.new(|cx| {
            KeywordHighlightWindow::new(
                app.clone(),
                "local".into(),
                "selected".into(),
                ChildWindowSpec::modal_editor("test".into(), 880., 600.).chrome(),
                cx,
            )
        });
        editor.update(&mut cx, |editor, cx| editor.save(cx));
        let mut concurrent = new_popup_rule();
        concurrent.patterns = vec!["concurrent".into()];
        app.update(&mut cx, |app, cx| {
            app.set_session_keyword_rules("local", vec![concurrent.clone()], cx)
        });
        cx.run_until_parked();
        editor.read_with(&cx, |editor, _| assert!(editor.closed));
        app.read_with(&cx, |app, _| {
            assert_eq!(app.settings.keyword_config().rules, vec![legacy]);
            let rules = app.session_keyword_rules("local").unwrap();
            assert_eq!(rules.len(), 2);
            assert_eq!(rules[0], concurrent);
            assert_eq!(rules[1].patterns, vec!["selected"]);
        });
    }

    #[test]
    fn regression_rule_merge_preserves_concurrent_additions_and_rejects_conflicting_edits() {
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
    fn regression_popup_inputs_keep_progressive_color_and_create_explicit_override() {
        let dir = TestConfigDir::new("nyaterm-highlight-editor");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "local");
        let mut global = new_popup_rule();
        global.name = "global".into();
        app.update(&mut cx, |app, _| {
            app.settings
                .replace_keyword_highlight_rules(vec![global.clone()])
        });
        let editor = cx.new(|cx| {
            KeywordHighlightWindow::new(
                app,
                "local".into(),
                String::new(),
                ChildWindowSpec::modal_editor("test".into(), 880., 600.).chrome(),
                cx,
            )
        });
        editor.update(&mut cx, |editor, cx| {
            editor.select(global.id.clone(), cx);
            assert!(editor.inherited());
            editor.edit_field(0, "forbidden".into(), cx);
            assert!(!editor.dirty());
            editor.override_rule(cx);
            editor.edit_field(2, "#a".into(), cx);
            assert_eq!(editor.session[0].color_dark, "#a");
            assert_eq!(editor.global[0].color_dark, global.color_dark);
            editor.remove_rule(cx);
            assert!(editor.inherited());
            assert!(!editor.dirty());
            editor.change_scope(RuleScope::Global, cx);
            editor.select(global.id.clone(), cx);
            editor.edit_field(0, "changed".into(), cx);
            assert!(!editor.request_close(cx));
            assert!(editor.confirm_close);
        });
    }

    #[test]
    fn regression_prefill_discards_empty_lines() {
        let rule = prefill_rule_from_text("  ERROR \n\n warning ".into()).unwrap();
        assert_eq!(rule.patterns, vec!["ERROR", "warning"]);
        assert!(prefill_rule_from_text(" \n ".into()).is_none());
    }
}

#[cfg(test)]
mod native_probe {
    use super::{KeywordHighlightWindow, new_popup_rule};
    use crate::entities::{OverlayStore, StartupRestoreStore, UiStoreHandles};
    use crate::features::NyaTermApp;
    use crate::features::view_widgets::ChildWindowSpec;
    use crate::test_support::TestConfigDir;
    use gpui::{App, AppContext as _, Bounds, WindowBounds, WindowOptions, px, size};
    use nyaterm_core::{AppRuntime, RuntimeMode};
    use std::time::Duration;

    #[test]
    #[ignore = "native GPU UI fixture; run alone with NYATERM_EDITOR_PROBE_OUTPUT"]
    fn keyword_editor_native_probe() {
        crate::preload_i18n().unwrap();
        let output =
            std::env::var("NYATERM_EDITOR_PROBE_OUTPUT").expect("fixture output directory");
        let rule_count = std::env::var("NYATERM_EDITOR_PROBE_RULES")
            .ok()
            .map(|v| v.parse::<usize>().expect("rule count"))
            .unwrap_or(200);
        let invalid = std::env::var("NYATERM_EDITOR_PROBE_INVALID").is_ok();
        let root = TestConfigDir::new("nyaterm-keyword-ui-probe");
        let path = root.path().to_path_buf();
        gpui_platform::application().run(move |cx: &mut App| {
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
                app.terminal
                    .seed_session_view("synthetic".into(), String::new(), "UTF-8");
                app.ensure_terminal_surface("synthetic", cx);
                let rules = (0..rule_count)
                    .map(|index| {
                        let mut rule = new_popup_rule();
                        rule.name = format!("Synthetic rule {index:03}");
                        rule.patterns = vec!["ERROR".into(), "WARNING".into()];
                        rule
                    })
                    .collect();
                app.settings.replace_keyword_highlight_rules(rules);
                app.sync_component_theme(cx);
            });
            let app_task = app.clone();
            cx.spawn(async move |cx| {
                for (width, height, font_size) in [
                    (640., 460., 12),
                    (640., 460., 24),
                    (880., 600., 12),
                    (880., 600., 24),
                ] {
                    let (window, editor) =
                        cx.update(|cx| {
                            app_task.update(cx, |app, _| {
                                let mut summary = app.settings.summary().clone();
                                summary.ui_font_size = font_size;
                                app.settings.replace_summary(summary);
                            });
                            let editor = cx.new(|cx| {
                                KeywordHighlightWindow::new(
                                    app_task.clone(),
                                    "synthetic".into(),
                                    "ERROR\nWARNING".into(),
                                    ChildWindowSpec::modal_editor("Fixture".into(), width, height)
                                        .chrome(),
                                    cx,
                                )
                            });
                            let view = editor.clone();
                            if invalid {
                                editor.update(cx, |editor, cx| {
                                    editor.edit_field(1, "[".into(), cx);
                                    editor.save(cx);
                                });
                            }
                            let window = cx
                                .open_window(
                                    WindowOptions {
                                        window_bounds: Some(WindowBounds::Windowed(
                                            Bounds::centered(None, size(px(width), px(height)), cx),
                                        )),
                                        show: true,
                                        focus: false,
                                        ..Default::default()
                                    },
                                    move |window, cx| {
                                        cx.new(|cx| nyaterm_ui::nya_root(view, window, cx))
                                    },
                                )
                                .unwrap();
                            (window, editor)
                        });
                    cx.background_executor()
                        .timer(Duration::from_millis(500))
                        .await;
                    let image = cx
                        .update_window(window.into(), |_, window, _| {
                            window.render_to_image().unwrap()
                        })
                        .unwrap();
                    let file = std::path::Path::new(&output)
                        .join(format!("keyword-{width}x{height}-font{font_size}.png"));
                    cx.background_executor()
                        .spawn(async move {
                            image.save(file).unwrap();
                        })
                        .await;
                    cx.update_window(window.into(), |_, window, cx| {
                        window.dispatch_event(
                            gpui::PlatformInput::Scroll(gpui::ScrollEvent {
                                position: gpui::point(px(width - 50.), px(220.)),
                                delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.), px(-1200.))),
                                ..Default::default()
                            }),
                            cx,
                        );
                    })
                    .unwrap();
                    cx.background_executor()
                        .timer(Duration::from_millis(150))
                        .await;
                    let image = cx
                        .update_window(window.into(), |_, window, _| {
                            window.render_to_image().unwrap()
                        })
                        .unwrap();
                    let file = std::path::Path::new(&output).join(format!(
                        "keyword-{width}x{height}-font{font_size}-scrolled.png"
                    ));
                    cx.background_executor()
                        .spawn(async move {
                            image.save(file).unwrap();
                        })
                        .await;
                    cx.update_window(window.into(), |_, window, cx| {
                        editor.update(cx, |editor, cx| {
                            editor.detail = false;
                            window.focus(&editor.list_focus, cx);
                            cx.notify();
                        })
                    })
                    .unwrap();
                    cx.background_executor()
                        .timer(Duration::from_millis(150))
                        .await;
                    cx.update_window(window.into(), |_, window, cx| {
                        window.dispatch_event(
                            gpui::PlatformInput::KeyDown(gpui::KeyDownEvent {
                                keystroke: gpui::Keystroke::parse("end").unwrap(),
                                is_held: false,
                                prefer_character_input: false,
                            }),
                            cx,
                        );
                        editor.read_with(cx, |editor, _| {
                            assert_eq!(
                                editor.selected[editor.index()].as_deref(),
                                editor
                                    .visible
                                    .last()
                                    .map(|&index| editor.summaries[index].id.as_str())
                            )
                        });
                    })
                    .unwrap();
                    // Keep the list visible on narrow windows to inspect the final row.
                    editor.update(cx, |editor, cx| {
                        editor.detail = false;
                        cx.notify();
                    });
                    cx.background_executor()
                        .timer(Duration::from_millis(150))
                        .await;
                    let image = cx
                        .update_window(window.into(), |_, window, _| {
                            window.render_to_image().unwrap()
                        })
                        .unwrap();
                    let file = std::path::Path::new(&output).join(format!(
                        "keyword-{width}x{height}-font{font_size}-list-end.png"
                    ));
                    cx.background_executor()
                        .spawn(async move {
                            image.save(file).unwrap();
                        })
                        .await;
                    // Keep native windows alive until the complete matrix is captured;
                    // GPUI quits when its last window closes.
                }
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
        drop(root);
    }
}
