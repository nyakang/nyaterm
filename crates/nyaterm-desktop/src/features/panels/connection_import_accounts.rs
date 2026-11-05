use rust_i18n::t;

use gpui::{
    AnyElement, Context, FontWeight, IntoElement, SharedString, Stateful, Window, div,
    prelude::{
        FluentBuilder as _, InteractiveElement as _, ParentElement as _,
        StatefulInteractiveElement as _, Styled as _,
    },
    px, rgb,
};
use nyaterm_core::WindtermOneKeyConfigState;
use nyaterm_ui::{NyaScrollable, NyaSwitch};

use crate::features::NyaTermApp;
use crate::features::connections::WindtermAccountDraft;
use crate::features::text_inputs::TextInputSetup;
use crate::theme::ThemePalette;

impl NyaTermApp {
    /// Step 2 of the WindTerm import: confirm the accounts inferred from
    /// `onekeys.config`, letting the user fill in or override each credential
    /// before the sessions and accounts are written together.
    pub(in crate::features) fn open_windterm_accounts_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_form_dialog(
            (
                t!("savedConnections.windtermAccountsTitle").to_string(),
                520.,
                t!("savedConnections.windtermAccountsImport").to_string(),
                |app, _, cx| app.windterm_accounts_dialog_content(cx),
                |app, window, cx| app.submit_windterm_accounts_import(window, cx),
                |app, cx| app.cancel_windterm_accounts_import(cx),
            ),
            window,
            cx,
        );
        cx.notify();
    }

    fn windterm_accounts_dialog_content(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(preview) = self.connection_state.windterm_accounts_preview() else {
            return div().into_any_element();
        };
        let one_key_state = preview.one_key_state;
        let unsupported = preview.unsupported_one_key_count;
        let invalid = preview.invalid_one_key_count;
        let duplicate = preview.duplicate_one_key_uuid_count;
        let missing = preview.missing_one_keys.len();
        let unreadable = preview.unreadable_account_count;
        let drafts = self.connection_state.windterm_account_drafts().to_vec();
        let palette = self.theme_palette();
        let session_total: usize = drafts.iter().map(|draft| draft.session_count).sum();

        let mut rows = Vec::with_capacity(drafts.len());
        for draft in &drafts {
            rows.push(self.windterm_account_row(draft, palette, cx));
        }

        let mut content = div()
            .id("windterm-accounts-dialog-content")
            .debug_selector(|| "windterm-accounts-dialog-content".to_owned())
            .flex()
            .flex_col()
            .w_full()
            .gap_3()
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(palette.text_muted))
                    .child(t!(
                        "savedConnections.windtermAccountsDesc",
                        accounts = drafts.len(),
                        sessions = session_total
                    )),
            )
            .child(
                div()
                    .id("windterm-accounts-list")
                    .max_h(px(360.))
                    .overflow_y_scrollbar()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .children(rows),
            );

        if one_key_state == WindtermOneKeyConfigState::Absent {
            content = content.child(notice(
                palette.warning,
                t!("savedConnections.windtermOneKeysConfigMissing").to_string(),
            ));
        } else if one_key_state == WindtermOneKeyConfigState::Unreadable {
            content = content.child(notice(
                palette.warning,
                t!("savedConnections.windtermOneKeysConfigUnreadable").to_string(),
            ));
        }
        if missing > 0 {
            content = content.child(notice(
                palette.warning,
                t!(
                    "savedConnections.windtermAccountsMissingReferences",
                    count = missing
                )
                .to_string(),
            ));
        }
        if unsupported > 0 {
            content = content.child(notice(
                palette.warning,
                t!(
                    "savedConnections.windtermAccountsUnsupportedTypes",
                    count = unsupported
                )
                .to_string(),
            ));
        }
        if unreadable > 0 {
            content = content.child(notice(
                palette.warning,
                t!(
                    "savedConnections.windtermAccountsUnreadable",
                    count = unreadable
                )
                .to_string(),
            ));
        }
        if invalid > 0 {
            content = content.child(notice(
                palette.text_dimmed,
                t!(
                    "savedConnections.windtermAccountsInvalidEntries",
                    count = invalid
                )
                .to_string(),
            ));
        }
        if duplicate > 0 {
            content = content.child(notice(
                palette.text_dimmed,
                t!(
                    "savedConnections.windtermAccountsDuplicateUuids",
                    count = duplicate
                )
                .to_string(),
            ));
        }

        content
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(palette.text_dimmed))
                    .child(t!("savedConnections.windtermAccountsHint")),
            )
            .into_any_element()
    }

    fn windterm_account_row(
        &mut self,
        draft: &WindtermAccountDraft,
        palette: ThemePalette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let uuid = draft.one_key_uuid.clone();
        let username_id = format!("windterm-account.{uuid}.username");
        let password_id = format!("windterm-account.{uuid}.password");
        let username_input =
            self.text_input_box(username_id, &draft.username, TextInputSetup::default(), cx);
        let password_input = self.text_input_box(
            password_id,
            draft.password.expose_secret(),
            TextInputSetup::masked(),
            cx,
        );
        let reuse_selected = draft.reuse_account_id.is_some();
        let skip = draft.skip;

        // Reuse choices are plain chips rather than a select: the list is at most
        // a handful of same-name accounts, and each choice is a single click.
        let mut reuse_choices: Vec<AnyElement> = Vec::new();
        let new_uuid = uuid.clone();
        reuse_choices.push(
            account_choice_chip(
                palette,
                format!("windterm-account.{uuid}.new").into(),
                t!("savedConnections.windtermAccountsCreateNew").into(),
                !reuse_selected,
            )
            .on_click(cx.listener(move |app, _, _, cx| {
                app.set_windterm_account_reuse(&new_uuid, None, cx);
            }))
            .into_any_element(),
        );
        for candidate in &draft.reuse_candidates {
            let candidate_uuid = uuid.clone();
            let candidate_id = candidate.id.clone();
            reuse_choices.push(
                account_choice_chip(
                    palette,
                    format!("windterm-account.{uuid}.reuse.{}", candidate.id).into(),
                    SharedString::from(format!("{} ({})", candidate.name, candidate.username)),
                    draft.reuse_account_id.as_deref() == Some(candidate.id.as_str()),
                )
                .on_click(cx.listener(move |app, _, _, cx| {
                    app.set_windterm_account_reuse(&candidate_uuid, Some(candidate_id.clone()), cx);
                }))
                .into_any_element(),
            );
        }

        div()
            .w_full()
            .p_3()
            .rounded(px(6.))
            .border_1()
            .border_color(rgb(palette.border))
            .bg(rgb(palette.surface))
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .text_xs()
                            .font_weight(FontWeight(600.))
                            .text_color(rgb(palette.text))
                            .child(SharedString::from(draft.name.clone())),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(11.))
                            .text_color(rgb(palette.text_muted))
                            .child(t!(
                                "savedConnections.windtermAccountsSessions",
                                count = draft.session_count
                            )),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(field_label(
                                palette,
                                t!("savedConnections.windtermAccountsUsername").to_string(),
                            ))
                            .child(username_input),
                    )
                    .when(!reuse_selected, |this| {
                        this.child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(field_label(
                                    palette,
                                    t!("savedConnections.windtermAccountsPassword").to_string(),
                                ))
                                .child(password_input),
                        )
                    }),
            )
            .when(!draft.password_available, |this| {
                this.child(notice(
                    palette.warning,
                    t!("savedConnections.windtermAccountsNeedsManualPassword").to_string(),
                ))
            })
            .when(!draft.reuse_candidates.is_empty(), |this| {
                this.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(
                            div()
                                .flex_none()
                                .text_size(px(10.))
                                .text_color(rgb(palette.text_muted))
                                .child(t!("savedConnections.windtermAccountsReuseChoice")),
                        )
                        .children(reuse_choices),
                )
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(palette.text_muted))
                            .child(t!("savedConnections.windtermAccountsSkip")),
                    )
                    .child(
                        NyaSwitch::new(format!("windterm-account.{uuid}.skip"))
                            .checked(skip)
                            .on_click(cx.listener(move |app, _, _, cx| {
                                app.set_windterm_account_skip(&uuid, !skip, cx);
                            })),
                    ),
            )
            .into_any_element()
    }
}

fn field_label(palette: ThemePalette, label: String) -> AnyElement {
    div()
        .text_size(px(10.))
        .text_color(rgb(palette.text_muted))
        .child(label)
        .into_any_element()
}

fn notice(color: u32, message: String) -> AnyElement {
    div()
        .text_xs()
        .text_color(rgb(color))
        .child(message)
        .into_any_element()
}

fn account_choice_chip(
    palette: ThemePalette,
    id: SharedString,
    label: SharedString,
    active: bool,
) -> Stateful<gpui::Div> {
    div()
        .id(id)
        .px_2()
        .py(px(2.))
        .rounded_full()
        .border_1()
        .border_color(rgb(if active {
            palette.primary
        } else {
            palette.border
        }))
        .bg(rgb(if active {
            palette.surface_elevated
        } else {
            palette.surface
        }))
        .text_size(px(10.))
        .text_color(rgb(if active {
            palette.text
        } else {
            palette.text_muted
        }))
        .cursor_pointer()
        .child(label)
}
