use super::ASSET_BATCH_SELECT_PREFIX;
use super::basis::{asset_batch_kind_counts_label, batch_skip_reasons_label};
use super::fields::{
    BatchFieldAction, BatchFieldKey, BatchFieldSpec, BatchRowLayout, FieldBasis, batch_rows,
};
use crate::features::NyaTermApp;
use crate::features::assets::BatchEditMode;
use gpui::{AnyElement, Context, IntoElement, SharedString, div, prelude::*, px, rgb};
use nyaterm_ui::{
    NyaButton, NyaButtonVariant, NyaCheckbox, NyaInputShell, NyaScrollable, NyaSelect, NyaTabItem,
    NyaTabs,
};
use rust_i18n::t;
fn batch_field_is_select(field: BatchFieldKey) -> bool {
    matches!(
        field,
        BatchFieldKey::AuthMode
            | BatchFieldKey::Group
            | BatchFieldKey::Icon
            | BatchFieldKey::Encoding
            | BatchFieldKey::Backspace
            | BatchFieldKey::SshProfile
            | BatchFieldKey::TerminalType
            | BatchFieldKey::SftpCwdFollow
            | BatchFieldKey::SftpFilenameEncoding
            | BatchFieldKey::SftpPipelineDepth
            | BatchFieldKey::SshAlgorithmMode
            | BatchFieldKey::RdpCertificatePolicy
            | BatchFieldKey::VncSecurityMode
            | BatchFieldKey::SshAuthAgentEndpoint
            | BatchFieldKey::Account
            | BatchFieldKey::Key
            | BatchFieldKey::Otp
            | BatchFieldKey::Proxy
            | BatchFieldKey::ProxyJump
            | BatchFieldKey::SerialPort
            | BatchFieldKey::BaudRate
            | BatchFieldKey::DataBits
            | BatchFieldKey::Parity
            | BatchFieldKey::StopBits
            | BatchFieldKey::FlowControl
            | BatchFieldKey::TelnetEnterMode
            | BatchFieldKey::RdpBackend
            | BatchFieldKey::RdpDisplayMode
            | BatchFieldKey::RdpAudio
            | BatchFieldKey::VncScaleMode
            | BatchFieldKey::RecordingMode
            | BatchFieldKey::AgentPolicy
            | BatchFieldKey::AdbScreenQuality
            | BatchFieldKey::AdbScreenOrientation
            | BatchFieldKey::FtpPassiveMode
            | BatchFieldKey::FtpFilenameEncodingMode
            | BatchFieldKey::FtpFilenameEncoding
            | BatchFieldKey::FtpSecurityMode
            | BatchFieldKey::FtpTlsVersion
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
        let selected = draft
            .display_kind()
            .and_then(|kind| {
                crate::models::ConnectionKindTab::from_batch_kind(kind).position_in_all()
            })
            .map(|index| index + 1)
            .unwrap_or(0);
        let mut tabs = crate::models::ConnectionKindTab::ALL
            .into_iter()
            .map(|tab| {
                let count = draft
                    .counts()
                    .iter()
                    .find(|(kind, _)| *kind == tab.batch_kind())
                    .map(|(_, count)| *count)
                    .unwrap_or(0);
                NyaTabItem::new(format!("{} ({count})", tab.label())).disabled(count == 0)
            })
            .collect::<Vec<_>>();
        tabs.insert(
            0,
            NyaTabItem::new(t!("assets.batch.scopeCommon").to_string()),
        );
        let kind_control = if viewport.0 >= 760. {
            NyaTabs::new("asset-batch-scopes")
                .items(tabs)
                .selected_index(selected)
                .full_width(true)
                .on_select(cx.listener(move |this, index: &usize, _, cx| {
                    let kind = index
                        .checked_sub(1)
                        .and_then(|index| crate::models::ConnectionKindTab::ALL.get(index))
                        .map(|tab| tab.batch_kind());
                    this.set_asset_batch_scope(kind, cx);
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
        let (changed_connections, changed_fields) = draft.summary();
        let summary = t!(
            "assets.batch.impactSummary",
            connections = changed_connections,
            fields = changed_fields
        )
        .to_string();
        let expanded = draft.summary_expanded();
        let scope_summaries = if expanded {
            draft.scope_summaries()
        } else {
            Vec::new()
        };
        let counts = asset_batch_kind_counts_label(draft.counts());
        let total = draft.ids().len();
        let error = draft.error().cloned();
        let saving = draft.saving();
        let has_connections =
            !draft.ids().is_empty() && draft.counts().iter().any(|(_, count)| *count > 0);
        let apply_disabled =
            saving || !has_connections || !draft.has_edits() || !draft.valid_for_save();
        let scroll_handle = draft.scroll().clone();
        let kind_tab = draft
            .display_kind()
            .map(crate::models::ConnectionKindTab::from_batch_kind)
            .unwrap_or(crate::models::ConnectionKindTab::Ssh);
        let sections = if draft.display_kind().is_none() {
            &[crate::models::ConnectionEditorSection::Basic][..]
        } else {
            crate::models::ConnectionEditorSection::for_kind(kind_tab, false)
        };
        let show_section_nav = sections.len() > 1;
        let active_section = draft
            .section()
            .filter(|section| sections.contains(section))
            .unwrap_or(sections[0]);
        // Mirror the connection editor's left rail: one vertical column of voice
        // buttons, 180 px wide when the dialog is wide and 128 px otherwise.
        let nav_width = if viewport.0 >= 760. { 180. } else { 128. };
        let mut nav = div()
            .id("asset-batch-navigation-rail")
            .flex()
            .flex_col()
            .gap_1()
            .p_3()
            .w(px(nav_width))
            .flex_none()
            .overflow_y_scrollbar();
        if let Some(focus) = draft.navigation_focus().cloned() {
            nav = nav.track_focus(&focus).on_key_down(cx.listener(
                |this, event: &gpui::KeyDownEvent, _, cx| {
                    if this.asset_batch_navigation_key(event, cx) {
                        cx.stop_propagation();
                    }
                },
            ));
        }
        let nav = nav.children(sections.iter().copied().map(|section| {
            NyaButton::new(
                format!("asset-batch-section-{}", section.id()),
                section.label().to_string(),
            )
            .variant(NyaButtonVariant::Ghost)
            .full_width()
            .height(px(40.))
            .label_left()
            .tooltip(section.label())
            .selected(section == active_section)
            .on_click(cx.listener(move |this, _, window, cx| {
                this.select_asset_batch_section(section, cx);
                this.focus_asset_batch_navigation(window, cx);
            }))
        }));
        let section_nav = div()
            .debug_selector(|| "asset-batch-navigation".to_string())
            .w(px(nav_width))
            .h_full()
            .min_h_0()
            .flex_none()
            .child(nav.size_full());
        let rows = batch_rows(kind_tab, active_section)
            .iter()
            .filter_map(|row| {
                let visible = row
                    .fields
                    .iter()
                    .filter(|spec| {
                        draft.basis(spec.key) != FieldBasis::Inapplicable
                            || draft.rdp_applicability(spec.key).is_some()
                    })
                    .collect::<Vec<_>>();
                if visible.is_empty() {
                    return None;
                }
                debug_assert!(
                    visible
                        .iter()
                        .all(|spec| spec.key.section() == active_section),
                    "batch row table must agree with BatchFieldKey::section"
                );
                let layout = if viewport.0 < 640. {
                    BatchRowLayout::Stack
                } else {
                    row.layout
                };
                Some(self.asset_batch_row(layout, &visible, cx))
            })
            .collect::<Vec<_>>();
        let rows = if has_connections {
            rows
        } else {
            vec![
                div()
                    .py_8()
                    .text_sm()
                    .text_color(rgb(palette.text_muted))
                    .child(t!("assets.batch.kindEmpty"))
                    .into_any_element(),
            ]
        };
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
                    .pb_0()
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
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .flex()
                    .when(show_section_nav, |this| this.child(section_nav))
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
                    ),
            )
            .when(expanded, |this| {
                this.child(
                    div()
                        .id("asset-batch-impact-details")
                        .h(px((scope_summaries.len() as f32 * 20.).min(96.)))
                        .flex_none()
                        .px_3()
                        .overflow_y_scrollbar()
                        .children(scope_summaries.into_iter().map(
                            |(kind, connections, fields)| {
                                let label = kind
                                    .map(super::options::batch_kind_label)
                                    .unwrap_or_else(|| t!("assets.batch.scopeCommon").into());
                                div().text_xs().child(format!(
                                    "{label}: {}",
                                    t!(
                                        "assets.batch.impactSummary",
                                        connections = connections,
                                        fields = fields
                                    )
                                ))
                            },
                        )),
                )
            })
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
                    .child(
                        div().flex_1().min_w(px(160.)).child(
                            NyaButton::new(
                                "asset-batch-impact",
                                format!("{} {summary}", if expanded { "▾" } else { "▸" }),
                            )
                            .tooltip(summary)
                            .variant(NyaButtonVariant::Ghost)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.start_workspace.asset_batch_mut().toggle_summary();
                                cx.notify();
                            })),
                        ),
                    )
                    .child(
                        NyaButton::new("asset-batch-cancel", t!("common.cancel"))
                            .variant(NyaButtonVariant::Secondary)
                            .on_click(
                                cx.listener(|this, _, _, cx| this.close_asset_batch_edit(cx)),
                            ),
                    )
                    .child(
                        NyaButton::new("asset-batch-save", t!("common.save"))
                            .variant(NyaButtonVariant::Primary)
                            .disabled(apply_disabled)
                            .on_click(cx.listener(|this, _, _, cx| this.save_asset_batch_edit(cx))),
                    ),
            )
            .into_any_element()
    }
    /// One row of the batch form. Fields that the connection editor places on
    /// the same line stay on one line here, with the editor's column ratios.
    fn asset_batch_row(
        &self,
        layout: BatchRowLayout,
        specs: &[&BatchFieldSpec],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match layout {
            BatchRowLayout::Stack => {
                let mut column = div().w_full().min_w_0().flex().flex_col().gap_2();
                for spec in specs {
                    column = column.child(self.asset_batch_mode_row(spec.key, spec.action, cx));
                }
                column.into_any_element()
            }
            BatchRowLayout::Columns | BatchRowLayout::ThreeColumns => {
                let mut row = div().w_full().min_w_0().flex().items_start().gap_3();
                for spec in specs {
                    row = row.child(div().min_w_0().flex_1().child(self.asset_batch_mode_row(
                        spec.key,
                        spec.action,
                        cx,
                    )));
                }
                row.into_any_element()
            }
            BatchRowLayout::HostPort | BatchRowLayout::HostPortWide => {
                let fixed = if layout == BatchRowLayout::HostPort {
                    132.
                } else {
                    150.
                };
                let last = specs.len().saturating_sub(1);
                let mut row = div().w_full().min_w_0().flex().items_start().gap_3();
                for (index, spec) in specs.iter().enumerate() {
                    let cell = self.asset_batch_mode_row(spec.key, spec.action, cx);
                    row = row.child(if index == last {
                        div().w(px(fixed)).flex_none().child(cell)
                    } else {
                        div().min_w_0().flex_1().child(cell)
                    });
                }
                row.into_any_element()
            }
        }
    }

    fn asset_batch_mode_row(
        &self,
        field: BatchFieldKey,
        action: BatchFieldAction,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let draft = self.start_workspace.asset_batch();
        let palette = self.theme_palette();
        let mode = draft.mode(field);
        let editable = field.is_editable();
        let basis = draft.basis(field);
        let dirty = mode != BatchEditMode::Keep;
        let applicability = draft.rdp_applicability(field);
        debug_assert_eq!(action, field.action(), "batch table action drift");
        let control = if editable {
            if field.is_bool() {
                // Indeterminate preserves mixed values until the user makes a choice.
                div()
                    .w_full()
                    .h(px(32.))
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        NyaCheckbox::new(SharedString::from(format!(
                            "asset-batch-bool-{}",
                            field.key()
                        )))
                        .checked(draft.bool_value(field))
                        .indeterminate(
                            basis == FieldBasis::Mixed && !draft.current_bool_is_set(field),
                        )
                        .disabled(draft.saving() || !draft.field_available(field))
                        .on_click(cx.listener(
                            move |this, value: &bool, _, cx| {
                                this.set_asset_batch_bool_value(field, *value, cx)
                            },
                        )),
                    )
                    .when(
                        basis == FieldBasis::Mixed && !draft.current_bool_is_set(field),
                        |this| {
                            this.child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(palette.text_muted))
                                    .child(t!("assets.batch.keepPlaceholder")),
                            )
                        },
                    )
                    .into_any_element()
            } else if batch_field_is_select(field) {
                div()
                    .w_full()
                    .h(px(32.))
                    .children(
                        self.selects
                            .field(&field.select_id())
                            .map(|entity| NyaSelect::new(&entity)),
                    )
                    .into_any_element()
            } else if field == BatchFieldKey::RdpCustomDrivePath {
                let configured = draft.value(field);
                let display = if configured.is_empty() {
                    t!("dialog.rdpCustomDriveSelect").to_string()
                } else {
                    configured.to_string()
                };
                div()
                    .w_full()
                    .h(px(32.))
                    .child(
                        NyaButton::new(
                            SharedString::from(format!(
                                "asset-batch-picker-{}",
                                field.key()
                            )),
                            display,
                        )
                        .form_control()
                        .full_width()
                        .label_left()
                        .icon("icons/conn/folder.svg")
                        .tooltip(t!("dialog.rdpCustomDrivePickPrompt"))
                        .disabled(draft.saving() || !draft.field_available(field))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.prompt_asset_batch_custom_drive_path(cx)
                        })),
                    )
                    .into_any_element()
            } else {
                let multi_line = matches!(
                    field,
                    BatchFieldKey::Description | BatchFieldKey::PostLoginCommand
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
            let text = if basis == FieldBasis::Mixed {
                t!("assets.batch.keepPlaceholder").to_string()
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
        let default_value = field.default_value(
            draft
                .display_kind()
                .and_then(|kind| {
                    crate::models::ConnectionKindTab::ALL
                        .into_iter()
                        .find(|tab| tab.batch_kind() == kind)
                })
                .unwrap_or(crate::models::ConnectionKindTab::Ssh),
        );
        let at_default = default_value.is_some_and(|default| {
            if field.is_bool() {
                draft.bool_value(field).to_string() == default
            } else {
                !dirty && draft.value(field) == default
            }
        });
        let action_button = |id: String, label: String| {
            NyaButton::new(id, label)
                .variant(NyaButtonVariant::Ghost)
                .height(px(22.))
                .text_size(px(12.))
        };
        let mut actions = Vec::new();
        if editable {
            match action {
                BatchFieldAction::ClearPassword => {
                    if basis != FieldBasis::Inapplicable {
                        actions.push(
                            action_button(
                                format!("asset-batch-clear-password-{}", field.key()),
                                t!("assets.batch.clearSavedPassword").to_string(),
                            )
                            .disabled(draft.saving() || !draft.field_available(field))
                            .on_click(
                                cx.listener(|this, _, _, cx| this.clear_asset_batch_password(cx)),
                            )
                            .into_any_element(),
                        );
                    }
                }
                BatchFieldAction::ResetDefault => {
                    actions.push(
                        action_button(
                            format!("asset-batch-reset-{}", field.key()),
                            t!("assets.batch.resetDefault").to_string(),
                        )
                        .disabled(draft.saving() || !draft.field_available(field) || at_default)
                        .tooltip(t!("assets.batch.resetDefaultTooltip").to_string())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.reset_asset_batch_field(field, cx)
                        }))
                        .into_any_element(),
                    );
                }
                BatchFieldAction::None
                    if (basis == FieldBasis::Mixed && !dirty)
                        || matches!(
                            field,
                            BatchFieldKey::Account
                                | BatchFieldKey::Key
                                | BatchFieldKey::Otp
                                | BatchFieldKey::Proxy
                                | BatchFieldKey::ProxyJump
                        ) =>
                {
                    actions.push(
                        action_button(
                            format!("asset-batch-clear-{}", field.key()),
                            t!("assets.batch.clearField").to_string(),
                        )
                        .disabled(draft.saving() || !draft.field_available(field))
                        .tooltip(t!("assets.batch.clearFieldTooltip").to_string())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.reset_asset_batch_field(field, cx)
                        }))
                        .into_any_element(),
                    );
                }
                BatchFieldAction::None => {}
            }
            if dirty {
                actions.push(
                    action_button(
                        format!("asset-batch-undo-{}", field.key()),
                        t!("assets.batch.undoEdit").to_string(),
                    )
                    .disabled(draft.saving())
                    .tooltip(t!("assets.batch.undoEditTooltip").to_string())
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.undo_asset_batch_field(field, cx)),
                    )
                    .into_any_element(),
                );
            }
        }
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
                    .items_center()
                    .gap_1()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .child(t!(field.label_key())),
                    )
                    .children(actions),
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
            .when(
                field == BatchFieldKey::Password && mode == BatchEditMode::Clear,
                |this| {
                    this.child(
                        div()
                            .text_xs()
                            .text_color(rgb(palette.text_muted))
                            .child(t!("assets.batch.passwordCleared")),
                    )
                },
            )
            .when(
                editable && mode == BatchEditMode::Set && !field.is_bool(),
                |this| {
                    let empty = if field == BatchFieldKey::Password {
                        draft.replacement_password().is_empty()
                    } else {
                        draft.value(field).trim().is_empty()
                    };
                    this.when(empty && field != BatchFieldKey::Password, |this| {
                        this.child(
                            div().text_xs().text_color(rgb(palette.text_muted)).child(
                                t!("assets.batch.errorUseClear", field = t!(field.label_key()))
                                    .to_string(),
                            ),
                        )
                    })
                },
            )
            .into_any_element()
    }
}
