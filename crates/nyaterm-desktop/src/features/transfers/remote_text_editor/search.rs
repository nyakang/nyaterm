use gpui::{Context, Entity, IntoElement, Subscription, div, prelude::*, px};
use nyaterm_ui::{NyaButton, NyaCheckbox, NyaInput, NyaInputEvent, NyaInputState};
use rust_i18n::t;

use super::RemoteTextEditor;

impl RemoteTextEditor {
    pub(super) fn search_inputs(
        cx: &mut Context<Self>,
    ) -> (
        Entity<NyaInputState>,
        Entity<NyaInputState>,
        Vec<Subscription>,
    ) {
        let search = cx.new(|cx| NyaInputState::new(cx, "").placeholder(t!("documentEditor.find")));
        let replacement =
            cx.new(|cx| NyaInputState::new(cx, "").placeholder(t!("documentEditor.replace")));
        let subscription = cx.subscribe(&search, |this, _, event, cx| match event {
            NyaInputEvent::Changed(query) => {
                this.search_query = query.clone();
                this.active_match = 0;
                this.select_search_match(0, cx);
            }
            NyaInputEvent::Submitted(_) => this.select_search_match(1, cx),
            NyaInputEvent::Blurred(_) => {}
        });
        (search, replacement, vec![subscription])
    }

    pub(super) fn did_edit(&mut self, cx: &mut Context<Self>) {
        self.marked_range = None;
        self.last_layout = None;
        self.scroll_cursor_pending = true;
        self.sync_content_to_app(cx);
        cx.notify();
    }

    fn select_search_match(&mut self, step: isize, cx: &mut Context<Self>) {
        match self
            .search_options
            .matches(&self.document.content, &self.search_query)
        {
            Ok(matches) => {
                self.search_error = None;
                if !matches.is_empty() {
                    self.active_match = (self.active_match as isize + step)
                        .rem_euclid(matches.len() as isize)
                        as usize;
                    let range = &matches[self.active_match];
                    self.folds.retain(|fold| !fold.contains(&range.start));
                    self.document.anchor = range.start;
                    self.document.head = range.end;
                    self.document.additional_selections.clear();
                    self.scroll_cursor_pending = true;
                }
            }
            Err(_) => self.search_error = Some(t!("documentEditor.invalidRegex").to_string()),
        }
        cx.notify();
    }

    fn replace_search(&mut self, all: bool, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        let replacement = self.replace_input.read(cx).value(cx);
        match self.search_options.replacements(
            &self.document.content,
            &self.search_query,
            &replacement,
            (!all).then_some(self.active_match),
        ) {
            Ok(edits) => {
                if self.document.apply(edits).is_ok() {
                    self.document.additional_selections.clear();
                    self.did_edit(cx);
                    self.select_search_match(0, cx);
                }
            }
            Err(_) => {
                self.search_error = Some(t!("documentEditor.invalidRegex").to_string());
                cx.notify();
            }
        }
    }

    pub(super) fn select_next_occurrence(&mut self, cx: &mut Context<Self>) {
        let range = self.selected_range();
        if range.is_empty() {
            self.select_word_at(self.document.head, cx);
            return;
        }
        let text = self.document.content[range.clone()].to_string();
        let existing = self.document.selections();
        let mut matches = self
            .document
            .content
            .match_indices(&text)
            .map(|(offset, _)| offset..offset + text.len())
            .filter(|range| !existing.contains(range))
            .collect::<Vec<_>>();
        matches.sort_by_key(|found| (found.start < range.end, found.start));
        if let Some(found) = matches.into_iter().next() {
            self.document.additional_selections.push(range);
            self.document.anchor = found.start;
            self.document.head = found.end;
            self.folds.retain(|fold| !fold.contains(&found.start));
            self.scroll_cursor_pending = true;
            cx.notify();
        }
    }

    pub(super) fn search_bar(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let _retain_subscriptions = &self.search_subscriptions;
        let count = self
            .search_options
            .matches(&self.document.content, &self.search_query)
            .map(|matches| matches.len())
            .unwrap_or(0);
        div()
            .flex_none()
            .flex()
            .flex_col()
            .gap_2()
            .p_2()
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    this.search_open = false;
                    this.app.update(cx, |app, _| {
                        if let Some(tab) = app.transfer.active_editor_tab_mut() {
                            tab.focused_field = crate::models::TransferEditorField::Content;
                        }
                    });
                    window.focus(&this.focus_handle, cx);
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h(px(32.))
                            .child(NyaInput::new(&self.search_input)),
                    )
                    .child(div().text_xs().child(format!(
                        "{} / {count}",
                        if count == 0 {
                            0
                        } else {
                            self.active_match.min(count - 1) + 1
                        }
                    )))
                    .child(
                        NyaButton::new("editor-search-prev", t!("documentEditor.previous"))
                            .small()
                            .on_click(
                                cx.listener(|this, _, _, cx| this.select_search_match(-1, cx)),
                            ),
                    )
                    .child(
                        NyaButton::new("editor-search-next", t!("documentEditor.next"))
                            .small()
                            .on_click(
                                cx.listener(|this, _, _, cx| this.select_search_match(1, cx)),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        NyaCheckbox::new("editor-search-case")
                            .label(t!("documentEditor.caseSensitive"))
                            .checked(self.search_options.case_sensitive)
                            .on_click(cx.listener(|this, checked, _, cx| {
                                this.search_options.case_sensitive = *checked;
                                this.select_search_match(0, cx);
                            })),
                    )
                    .child(
                        NyaCheckbox::new("editor-search-word")
                            .label(t!("documentEditor.wholeWord"))
                            .checked(self.search_options.whole_word)
                            .on_click(cx.listener(|this, checked, _, cx| {
                                this.search_options.whole_word = *checked;
                                this.select_search_match(0, cx);
                            })),
                    )
                    .child(
                        NyaCheckbox::new("editor-search-regex")
                            .label(t!("documentEditor.regex"))
                            .checked(self.search_options.regex)
                            .on_click(cx.listener(|this, checked, _, cx| {
                                this.search_options.regex = *checked;
                                this.select_search_match(0, cx);
                            })),
                    ),
            )
            .when(!self.read_only, |this| {
                this.child(
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .h(px(32.))
                                .child(NyaInput::new(&self.replace_input)),
                        )
                        .child(
                            NyaButton::new("editor-replace", t!("documentEditor.replace"))
                                .small()
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.replace_search(false, cx)),
                                ),
                        )
                        .child(
                            NyaButton::new("editor-replace-all", t!("documentEditor.replaceAll"))
                                .small()
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.replace_search(true, cx)),
                                ),
                        ),
                )
            })
            .when_some(self.search_error.clone(), |this, error| {
                this.child(div().text_xs().child(error))
            })
    }
}
