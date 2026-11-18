//! The TFTP configuration window layout.
//!
//! Renders from draft snapshots only; the only IO is the deferred directory
//! picker (in the parent module) and the store save submitted on confirmation.

use gpui::{AnyElement, Context, IntoElement, div, prelude::*, px, rgb};
use nyaterm_core::DEFAULT_TFTP_BIND_ADDRESS;
use nyaterm_ui::{NyaButton, NyaButtonVariant, NyaScrollable, NyaSelectOption, NyaSwitch};
use rust_i18n::t;

use crate::features::NyaTermApp;
use crate::features::networktools::common::form::{
    caption_row, field_with_caption, field_with_error, log_view, plain_caption, section,
    section_with_hint, switch_row,
};
use crate::features::networktools::common::ids;
use crate::features::text_inputs::TextInputSetup;

use super::{KIND, config_input_id, validation};

impl NyaTermApp {
    pub(in crate::features) fn tftp_config_window_view(
        &mut self,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = self.theme_palette();
        let draft = self.networktools.tftp_draft();
        let status_message = self.networktools.status_message(KIND).map(str::to_string);
        let log_lines = self.networktools.log_lines(KIND).to_vec();
        let port_value = self
            .config_input_value("port", cx)
            .unwrap_or_else(|| draft.port.to_string());
        let root_value = self
            .config_input_value("root", cx)
            .unwrap_or_else(|| draft.root_dir.clone());
        let block_value = self
            .config_input_value("block-size", cx)
            .unwrap_or_else(|| draft.block_size.to_string());
        let timeout_value = self
            .config_input_value("timeout", cx)
            .unwrap_or_else(|| draft.timeout_secs.to_string());
        let concurrency_value = self
            .config_input_value("concurrency", cx)
            .unwrap_or_else(|| draft.max_concurrency.to_string());
        let root_probe = self.networktools.root_dir_probe(KIND, root_value.trim());
        let errors = validation::field_errors(
            &draft.bind_address,
            &port_value,
            &root_value,
            &block_value,
            &timeout_value,
            &concurrency_value,
            root_probe,
        );

        let bind_options = self.networktools.bind_options(KIND).to_vec();
        let selected_bind = if draft.bind_address.trim().is_empty() {
            DEFAULT_TFTP_BIND_ADDRESS.to_string()
        } else {
            draft.bind_address.trim().to_string()
        };
        let bind_select = self
            .form_select_control(
                config_input_id("bind"),
                bind_options
                    .iter()
                    .map(|option| {
                        NyaSelectOption::new(option.value.clone(), option.value.clone())
                            .subtitle(option.subtitle.clone())
                    })
                    .collect(),
                Some(selected_bind),
                false,
                cx,
            )
            .into_any_element();
        let port_input = self
            .text_input_box(
                config_input_id("port"),
                &draft.port.to_string(),
                TextInputSetup::default(),
                cx,
            )
            .into_any_element();
        let root_input = self
            .text_input_box(
                config_input_id("root"),
                &draft.root_dir,
                TextInputSetup::default(),
                cx,
            )
            .into_any_element();
        let block_input = self
            .text_input_box(
                config_input_id("block-size"),
                &draft.block_size.to_string(),
                TextInputSetup::default(),
                cx,
            )
            .into_any_element();
        let timeout_input = self
            .text_input_field(
                config_input_id("timeout"),
                t!("networktools.common.field.timeout"),
                &draft.timeout_secs.to_string(),
                TextInputSetup::default(),
                cx,
            )
            .into_any_element();
        let concurrency_input = self
            .text_input_field(
                config_input_id("concurrency"),
                t!("networktools.common.field.concurrency"),
                &draft.max_concurrency.to_string(),
                TextInputSetup::default(),
                cx,
            )
            .into_any_element();

        let root_row = div()
            .flex()
            .items_end()
            .gap_2()
            .child(div().flex_1().min_w_0().child(root_input))
            .child(
                NyaButton::new(
                    ids::config_element(KIND, "browse"),
                    t!("networktools.common.action.browse"),
                )
                .form_control()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.prompt_tftp_root_dir(cx);
                })),
            );

        let address_row = div()
            .flex()
            .items_start()
            .gap_3()
            .child(div().flex_1().min_w_0().child(field_with_caption(
                plain_caption(
                    t!("networktools.common.field.bindAddress").to_string(),
                    palette,
                ),
                bind_select,
                errors.common.bind.clone(),
                palette,
            )))
            .child(div().w(px(120.)).flex_none().child(field_with_caption(
                caption_row(
                    t!("networktools.common.field.port").to_string(),
                    t!("networktools.common.hint.firewall").to_string(),
                    ids::config_element(KIND, "info.port"),
                    palette,
                ),
                port_input,
                errors.common.port.clone(),
                palette,
            )));

        let basic_section = section(
            t!("networktools.common.section.basic").to_string(),
            palette,
            vec![
                address_row.into_any_element(),
                field_with_caption(
                    caption_row(
                        t!("networktools.common.field.rootDir").to_string(),
                        t!("networktools.common.hint.security").to_string(),
                        ids::config_element(KIND, "info.root"),
                        palette,
                    ),
                    root_row.into_any_element(),
                    errors.common.root.clone(),
                    palette,
                ),
            ],
        );
        let limits_section = section(
            t!("networktools.common.section.limits").to_string(),
            palette,
            vec![
                div()
                    .flex()
                    .items_start()
                    .gap_3()
                    .child(div().flex_1().min_w_0().child(field_with_caption(
                        caption_row(
                            t!("networktools.tftp.field.blockSize").to_string(),
                            t!("networktools.tftp.hint.blockSize").to_string(),
                            ids::config_element(KIND, "info.block-size"),
                            palette,
                        ),
                        block_input,
                        errors.block.clone(),
                        palette,
                    )))
                    .child(div().flex_1().min_w_0().child(field_with_error(
                        timeout_input,
                        errors.common.timeout.clone(),
                        palette,
                    )))
                    .child(div().flex_1().min_w_0().child(field_with_error(
                        concurrency_input,
                        errors.common.concurrency.clone(),
                        palette,
                    )))
                    .into_any_element(),
            ],
        );

        // The upload consequence is a standing subtitle next to the title, in
        // the same muted style as the other secondary captions in this window.
        let upload_label = div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(palette.text))
                    .child(t!("networktools.tftp.field.allowUpload").to_string()),
            )
            .child(
                div()
                    .min_w_0()
                    .text_xs()
                    .text_color(rgb(palette.text_muted))
                    .child(t!("networktools.tftp.hint.upload").to_string()),
            )
            .into_any_element();
        let upload_row = switch_row(
            upload_label,
            NyaSwitch::new(ids::config_element(KIND, "upload"))
                .checked(draft.allow_upload)
                .on_click(cx.listener(|this, value: &bool, _, cx| {
                    this.networktools.tftp_draft_mut().allow_upload = *value;
                    cx.notify();
                })),
        );
        let overwrite_row = switch_row(
            div()
                .text_sm()
                .text_color(rgb(palette.text))
                .child(t!("networktools.tftp.field.allowOverwrite").to_string())
                .into_any_element(),
            NyaSwitch::new(ids::config_element(KIND, "overwrite"))
                .checked(draft.allow_overwrite)
                .on_click(cx.listener(|this, value: &bool, _, cx| {
                    this.networktools.tftp_draft_mut().allow_overwrite = *value;
                    cx.notify();
                })),
        );
        let permissions_section = section(
            t!("networktools.common.section.permissions").to_string(),
            palette,
            vec![upload_row, overwrite_row],
        );

        let log_section = section_with_hint(
            t!("networktools.common.log.title").to_string(),
            Some(t!("networktools.common.log.todayHint").to_string()),
            palette,
            vec![log_view(
                &log_lines,
                ids::config_element(KIND, "log"),
                palette,
            )],
        );

        // The running state lives in the panel only; this window shows edits and
        // any validation or start error for this service.
        let mut body = Vec::new();
        if let Some(message) = status_message {
            body.push(
                div()
                    .text_xs()
                    .text_color(rgb(palette.danger))
                    .child(message)
                    .into_any_element(),
            );
        }
        body.push(basic_section);
        body.push(limits_section);
        body.push(permissions_section);
        body.push(log_section);

        let footer = div()
            .flex_none()
            .border_t_1()
            .border_color(rgb(palette.border))
            .p_3()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .child(
                div()
                    .flex_1()
                    .min_w(px(160.))
                    .text_xs()
                    .text_color(rgb(palette.text_muted))
                    .child(t!("networktools.common.status.saveHint").to_string()),
            )
            .child(
                NyaButton::new(
                    ids::config_element(KIND, "reset"),
                    t!("networktools.common.action.reset"),
                )
                .form_control()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.reset_tftp_config_draft(cx);
                })),
            )
            .child(
                NyaButton::new(ids::config_element(KIND, "cancel"), t!("common.cancel"))
                    .form_control()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.end_networktools_config_session(KIND, true, cx);
                    })),
            )
            .child(
                NyaButton::new(
                    ids::config_element(KIND, "save"),
                    if self.networktools.is_running(KIND) {
                        t!("networktools.common.status.saveAndRestart")
                    } else {
                        t!("common.save")
                    },
                )
                .form_control()
                .variant(NyaButtonVariant::Primary)
                .disabled(errors.any())
                .on_click(cx.listener(|this, _, _, cx| {
                    this.save_networktools_config(KIND, cx);
                })),
            );

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div().flex_1().min_h_0().child(
                    div()
                        .id(ids::config_element(KIND, "scroll"))
                        .size_full()
                        .overflow_y_scrollbar()
                        .p_4()
                        .flex()
                        .flex_col()
                        .gap_4()
                        .children(body),
                ),
            )
            .child(footer)
            .into_any_element()
    }
}
