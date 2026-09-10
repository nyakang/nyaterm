use rust_i18n::t;

use gpui::{AnyElement, Context, IntoElement, div, prelude::*, px, rgb};
use nyaterm_core::truncate_preview;
use nyaterm_transport::{SftpFileProperties, SftpFileType};
use nyaterm_ui::{NyaCheckbox, NyaScrollable};

use crate::features::{NyaTermApp, text_inputs::TextInputSetup, transfers::format_file_size};
use crate::models::{TransferPermissionTarget, TransferPropertiesState};
use crate::theme::ThemePalette;

use super::{
    format_owner_group, format_sftp_modified, parse_transfer_mode, property_row,
    property_section_heading, remote_parent_path,
};

impl NyaTermApp {
    pub(in crate::features) fn transfer_properties_dialog_content(
        &mut self,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = self.theme_palette();
        let Some(state) = self.transfer.properties_dialog().cloned() else {
            return div().into_any_element();
        };
        let entry = state.entry.clone();
        let properties = state.properties.clone();
        let loading = properties.is_none();
        let editable = state
            .session_id
            .as_deref()
            .and_then(|session_id| self.session.file_browser_backend_for_session(session_id))
            == Some(nyaterm_transport::FileBrowserBackendKind::Remote);
        let property_mode = parse_transfer_mode(&state.mode_value)
            .or(entry.permissions)
            .unwrap_or(0o644);
        let owner_input = self
            .text_input_box(
                "transfer.properties.owner",
                &state.owner_value,
                TextInputSetup::placeholder(t!("fileExplorer.owner")),
                cx,
            )
            .into_any_element();
        let group_input = self
            .text_input_box(
                "transfer.properties.group",
                &state.group_value,
                TextInputSetup::placeholder(t!("fileExplorer.group")),
                cx,
            )
            .into_any_element();
        let mode_input = self
            .text_input_box(
                "transfer.properties.mode",
                &state.mode_value,
                TextInputSetup::placeholder("0644"),
                cx,
            )
            .into_any_element();
        let symlink = properties
            .as_ref()
            .is_some_and(|properties| properties.file_type == SftpFileType::Symlink);
        let link_input = self
            .text_input_box(
                "transfer.properties.symlink-target",
                &state.symlink_target_value,
                TextInputSetup::placeholder(t!("fileExplorer.symlinkTarget")),
                cx,
            )
            .into_any_element();
        let app = cx.weak_entity();

        div()
            .id("transfer-properties-dialog-content")
            .max_h(px(560.))
            .overflow_y_scrollbar()
            .when_some(properties, |this, properties| {
                this.child(properties_summary(palette, &state, &properties))
                    .when(symlink, |this| {
                        this.child(property_input(
                            palette,
                            t!("fileExplorer.symlinkTarget"),
                            link_input,
                        ))
                    })
                    .when(editable, |this| {
                        this.child(
                            div()
                                .mt_5()
                                .pt_5()
                                .border_t_1()
                                .border_color(rgb(palette.border))
                                .child(property_section_heading(
                                    palette,
                                    t!("fileExplorer.ownership"),
                                )),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .when(symlink, |this| this.hidden())
                            .flex_col()
                            .gap_3()
                            .child(property_input(
                                palette,
                                t!("fileExplorer.owner"),
                                owner_input,
                            ))
                            .child(property_input(
                                palette,
                                t!("fileExplorer.group"),
                                group_input,
                            ))
                            .child(
                                div()
                                    .mt_1()
                                    .pt_4()
                                    .border_t_1()
                                    .border_color(rgb(palette.border))
                                    .child(property_section_heading(
                                        palette,
                                        t!("fileExplorer.permissions"),
                                    )),
                            )
                            .child(self.transfer_permission_grid(
                                palette,
                                property_mode,
                                TransferPermissionTarget::Properties,
                                cx,
                            ))
                            .child(property_input(
                                palette,
                                t!("fileExplorer.octal"),
                                mode_input,
                            ))
                            .when(entry.is_directory(), |this| {
                                this.child(
                                    NyaCheckbox::new("transfer-properties-recursive")
                                        .checked(state.recursive)
                                        .label(t!("fileExplorer.applyRecursively"))
                                        .disabled(state.saving)
                                        .on_click(move |_, _, cx| {
                                            let _ = app.update(cx, |app, cx| {
                                                app.transfer.toggle_properties_recursive();
                                                cx.notify();
                                            });
                                        }),
                                )
                            }),
                    )
            })
            .when(loading, |this| {
                this.child(
                    div()
                        .min_h(px(220.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_xs()
                        .text_color(rgb(palette.text_muted))
                        .child(t!("fileExplorer.loading")),
                )
            })
            .when_some(state.error, |this, error| {
                this.child(
                    div()
                        .mt_3()
                        .rounded_sm()
                        .bg(rgb(0x351216))
                        .px_3()
                        .py_2()
                        .text_xs()
                        .text_color(rgb(0xfca5a5))
                        .child(error),
                )
            })
            .when(state.saving, |this| {
                this.child(
                    div()
                        .mt_3()
                        .text_xs()
                        .text_color(rgb(palette.text_muted))
                        .child(t!("common.saving")),
                )
            })
            .into_any_element()
    }
}

fn properties_summary(
    palette: ThemePalette,
    state: &TransferPropertiesState,
    properties: &SftpFileProperties,
) -> impl IntoElement {
    let entry = &state.entry;
    let entry_type_label = match entry.file_type {
        SftpFileType::Directory => t!("fileExplorer.folder"),
        SftpFileType::File => t!("fileExplorer.file"),
        SftpFileType::Symlink => t!("fileExplorer.newSymlink"),
        SftpFileType::Other => t!("fileExplorer.special"),
    };
    div()
        .child(property_section_heading(
            palette,
            t!("fileExplorer.general"),
        ))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(10.))
                .child(property_row(
                    palette,
                    t!("fileExplorer.type"),
                    entry_type_label,
                ))
                .child(property_row(
                    palette,
                    t!("fileExplorer.location"),
                    truncate_preview(&remote_parent_path(&entry.path), 76),
                ))
                .child(property_row(
                    palette,
                    t!("fileExplorer.size"),
                    format_file_size(properties.size.or(entry.size)),
                ))
                .child(property_row(
                    palette,
                    t!("fileExplorer.mtime"),
                    format_sftp_modified(properties.modified_at.or(entry.modified_at)),
                ))
                .child(property_row(
                    palette,
                    t!("fileExplorer.atime"),
                    format_sftp_modified(properties.accessed_at),
                ))
                .child(property_row(
                    palette,
                    t!("fileExplorer.owner"),
                    format_owner_group(&properties.owner, properties.uid),
                ))
                .child(property_row(
                    palette,
                    t!("fileExplorer.group"),
                    format_owner_group(&properties.group, properties.gid),
                )),
        )
}

fn property_input(
    palette: ThemePalette,
    label: impl IntoElement,
    input: impl IntoElement,
) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap_3()
        .child(
            div()
                .w(px(88.))
                .flex_none()
                .text_xs()
                .text_color(rgb(palette.text_muted))
                .child(label),
        )
        .child(div().flex_1().min_w_0().child(input))
}
