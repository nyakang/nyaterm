use super::ASSET_BATCH_SELECT_PREFIX;
use super::basis::{asset_batch_kind_counts_label, batch_skip_reasons_label};
use super::fields::{BatchFieldId, DISPLAY_FIELDS, EDITABLE_FIELDS, FieldBasis};
use super::options::batch_kind_label;
use crate::features::NyaTermApp;
use crate::features::assets::BatchEditMode;
use gpui::{AnyElement, Context, IntoElement, SharedString, div, prelude::*, px, rgb};
use nyaterm_ui::{
    NyaButton, NyaButtonVariant, NyaInputShell, NyaScrollable, NyaSelect, NyaTabItem, NyaTabs,
};
use rust_i18n::t;
fn batch_field_is_select(field: BatchFieldId) -> bool {
    matches!(
        field,
        BatchFieldId::AuthMode
            | BatchFieldId::Group
            | BatchFieldId::Icon
            | BatchFieldId::Encoding
            | BatchFieldId::Backspace
            | BatchFieldId::SshProfile
            | BatchFieldId::TerminalType
            | BatchFieldId::SftpCwdFollow
            | BatchFieldId::SftpFilenameEncoding
            | BatchFieldId::SftpPipelineDepth
            | BatchFieldId::SshAlgorithmMode
            | BatchFieldId::RdpCertificatePolicy
            | BatchFieldId::VncSecurityMode
            | BatchFieldId::SshAuthAgentEndpoint
            | BatchFieldId::Account
            | BatchFieldId::Key
            | BatchFieldId::Otp
            | BatchFieldId::Proxy
            | BatchFieldId::ProxyJump
            | BatchFieldId::SerialPort
            | BatchFieldId::BaudRate
            | BatchFieldId::DataBits
            | BatchFieldId::Parity
            | BatchFieldId::StopBits
            | BatchFieldId::FlowControl
            | BatchFieldId::TelnetEnterMode
            | BatchFieldId::RdpBackend
            | BatchFieldId::RdpDisplayMode
            | BatchFieldId::VncScaleMode
            | BatchFieldId::RecordingMode
            | BatchFieldId::AgentPolicy
    )
}

impl NyaTermApp {
    pub(in crate::features) fn asset_batch_edit_window_view(
        &mut self,
        viewport: (f32, f32),
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = self.theme_palette();
        let draft = self.start_workspace.asset_batch();
        let kinds = draft
            .counts()
            .iter()
            .map(|(kind, _)| *kind)
            .collect::<Vec<_>>();
        let selected = draft
            .display_kind()
            .and_then(|kind| kinds.iter().position(|k| *k == kind))
            .map(|index| index + 1)
            .unwrap_or(0);
        let mut tabs = vec![NyaTabItem::new(t!(
            "assets.batch.commonScope",
            count = draft.ids().len()
        ))];
        tabs.extend(draft.counts().iter().map(|(kind, count)| {
            NyaTabItem::new(format!("{} ({count})", batch_kind_label(*kind)))
        }));
        let kind_control = if viewport.0 >= 760. {
            NyaTabs::new("asset-batch-scopes")
                .items(tabs)
                .selected_index(selected)
                .full_width(true)
                .on_select(cx.listener(move |this, index: &usize, _, cx| {
                    this.set_asset_batch_scope(
                        index.checked_sub(1).and_then(|i| kinds.get(i).copied()),
                        cx,
                    );
                }))
                .into_any_element()
        } else {
            div()
                .w_full()
                .h(px(32.))
                .children(
                    self.selects
                        .field(&SharedString::from(format!(
                            "{ASSET_BATCH_SELECT_PREFIX}kind"
                        )))
                        .map(|entity| NyaSelect::new(&entity)),
                )
                .into_any_element()
        };
        let (connections, fields) = draft.summary();
        let summary = t!(
            "assets.batch.summary",
            connections = connections,
            fields = fields
        )
        .to_string();
        let counts = asset_batch_kind_counts_label(draft.counts());
        let total = draft.ids().len();
        let error = draft.error().cloned();
        let saving = draft.saving();
        let apply_disabled = saving || !draft.has_edits() || draft.build_edits().is_err();
        let scroll_handle = draft.scroll().clone();
        let rows = DISPLAY_FIELDS
            .into_iter()
            .chain(EDITABLE_FIELDS)
            .filter(|field| {
                draft.basis(*field) != FieldBasis::Inapplicable
                    || draft.rdp_applicability(*field).is_some()
            })
            .map(|field| self.asset_batch_mode_row(field, cx))
            .collect::<Vec<_>>();
        div()
            .id("asset-batch-panel")
            .size_full()
            .flex()
            .flex_col()
            .min_w_0()
            .child(
                div()
                    .flex_none()
                    .p_4()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(kind_control)
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(palette.text_muted))
                            .child(t!(
                                "assets.batch.selectionSummary",
                                count = total,
                                protocols = counts
                            )),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(palette.text_muted))
                            .child(t!("assets.batch.scopeNote")),
                    ),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .vertical_scrollbar(&scroll_handle)
                    .child(
                        div()
                            .id("asset-batch-scroll")
                            .size_full()
                            .min_h_0()
                            .min_w_0()
                            .overflow_y_scroll()
                            .track_scroll(&scroll_handle)
                            .p_4()
                            .flex()
                            .flex_col()
                            .gap_4()
                            .children(rows),
                    ),
            )
            .when_some(error, |this, error| {
                this.child(
                    div()
                        .p_3()
                        .text_sm()
                        .text_color(rgb(palette.danger))
                        .child(error),
                )
            })
            .child(
                div()
                    .flex_none()
                    .border_t_1()
                    .border_color(rgb(palette.border))
                    .p_3()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(div().flex_1().min_w(px(160.)).text_xs().child(summary))
                    .child(
                        NyaButton::new("asset-batch-cancel", t!("common.cancel"))
                            .variant(NyaButtonVariant::Secondary)
                            .on_click(
                                cx.listener(|this, _, _, cx| this.close_asset_batch_edit(cx)),
                            ),
                    )
                    .child(
                        NyaButton::new("asset-batch-apply", t!("assets.batch.apply"))
                            .variant(NyaButtonVariant::Primary)
                            .disabled(apply_disabled)
                            .on_click(cx.listener(|this, _, _, cx| this.save_asset_batch_edit(cx))),
                    ),
            )
            .into_any_element()
    }
    fn asset_batch_mode_row(&self, field: BatchFieldId, cx: &mut Context<Self>) -> AnyElement {
        let draft = self.start_workspace.asset_batch();
        let palette = self.theme_palette();
        let mode = draft.mode(field);
        let editable = field.is_editable();
        let applicability = draft.rdp_applicability(field);
        let control = if editable && mode == BatchEditMode::Set {
            if field.is_bool() || batch_field_is_select(field) {
                div()
                    .w_full()
                    .h(px(32.))
                    .children(
                        self.selects
                            .field(&field.select_id())
                            .map(|entity| NyaSelect::new(&entity)),
                    )
                    .into_any_element()
            } else {
                let multi_line = matches!(
                    field,
                    BatchFieldId::Description | BatchFieldId::PostLoginCommand
                );
                div()
                    .w_full()
                    .h(px(if multi_line { 80. } else { 32. }))
                    .children(
                        self.existing_text_input(field.text_input_id())
                            .map(|entity| {
                                let shell = NyaInputShell::new(field.text_input_id(), &entity)
                                    .height(px(if multi_line { 80. } else { 32. }));
                                if multi_line {
                                    shell.multi_line()
                                } else {
                                    shell
                                }
                            }),
                    )
                    .into_any_element()
            }
        } else {
            let text = if mode == BatchEditMode::Clear {
                match field {
                    BatchFieldId::Port => draft
                        .display_kind()
                        .map(|kind| {
                            t!("assets.batch.restoreDefault", value = kind.default_port())
                                .to_string()
                        })
                        .unwrap_or_default(),
                    BatchFieldId::RecordingUseGlobal
                    | BatchFieldId::RecordingAutoStart
                    | BatchFieldId::RecordingMode => t!("assets.batch.clearRecording").to_string(),
                    BatchFieldId::Encoding => t!("assets.batch.clearEncoding").to_string(),
                    BatchFieldId::SftpFilenameEncoding => {
                        t!("assets.batch.clearFilenameEncoding").to_string()
                    }
                    BatchFieldId::Group
                    | BatchFieldId::Description
                    | BatchFieldId::Icon
                    | BatchFieldId::Tags
                    | BatchFieldId::Account
                    | BatchFieldId::Password
                    | BatchFieldId::Key
                    | BatchFieldId::Otp
                    | BatchFieldId::Proxy
                    | BatchFieldId::ProxyJump
                    | BatchFieldId::ShellArgs
                    | BatchFieldId::WorkingDir => t!("assets.batch.clearValue").to_string(),
                    _ => t!("assets.batch.clearMeaning").to_string(),
                }
            } else {
                draft.disabled_text(field)
            };
            div()
                .w_full()
                .min_h(px(32.))
                .rounded_sm()
                .border_1()
                .border_color(rgb(palette.border))
                .bg(rgb(palette.input_surface()))
                .px_3()
                .py_2()
                .text_sm()
                .text_color(rgb(palette.text_muted))
                .child(text)
                .into_any_element()
        };
        div()
            .id(format!("asset-batch-field-{}", field.key()))
            .debug_selector(move || format!("asset-batch-field-{}", field.key()))
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(140.))
                            .text_sm()
                            .child(t!(field.label_key())),
                    )
                    .when(editable, |this| {
                        this.children(
                            [
                                (BatchEditMode::Keep, "assets.batch.modeKeep"),
                                (BatchEditMode::Set, "assets.batch.modeSet"),
                                (BatchEditMode::Clear, "assets.batch.modeClear"),
                            ]
                            .into_iter()
                            .map(|(choice, label)| {
                                div()
                                    .debug_selector(move || {
                                        format!("asset-batch-mode-{}-{choice:?}", field.key())
                                    })
                                    .child(
                                        NyaButton::new(
                                            format!("asset-batch-mode-{}-{choice:?}", field.key()),
                                            t!(label),
                                        )
                                        .variant(NyaButtonVariant::Ghost)
                                        .selected(mode == choice)
                                        .disabled(
                                            draft.saving()
                                                || (choice != BatchEditMode::Keep
                                                    && !draft.field_available(field)),
                                        )
                                        .height(px(28.))
                                        .on_click(
                                            cx.listener(move |this, _, _, cx| {
                                                this.set_asset_batch_mode(field, choice, cx)
                                            }),
                                        ),
                                    )
                            }),
                        )
                    }),
            )
            .when_some(applicability, |this, applicability| {
                let reasons = batch_skip_reasons_label(applicability.skipped_reasons);
                this.child(
                    div()
                        .text_xs()
                        .text_color(rgb(palette.text_muted))
                        .child(t!(
                            "assets.batch.fieldApplicability",
                            applicable = applicability.applicable,
                            total = applicability.total
                        )),
                )
                .when(!reasons.is_empty(), |this| {
                    this.child(
                        div()
                            .text_xs()
                            .text_color(rgb(palette.text_muted))
                            .child(reasons),
                    )
                })
            })
            .child(control)
            .when(editable && mode == BatchEditMode::Set, |this| {
                let empty = if field.is_bool() {
                    !draft.current_bool_is_set(field)
                } else if field == BatchFieldId::Password {
                    draft.replacement_password().is_empty()
                } else {
                    draft.value(field).trim().is_empty()
                };
                this.when(empty, |this| {
                    this.child(div().text_xs().text_color(rgb(palette.text_muted)).child(
                        if field.is_bool() {
                            t!("assets.batch.chooseBoolean").to_string()
                        } else {
                            t!("assets.batch.errorUseClear", field = t!(field.label_key()))
                                .to_string()
                        },
                    ))
                })
            })
            .into_any_element()
    }
}
