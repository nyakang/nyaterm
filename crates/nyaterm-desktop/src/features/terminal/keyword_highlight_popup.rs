//! Right-click "keyword highlight" popup: a two-pane editor that manages
//! session-scoped rules (stored on the terminal surface) and global rules
//! (persisted in settings) side by side.

use std::rc::Rc;

use gpui::{
    AnyElement, App, AppContext as _, ClickEvent, Context, Entity, FocusHandle, FontWeight,
    IntoElement, Render, SharedString, Subscription, Window, div, prelude::*, px, rgb, rgba, svg,
};
use nyaterm_core::KeywordHighlightRule;
use nyaterm_ui::{
    ChildWindowSlot, NyaButton, NyaButtonVariant, NyaInputShell, NyaSwitch, NyaTooltip,
    NyaWindowHandle, activate_child_window, nya_root,
};
use rust_i18n::t;

use crate::features::view_widgets::{
    ChildWindowChrome, ChildWindowCloseHandler, ChildWindowSpec, child_window_header,
    child_window_options, child_window_root, focus_child_window_shell_if_idle,
};
use crate::features::{NyaTermApp, text_inputs::TextInputSetup};
use crate::models::KeywordHighlightEditorField;
use crate::theme::ThemePalette;

/// Which layer the currently selected rule belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum KeywordHighlightRuleGroup {
    /// Only this session; dropped when the terminal surface goes away.
    #[default]
    Session,
    /// Persisted and shared by every session.
    Global,
}

/// Draft state for the keyword-highlight popup. Lives on `TerminalFeatureState`.
/// Rules are edited in this draft and only written to their layer on Save.
#[derive(Debug, Clone, Default)]
pub(super) struct KeywordHighlightPopupState {
    /// Session whose surface owns the session rule layer.
    pub session_id: String,
    /// The rule group currently selected in the left pane.
    pub group: KeywordHighlightRuleGroup,
    /// Id of the selected rule, if any.
    pub selected_id: Option<String>,
    /// Draft of the global rules (mirrors settings on open).
    pub global_rules: Vec<KeywordHighlightRule>,
    /// Draft of the session rules (mirrors the surface on open).
    pub session_rules: Vec<KeywordHighlightRule>,
    /// Owned native editor window; keeps a second open request from starting a rival draft.
    pub window: ChildWindowSlot,
}

pub(in crate::features) const POPUP_INPUT_PREFIX: &str = "keyword.highlight.popup.";

/// Stable input id for one draft field. Prefix is routed to the popup handler.
fn popup_input_id(field: KeywordHighlightEditorField) -> String {
    format!("{POPUP_INPUT_PREFIX}{}", field.input_key())
}

impl KeywordHighlightPopupState {
    /// Whether the popup draft is currently open.
    fn is_open(&self) -> bool {
        !self.session_id.is_empty()
    }

    /// Rules of the currently selected group (immutable).
    fn selected_rules(&self) -> &Vec<KeywordHighlightRule> {
        match self.group {
            KeywordHighlightRuleGroup::Session => &self.session_rules,
            KeywordHighlightRuleGroup::Global => &self.global_rules,
        }
    }

    /// Rules of the currently selected group (mutable).
    fn selected_rules_mut(&mut self) -> &mut Vec<KeywordHighlightRule> {
        match self.group {
            KeywordHighlightRuleGroup::Session => &mut self.session_rules,
            KeywordHighlightRuleGroup::Global => &mut self.global_rules,
        }
    }
}

impl NyaTermApp {
    /// Apply a change from one of the popup's draft inputs.
    pub(in crate::features) fn apply_keyword_highlight_popup_input(
        &mut self,
        field: KeywordHighlightEditorField,
        text: String,
        cx: &mut Context<Self>,
    ) {
        let is_color = matches!(
            field,
            KeywordHighlightEditorField::ColorDark | KeywordHighlightEditorField::ColorLight
        );
        let value = if is_color {
            normalize_popup_color(&text)
        } else {
            text
        };

        let selected_id = self.terminal.keyword_popup.selected_id.clone();
        let Some(rule) = self
            .terminal
            .keyword_popup
            .selected_rules_mut()
            .iter_mut()
            .find(|rule| Some(rule.id.as_str()) == selected_id.as_deref())
        else {
            return;
        };
        match field {
            KeywordHighlightEditorField::Name => rule.name = value.clone(),
            KeywordHighlightEditorField::Patterns => {
                rule.patterns = value.split('\n').map(ToOwned::to_owned).collect();
            }
            KeywordHighlightEditorField::ColorDark => rule.color_dark = value.clone(),
            KeywordHighlightEditorField::ColorLight => rule.color_light = value.clone(),
        }
        if is_color {
            self.reset_text_input(&popup_input_id(field), &value, cx);
        }
        cx.notify();
    }

    /// Raise the popup window if one is already open.
    fn activate_keyword_highlight_window(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(handle) = self.terminal.keyword_popup.window.handle() else {
            return false;
        };
        activate_child_window(
            &cx.entity(),
            handle,
            |app: &mut NyaTermApp| Some(&mut app.terminal.keyword_popup.window),
            cx,
        );
        true
    }

    /// Open the keyword-highlight popup for `session_id` in a native child
    /// window. If `prefill` is a non-empty terminal selection, a new session
    /// rule is prepared from it. A second request raises the existing window.
    pub(in crate::features) fn open_keyword_highlight_popup(
        &mut self,
        session_id: String,
        prefill: String,
        cx: &mut Context<Self>,
    ) {
        if self.activate_keyword_highlight_window(cx) {
            return;
        }
        if self.terminal.keyword_popup.window.is_pending() {
            return;
        }

        let global_rules = self.settings.keyword_config().rules.clone();
        let session_rules = self
            .terminal
            .view
            .surfaces
            .get(&session_id)
            .map(|surface| surface.read(cx).session_keyword_rules().to_vec())
            .unwrap_or_default();

        let mut state = KeywordHighlightPopupState {
            session_id,
            group: KeywordHighlightRuleGroup::Session,
            selected_id: None,
            global_rules,
            session_rules,
            window: ChildWindowSlot::default(),
        };
        let prefill_rule = prefill_rule_from_text(prefill);
        if let Some(rule) = prefill_rule {
            state.selected_id = Some(rule.id.clone());
            state.session_rules.push(rule);
        }
        // Preserve the window claim across the draft reset below.
        let mut slot = std::mem::take(&mut self.terminal.keyword_popup.window);
        if !slot.begin_open() {
            return;
        }
        state.window = slot;
        self.terminal.keyword_popup = state;
        self.forget_text_inputs(POPUP_INPUT_PREFIX);
        cx.notify();

        let app = cx.entity();
        cx.defer(move |cx| {
            if app.read(cx).terminal.keyword_popup.window.is_pending() {
                open_keyword_highlight_window_now_from_app(app, cx);
            }
        });
    }

    /// Clean up popup state after cancel or save. Resetting the draft also
    /// clears the window slot; the window view tears itself down on the next frame.
    fn close_keyword_highlight_popup(&mut self, cx: &mut Context<Self>) {
        self.terminal.keyword_popup = KeywordHighlightPopupState::default();
        self.forget_text_inputs(POPUP_INPUT_PREFIX);
        cx.notify();
    }

    /// Commit both draft rule lists to their layers, then close the window.
    fn commit_keyword_highlight_popup(&mut self, cx: &mut Context<Self>) {
        let global_rules = self.terminal.keyword_popup.global_rules.clone();
        let session_rules = self.terminal.keyword_popup.session_rules.clone();
        let session_id = self.terminal.keyword_popup.session_id.clone();

        // Global rules are persisted; only write when they actually changed.
        if self.settings.keyword_config().rules != global_rules {
            self.settings.replace_keyword_highlight_rules(global_rules);
            self.save_keyword_highlights(cx);
        }
        // Session rules live on the surface and are dropped when it closes.
        if let Some(surface) = self.terminal.view.surfaces.get(&session_id).cloned() {
            surface.update(cx, |surface, cx| {
                surface.set_session_keyword_rules(session_rules, cx);
            });
        }
        self.close_keyword_highlight_popup(cx);
    }

    /// Select a rule in the left pane.
    fn select_keyword_highlight_popup_rule(
        &mut self,
        group: KeywordHighlightRuleGroup,
        id: String,
        cx: &mut Context<Self>,
    ) {
        self.terminal.keyword_popup.group = group;
        self.terminal.keyword_popup.selected_id = Some(id);
        self.reset_popup_inputs(cx);
        cx.notify();
    }

    /// Create a new empty rule in the given group and select it.
    fn add_keyword_highlight_popup_rule(
        &mut self,
        group: KeywordHighlightRuleGroup,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let rule = new_popup_rule();
        let id = rule.id.clone();
        match group {
            KeywordHighlightRuleGroup::Session => {
                self.terminal.keyword_popup.session_rules.push(rule);
            }
            KeywordHighlightRuleGroup::Global => {
                self.terminal.keyword_popup.global_rules.push(rule);
            }
        }
        self.terminal.keyword_popup.group = group;
        self.terminal.keyword_popup.selected_id = Some(id);
        self.reset_popup_inputs(cx);
        self.focus_keyword_highlight_popup_field(KeywordHighlightEditorField::Name, window, cx);
        cx.notify();
    }

    /// Remove a rule from the given group's draft.
    fn remove_keyword_highlight_popup_rule(
        &mut self,
        group: KeywordHighlightRuleGroup,
        id: &str,
        cx: &mut Context<Self>,
    ) {
        let rules = match group {
            KeywordHighlightRuleGroup::Session => &mut self.terminal.keyword_popup.session_rules,
            KeywordHighlightRuleGroup::Global => &mut self.terminal.keyword_popup.global_rules,
        };
        rules.retain(|rule| rule.id != id);
        if self.terminal.keyword_popup.selected_id.as_deref() == Some(id) {
            self.terminal.keyword_popup.selected_id = None;
        }
        cx.notify();
    }

    /// Toggle a rule's enabled flag in the given group's draft.
    fn toggle_keyword_highlight_popup_rule(
        &mut self,
        group: KeywordHighlightRuleGroup,
        id: &str,
        cx: &mut Context<Self>,
    ) {
        let rules = match group {
            KeywordHighlightRuleGroup::Session => &mut self.terminal.keyword_popup.session_rules,
            KeywordHighlightRuleGroup::Global => &mut self.terminal.keyword_popup.global_rules,
        };
        if let Some(rule) = rules.iter_mut().find(|rule| rule.id == id) {
            rule.enabled = !rule.enabled;
        }
        cx.notify();
    }

    /// Reset all popup inputs to the selected rule's values.
    fn reset_popup_inputs(&mut self, cx: &mut Context<Self>) {
        let values = {
            let popup = &self.terminal.keyword_popup;
            popup
                .selected_id
                .as_ref()
                .and_then(|id| popup.selected_rules().iter().find(|r| &r.id == id))
                .map(|rule| {
                    (
                        rule.name.clone(),
                        rule.patterns.join("\n"),
                        rule.color_dark.clone(),
                        rule.color_light.clone(),
                    )
                })
        };
        let Some((name, patterns, dark, light)) = values else {
            return;
        };
        self.reset_text_input(
            &popup_input_id(KeywordHighlightEditorField::Name),
            &name,
            cx,
        );
        self.reset_text_input(
            &popup_input_id(KeywordHighlightEditorField::Patterns),
            &patterns,
            cx,
        );
        self.reset_text_input(
            &popup_input_id(KeywordHighlightEditorField::ColorDark),
            &dark,
            cx,
        );
        self.reset_text_input(
            &popup_input_id(KeywordHighlightEditorField::ColorLight),
            &light,
            cx,
        );
    }

    fn focus_keyword_highlight_popup_field(
        &mut self,
        field: KeywordHighlightEditorField,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_text_input_if_present(&popup_input_id(field), window, cx);
    }

    /// Render the two-pane popup content.
    pub(in crate::features) fn render_keyword_highlight_popup(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = self.theme_palette();
        // Clone the selected rule so the immutable borrow of popup ends before
        // the mutable text-input calls below.
        let selected_rule = {
            let popup = &self.terminal.keyword_popup;
            popup
                .selected_id
                .as_ref()
                .and_then(|id| popup.selected_rules().iter().find(|r| &r.id == id).cloned())
                .unwrap_or_else(empty_popup_rule)
        };
        let name = &selected_rule.name;
        let patterns = &selected_rule.patterns;
        let dark = &selected_rule.color_dark;
        let light = &selected_rule.color_light;

        // Ensure all four inputs exist before rendering them.
        self.text_input_box(
            popup_input_id(KeywordHighlightEditorField::Name),
            name.as_str(),
            TextInputSetup::placeholder(t!("settings.keywordHighlightRuleName")),
            cx,
        );
        self.text_input_box(
            popup_input_id(KeywordHighlightEditorField::Patterns),
            &patterns.join("\n"),
            TextInputSetup::multi_line(t!("settings.keywordHighlightRulePatterns")),
            cx,
        );
        self.text_input_box(
            popup_input_id(KeywordHighlightEditorField::ColorDark),
            dark.as_str(),
            TextInputSetup::placeholder(t!("settings.keywordHighlightDarkPalette")),
            cx,
        );
        self.text_input_box(
            popup_input_id(KeywordHighlightEditorField::ColorLight),
            light.as_str(),
            TextInputSetup::placeholder(t!("settings.keywordHighlightLightPalette")),
            cx,
        );

        let name_input = NyaInputShell::new(
            popup_input_id(KeywordHighlightEditorField::Name),
            self.existing_text_input(popup_input_id(KeywordHighlightEditorField::Name))
                .as_ref()
                .expect("popup name input"),
        )
        .into_any_element();
        let patterns_input = NyaInputShell::new(
            popup_input_id(KeywordHighlightEditorField::Patterns),
            self.existing_text_input(popup_input_id(KeywordHighlightEditorField::Patterns))
                .as_ref()
                .expect("popup patterns input"),
        )
        .multi_line()
        .into_any_element();
        let dark_input = NyaInputShell::new(
            popup_input_id(KeywordHighlightEditorField::ColorDark),
            self.existing_text_input(popup_input_id(KeywordHighlightEditorField::ColorDark))
                .as_ref()
                .expect("popup dark input"),
        )
        .into_any_element();
        let light_input = NyaInputShell::new(
            popup_input_id(KeywordHighlightEditorField::ColorLight),
            self.existing_text_input(popup_input_id(KeywordHighlightEditorField::ColorLight))
                .as_ref()
                .expect("popup light input"),
        )
        .into_any_element();

        let left = self.render_keyword_highlight_popup_list(palette, cx);
        let right = self.render_keyword_highlight_popup_editor(
            palette,
            name_input,
            patterns_input,
            dark_input,
            light_input,
            cx,
        );

        let cancel_label = t!("common.cancel").to_string();
        let save_label = t!("common.save").to_string();

        div()
            .size_full()
            .flex()
            .flex_col()
            // Two-pane editing area; keeps the original left-list / right-editor layout.
            .child(
                div()
                    .id("keyword-highlight-popup-body")
                    .flex_1()
                    .min_h_0()
                    .p_4()
                    .flex()
                    .gap_4()
                    .child(left)
                    .child(div().w(px(1.)).bg(rgb(palette.border)).flex_none())
                    .child(right),
            )
            // Footer action bar, matching the modal_editor child-window style.
            .child(
                div()
                    .h(px(64.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap_3()
                    .px_4()
                    .py_3()
                    .border_t_1()
                    .border_color(rgb(palette.border))
                    .bg(rgb(palette.section_header))
                    .child(
                        div().w(px(76.)).child(
                            NyaButton::new("keyword-highlight-popup-cancel", cancel_label)
                                .variant(NyaButtonVariant::Secondary)
                                .full_width()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.close_keyword_highlight_popup(cx);
                                })),
                        ),
                    )
                    .child(
                        div().w(px(76.)).child(
                            NyaButton::new("keyword-highlight-popup-save", save_label)
                                .variant(NyaButtonVariant::Primary)
                                .full_width()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.commit_keyword_highlight_popup(cx);
                                })),
                        ),
                    ),
            )
            .into_any_element()
    }

    fn render_keyword_highlight_popup_list(
        &self,
        palette: ThemePalette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected_id = self.terminal.keyword_popup.selected_id.clone();
        let group = self.terminal.keyword_popup.group;

        let session = self.render_popup_rule_group(
            palette,
            KeywordHighlightRuleGroup::Session,
            t!("terminalCtx.sessionRules").to_string(),
            &self.terminal.keyword_popup.session_rules,
            selected_id.as_deref(),
            group == KeywordHighlightRuleGroup::Session,
            cx,
        );
        let global = self.render_popup_rule_group(
            palette,
            KeywordHighlightRuleGroup::Global,
            t!("terminalCtx.globalRules").to_string(),
            &self.terminal.keyword_popup.global_rules,
            selected_id.as_deref(),
            group == KeywordHighlightRuleGroup::Global,
            cx,
        );

        div()
            .w(px(300.))
            .flex_none()
            .flex()
            .flex_col()
            .gap_3()
            .overflow_hidden()
            .child(session)
            .child(global)
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn render_popup_rule_group(
        &self,
        palette: ThemePalette,
        group: KeywordHighlightRuleGroup,
        title: String,
        rules: &[KeywordHighlightRule],
        selected_id: Option<&str>,
        is_active_group: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let add_group = group;
        let header = div()
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .child(
                div()
                    .text_size(px(12.))
                    .font_weight(FontWeight(600.))
                    .text_color(rgb(palette.text))
                    .child(title),
            )
            .child(popup_icon_button(
                palette,
                format!("keyword-popup-add-{group:?}"),
                "icons/conn/add.svg",
                t!("common.add"),
                true,
                cx.listener(move |this, _, window, cx| {
                    this.add_keyword_highlight_popup_rule(add_group, window, cx);
                }),
            ));

        let mut rows = Vec::new();
        for rule in rules {
            let rule_id = rule.id.clone();
            let select_id = rule.id.clone();
            let toggle_id = rule.id.clone();
            let remove_id = rule.id.clone();
            let selected = selected_id == Some(rule.id.as_str()) && is_active_group;
            let swatch = parse_popup_swatch(&rule.color_dark).unwrap_or(0x79c0ff);
            let count = rule
                .patterns
                .iter()
                .filter(|p| !p.trim().is_empty())
                .count();
            let select_group = group;
            let toggle_group = group;
            let remove_group = group;
            rows.push(
                div()
                    .id(SharedString::from(format!(
                        "keyword-popup-rule-{}",
                        rule.id
                    )))
                    .rounded_md()
                    .border_1()
                    .border_color(if selected {
                        rgb(palette.accent)
                    } else {
                        rgb(palette.border)
                    })
                    .bg(rgb(if selected {
                        palette.hover
                    } else {
                        palette.input
                    }))
                    .px_2()
                    .py_2()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select_keyword_highlight_popup_rule(
                            select_group,
                            select_id.clone(),
                            cx,
                        );
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .size(px(10.))
                                    .rounded_full()
                                    .bg(rgb(swatch))
                                    .border_1()
                                    .border_color(rgb(palette.border))
                                    .flex_none(),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .text_size(px(12.))
                                    .text_color(rgb(palette.text))
                                    .overflow_hidden()
                                    .child(if rule.name.trim().is_empty() {
                                        t!("settings.keywordHighlightNewRule").to_string()
                                    } else {
                                        rule.name.clone()
                                    }),
                            )
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(rgb(palette.text_muted))
                                    .child(t!(
                                        "settings.keywordHighlightPatternCount",
                                        count = count
                                    )),
                            )
                            .child(
                                NyaSwitch::new(format!("keyword-popup-enabled-{}", rule_id))
                                    .checked(rule.enabled)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        cx.stop_propagation();
                                        this.toggle_keyword_highlight_popup_rule(
                                            toggle_group,
                                            &toggle_id,
                                            cx,
                                        );
                                        let _ = window;
                                    })),
                            )
                            .child(popup_icon_button(
                                palette,
                                format!("keyword-popup-delete-{}", rule.id),
                                "icons/fe/delete.svg",
                                t!("common.delete"),
                                true,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.remove_keyword_highlight_popup_rule(
                                        remove_group,
                                        &remove_id,
                                        cx,
                                    );
                                }),
                            )),
                    ),
            );
        }

        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(header)
            .children(rows)
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn render_keyword_highlight_popup_editor(
        &self,
        palette: ThemePalette,
        name_input: AnyElement,
        patterns_input: AnyElement,
        dark_input: AnyElement,
        light_input: AnyElement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let has_selection = self.terminal.keyword_popup.selected_id.is_some();
        let dark_palette = nyaterm_core::keyword_highlight_color_palette(true);
        let light_palette = nyaterm_core::keyword_highlight_color_palette(false);

        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap_3()
            .when(!has_selection, |this| {
                this.child(
                    div().flex_1().flex().items_center().justify_center().child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(palette.text_dimmed))
                            .child(t!("terminalCtx.keywordHighlightSelectHint")),
                    ),
                )
            })
            .when(has_selection, |this| {
                this.child(popup_field_row(
                    palette,
                    t!("settings.keywordHighlightRuleName"),
                    name_input,
                ))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(rgb(palette.text_muted))
                                .child(t!("settings.keywordHighlightRulePatterns")),
                        )
                        .child(patterns_input),
                )
                .child(popup_field_row(
                    palette,
                    t!("settings.keywordHighlightDarkPalette"),
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(dark_input)
                        .child(popup_swatches(
                            palette,
                            "keyword-popup-dark-swatch",
                            dark_palette,
                            KeywordHighlightEditorField::ColorDark,
                            cx,
                        ))
                        .into_any_element(),
                ))
                .child(popup_field_row(
                    palette,
                    t!("settings.keywordHighlightLightPalette"),
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(light_input)
                        .child(popup_swatches(
                            palette,
                            "keyword-popup-light-swatch",
                            light_palette,
                            KeywordHighlightEditorField::ColorLight,
                            cx,
                        ))
                        .into_any_element(),
                ))
            })
            .into_any_element()
    }
}

/// Native child window hosting the keyword-highlight editor. Uses the same
/// `modal_editor` chrome as the quick-command and connection editors so the
/// platform title bar, close button and footer match every other editor.
pub(in crate::features) struct KeywordHighlightWindow {
    app: Entity<NyaTermApp>,
    shell_focus: FocusHandle,
    chrome: ChildWindowChrome,
    _app_subscription: Subscription,
}

impl KeywordHighlightWindow {
    fn new(app: Entity<NyaTermApp>, chrome: ChildWindowChrome, cx: &mut Context<Self>) -> Self {
        let app_subscription = cx.observe(&app, |_, _, cx| cx.notify());
        Self {
            app,
            shell_focus: cx.focus_handle(),
            chrome,
            _app_subscription: app_subscription,
        }
    }
}

impl Render for KeywordHighlightWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The draft is cleared on cancel/save; tear the window down with it.
        if !self.app.read(cx).terminal.keyword_popup.is_open() {
            window.defer(cx, |window, _| window.remove_window());
            return div().size_full().into_any_element();
        }

        let (palette, font, font_size, title) = self.app.read_with(cx, |app, _| {
            (
                app.theme_palette(),
                app.gpui_ui_font().font(),
                app.settings.summary().ui_font_size.clamp(12, 24) as f32,
                t!("terminalCtx.keywordHighlightTitle").to_string(),
            )
        });
        window.set_window_title(&title);
        let content = self
            .app
            .update(cx, |app, cx| app.render_keyword_highlight_popup(window, cx));
        let close_app = self.app.clone();
        let on_close: ChildWindowCloseHandler =
            Rc::new(move |window: &mut Window, cx: &mut App| {
                close_app.update(cx, |app, cx| app.close_keyword_highlight_popup(cx));
                window.remove_window();
            });
        let header_close = on_close.clone();
        focus_child_window_shell_if_idle(&self.shell_focus, window, cx);

        child_window_root(&self.shell_focus, true, on_close)
            .bg(rgb(palette.bg))
            .text_color(rgb(palette.text))
            .font(font)
            .text_size(px(font_size))
            .child(child_window_header(
                palette,
                title,
                None,
                self.chrome,
                window,
                cx,
                move |_, window, cx| header_close(window, cx),
            ))
            .child(div().flex_1().min_h_0().overflow_hidden().child(content))
            .into_any_element()
    }
}

fn open_keyword_highlight_window_now_from_app(app: Entity<NyaTermApp>, cx: &mut App) {
    if app
        .read(cx)
        .terminal
        .keyword_popup
        .window
        .handle()
        .is_some()
    {
        app.update(cx, |app, cx| {
            app.terminal.keyword_popup.window.cancel_open();
            app.activate_keyword_highlight_window(cx);
            cx.notify();
        });
        return;
    }
    if !app.read(cx).terminal.keyword_popup.is_open() {
        app.update(cx, |app, cx| {
            app.terminal.keyword_popup.window.cancel_open();
            cx.notify();
        });
        return;
    }

    let title = t!("terminalCtx.keywordHighlightTitle").to_string();
    let spec = ChildWindowSpec::modal_editor(title, 880., 600.).min_size(640., 460.);
    let chrome = spec.chrome();
    let parent = app.read(cx).shell.main_window();
    let options = child_window_options(&spec, parent, cx);
    let close_app = app.clone();
    let view_app = app.clone();
    let result: anyhow::Result<NyaWindowHandle> = cx.open_window(options, move |window, cx| {
        window.on_window_should_close(cx, move |_, cx| {
            close_app.update(cx, |app, cx| app.close_keyword_highlight_popup(cx));
            true
        });
        let view = cx.new(|cx| KeywordHighlightWindow::new(view_app, chrome, cx));
        cx.new(|cx| nya_root(view, window, cx))
    });

    app.update(cx, |app, _| match result {
        Ok(handle) => app.terminal.keyword_popup.window.finish_open(handle),
        Err(_) => app.terminal.keyword_popup.window.fail_open(),
    });
}

/// A swatch row that writes the chosen colour into a popup input.
fn popup_swatches(
    palette: ThemePalette,
    id_prefix: &str,
    colors: &[&str],
    field: KeywordHighlightEditorField,
    cx: &mut Context<NyaTermApp>,
) -> AnyElement {
    let mut swatches = Vec::new();
    for (index, color) in colors.iter().enumerate() {
        let value = (*color).to_string();
        let out = value.clone();
        swatches.push(
            div()
                .id(SharedString::from(format!("{id_prefix}-{index}")))
                .size(px(18.))
                .rounded_sm()
                .bg(rgb(parse_popup_swatch(color).unwrap_or(0x79c0ff)))
                .border_1()
                .border_color(rgb(palette.border))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.reset_text_input(&popup_input_id(field), &out, cx);
                    this.apply_keyword_highlight_popup_input(field, out.clone(), cx);
                    let _ = window;
                })),
        );
    }
    div()
        .flex()
        .flex_wrap()
        .gap_1()
        .children(swatches)
        .into_any_element()
}

fn popup_field_row(
    palette: ThemePalette,
    label: impl Into<SharedString>,
    control: AnyElement,
) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap_2()
        .child(
            div()
                .w(px(110.))
                .flex_none()
                .text_size(px(11.))
                .text_color(rgb(palette.text_muted))
                .child(label.into()),
        )
        .child(control)
        .into_any_element()
}

fn popup_icon_button(
    palette: ThemePalette,
    id: impl Into<String>,
    icon_path: &'static str,
    tooltip: impl Into<SharedString>,
    enabled: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let tooltip: SharedString = tooltip.into();
    let hover_bg = rgba((palette.danger << 8) | 0x18);
    div()
        .id(SharedString::from(id.into()))
        .size(px(24.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_sm()
        .text_color(rgb(palette.danger))
        .child(
            svg()
                .size(px(14.))
                .path(icon_path)
                .text_color(rgb(palette.danger)),
        )
        .tooltip(move |window, cx| NyaTooltip::new(tooltip.clone()).build(window, cx))
        .when(enabled, |this| {
            this.cursor_pointer()
                .hover(move |this| this.bg(hover_bg))
                .on_click(on_click)
        })
        .into_any_element()
}

fn new_popup_rule() -> KeywordHighlightRule {
    KeywordHighlightRule {
        id: format!("kh-{}", uuid::Uuid::new_v4()),
        name: String::new(),
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
        patterns: lines,
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
    use super::*;

    #[test]
    fn prefill_uses_first_line_as_name_and_trims_patterns() {
        let rule = prefill_rule_from_text("  ERROR  \n\n  panic at line 4 \nWARN\n".into())
            .expect("selection should produce a rule");
        assert_eq!(rule.name, "ERROR");
        assert_eq!(rule.patterns, vec!["ERROR", "panic at line 4", "WARN"]);
        assert!(rule.id.starts_with("kh-"));
        assert!(rule.enabled);
    }

    #[test]
    fn prefill_rejects_whitespace_only_selection() {
        assert!(prefill_rule_from_text("  \n\n\t\n".into()).is_none());
    }

    #[test]
    fn popup_color_normalizes_hex() {
        assert_eq!(normalize_popup_color("  #AABBCC "), "#aabbcc");
        assert_eq!(normalize_popup_color("79c0ff"), "#79c0ff");
        assert_eq!(normalize_popup_color("#GG0011"), "#0011");
    }

    #[test]
    fn popup_swatch_parses_six_digit_hex() {
        assert_eq!(parse_popup_swatch("#79c0ff"), Some(0x79c0ff));
        assert_eq!(parse_popup_swatch("0969da"), Some(0x0969da));
        assert_eq!(parse_popup_swatch("#fff"), None);
    }

    #[test]
    fn popup_input_id_uses_dedicated_prefix() {
        assert_eq!(
            popup_input_id(KeywordHighlightEditorField::Patterns),
            "keyword.highlight.popup.patterns"
        );
    }
}
