use rust_i18n::t;

use gpui::{AnyElement, Context, IntoElement, ParentElement as _, Styled as _, div, px, rgb};
use nyaterm_ui::NyaCheckbox;

use crate::features::{NyaTermApp, shell::gpui_code_font_family, text_inputs::TextInputSetup};
use crate::models::{
    TransferNewFileState, TransferNewFolderState, TransferNewSymlinkState,
    TransferPermissionTarget, TransferSymlinkField,
};
use crate::theme::ThemePalette;

use super::{
    format_permissions_octal, parse_transfer_mode, symlink_input_row, valid_remote_child_name,
};

mod create_permissions;
#[cfg(test)]
mod tests;

impl NyaTermApp {
    pub(in crate::features) fn transfer_new_folder_dialog_content(
        &mut self,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = self.theme_palette();
        let state = self
            .transfer
            .new_folder_dialog()
            .cloned()
            .unwrap_or(TransferNewFolderState {
                parent_path: String::new(),
                value: String::new(),
                mode: 0o755,
                open_after_create: false,
            });
        let name_invalid =
            !state.value.trim().is_empty() && !valid_remote_child_name(state.value.trim());
        let name_input = self
            .text_input_box(
                "transfer.new-folder.name",
                &state.value,
                TextInputSetup::placeholder(t!("fileExplorer.newFolderNamePlaceholder")),
                cx,
            )
            .into_any_element();
        let app = cx.weak_entity();

        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(create_name_row(
                palette,
                t!("fileExplorer.newFolderName"),
                name_invalid,
                name_input,
            ))
            .child(self.transfer_create_permissions(
                palette,
                state.mode,
                TransferPermissionTarget::NewFolder,
                cx,
            ))
            .child(
                create_option_section(palette).child(
                    NyaCheckbox::new("transfer-new-folder-open-after")
                        .checked(state.open_after_create)
                        .label(t!("fileExplorer.openAfterCreateFolder"))
                        .on_click(move |_, _, cx| {
                            let _ = app.update(cx, |app, cx| {
                                app.transfer.toggle_new_folder_open_after_create();
                                cx.notify();
                            });
                        }),
                ),
            )
            .into_any_element()
    }

    pub(in crate::features) fn transfer_new_file_dialog_content(
        &mut self,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = self.theme_palette();
        let state = self
            .transfer
            .new_file_dialog()
            .cloned()
            .unwrap_or(TransferNewFileState {
                parent_path: String::new(),
                value: String::new(),
                mode: 0o644,
                open_after_create: false,
            });
        let name_invalid =
            !state.value.trim().is_empty() && !valid_remote_child_name(state.value.trim());
        let name_input = self
            .text_input_box(
                "transfer.new-file.name",
                &state.value,
                TextInputSetup::placeholder(t!("fileExplorer.newFileNamePlaceholder")),
                cx,
            )
            .into_any_element();
        let app = cx.weak_entity();

        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(create_name_row(
                palette,
                t!("fileExplorer.newFileName"),
                name_invalid,
                name_input,
            ))
            .child(self.transfer_create_permissions(
                palette,
                state.mode,
                TransferPermissionTarget::NewFile,
                cx,
            ))
            .child(
                create_option_section(palette).child(
                    NyaCheckbox::new("transfer-new-file-open-after")
                        .checked(state.open_after_create)
                        .label(t!("fileExplorer.openAfterCreateFile"))
                        .on_click(move |_, _, cx| {
                            let _ = app.update(cx, |app, cx| {
                                app.transfer.toggle_new_file_open_after_create();
                                cx.notify();
                            });
                        }),
                ),
            )
            .into_any_element()
    }

    pub(in crate::features) fn transfer_new_symlink_dialog_content(
        &mut self,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = self.theme_palette();
        let state =
            self.transfer
                .new_symlink_dialog()
                .cloned()
                .unwrap_or(TransferNewSymlinkState {
                    parent_path: String::new(),
                    name: String::new(),
                    target: String::new(),
                    focused_field: TransferSymlinkField::Name,
                });
        let name_invalid =
            !state.name.trim().is_empty() && !valid_remote_child_name(state.name.trim());
        let name_input = self
            .text_input_box(
                "transfer.new-symlink.name",
                &state.name,
                TextInputSetup::placeholder(t!("fileExplorer.symlinkName")),
                cx,
            )
            .into_any_element();
        let target_input = self
            .text_input_box(
                "transfer.new-symlink.target",
                &state.target,
                TextInputSetup::placeholder("/path/to/target"),
                cx,
            )
            .into_any_element();

        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(symlink_input_row(
                palette,
                t!("fileExplorer.symlinkName"),
                name_invalid,
                name_input,
            ))
            .child(symlink_input_row(
                palette,
                t!("fileExplorer.symlinkTarget"),
                false,
                target_input,
            ))
            .into_any_element()
    }

    pub(in crate::features) fn transfer_permission_grid(
        &self,
        palette: ThemePalette,
        mode: u32,
        target: TransferPermissionTarget,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let app = cx.weak_entity();
        let toggle = move |id: &str, label: &str, mask: u32| {
            let app = app.clone();
            let checkbox = NyaCheckbox::new(id.to_string())
                .checked(mode & mask != 0)
                .on_click(move |_, _, cx| {
                    let _ = app.update(cx, |app, cx| {
                        app.toggle_transfer_permission_bit(target, mask, cx);
                    });
                });
            let checkbox = if label.is_empty() {
                checkbox
            } else {
                checkbox.label(label.to_string())
            };
            div()
                .w(px(if label.is_empty() { 42. } else { 72. }))
                .child(checkbox)
        };

        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(permission_header(palette, t!("fileExplorer.special")))
            .child(
                permission_row(palette, t!("fileExplorer.permUser"))
                    .child(toggle("transfer-perm-user-r", "", 0o400))
                    .child(toggle("transfer-perm-user-w", "", 0o200))
                    .child(toggle("transfer-perm-user-x", "", 0o100))
                    .child(toggle("transfer-perm-user-special", "UID", 0o4000)),
            )
            .child(
                permission_row(palette, t!("fileExplorer.permGroup"))
                    .child(toggle("transfer-perm-group-r", "", 0o040))
                    .child(toggle("transfer-perm-group-w", "", 0o020))
                    .child(toggle("transfer-perm-group-x", "", 0o010))
                    .child(toggle("transfer-perm-group-special", "GID", 0o2000)),
            )
            .child(
                permission_row(palette, t!("fileExplorer.permOther"))
                    .child(toggle("transfer-perm-other-r", "", 0o004))
                    .child(toggle("transfer-perm-other-w", "", 0o002))
                    .child(toggle("transfer-perm-other-x", "", 0o001))
                    .child(toggle(
                        "transfer-perm-other-special",
                        &t!("fileExplorer.permSticky"),
                        0o1000,
                    )),
            )
            .child(
                div()
                    .mt_1()
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_xs()
                    .text_color(rgb(palette.text_muted))
                    .child(t!("fileExplorer.octal"))
                    .child(
                        div()
                            .font_family(gpui_code_font_family())
                            .text_color(rgb(palette.text))
                            .child(format_permissions_octal(mode)),
                    ),
            )
    }

    fn toggle_transfer_permission_bit(
        &mut self,
        target: TransferPermissionTarget,
        bit: u32,
        cx: &mut Context<Self>,
    ) {
        match target {
            TransferPermissionTarget::NewFolder => {
                self.transfer.toggle_new_folder_mode_bit(bit);
            }
            TransferPermissionTarget::NewFile => {
                self.transfer.toggle_new_file_mode_bit(bit);
            }
            TransferPermissionTarget::Properties => {
                if let Some(state) = self.transfer.properties_dialog() {
                    let current = parse_transfer_mode(&state.mode_value)
                        .or(state.entry.permissions)
                        .unwrap_or(0o644);
                    self.transfer
                        .set_properties_mode_value(format_permissions_octal(current ^ bit));
                }
                self.sync_transfer_properties_inputs(cx);
            }
        }
        cx.notify();
    }
}

fn create_name_row(
    palette: ThemePalette,
    label: impl IntoElement,
    invalid: bool,
    input: impl IntoElement,
) -> impl IntoElement {
    div()
        .w_full()
        .min_w_0()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .text_sm()
                .text_color(rgb(if invalid {
                    palette.danger
                } else {
                    palette.text_muted
                }))
                .child(label),
        )
        .child(div().w_full().min_w_0().child(input))
}

fn create_option_section(palette: ThemePalette) -> gpui::Div {
    div()
        .w_full()
        .pt_4()
        .border_t_1()
        .border_color(rgb(palette.border))
}

fn permission_header(palette: ThemePalette, special: impl IntoElement) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap_1()
        .text_xs()
        .text_color(rgb(palette.text_dimmed))
        .child(div().w(px(48.)).flex_none().child(""))
        .child(div().w(px(42.)).text_center().child("R"))
        .child(div().w(px(42.)).text_center().child("W"))
        .child(div().w(px(42.)).text_center().child("X"))
        .child(div().w(px(72.)).text_center().child(special))
}

fn permission_row(palette: ThemePalette, label: impl IntoElement) -> gpui::Div {
    div().flex().items_center().gap_1().child(
        div()
            .w(px(48.))
            .flex_none()
            .text_xs()
            .text_color(rgb(palette.text_muted))
            .child(label),
    )
}
