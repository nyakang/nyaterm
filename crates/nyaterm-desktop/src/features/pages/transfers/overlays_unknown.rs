use super::{remote_file_name, transfer_dialog_width};
use crate::features::NyaTermApp;
use crate::models::TransferUnknownFileState;
use gpui::{Context, ParentElement as _, Styled as _, Window, div, rgb};
use nyaterm_files::sftp::{SftpFileEntry, SftpFileType};
use nyaterm_ui::{NyaButton, NyaButtonVariant, NyaDialogWindowExt};
use rust_i18n::t;

impl NyaTermApp {
    pub(in crate::features) fn open_transfer_unknown_file_component_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let palette = self.theme_palette();
        let state =
            self.transfer
                .unknown_file_dialog()
                .cloned()
                .unwrap_or(TransferUnknownFileState {
                    session_id: None,
                    entry: SftpFileEntry {
                        name: String::new(),
                        path: String::new(),
                        file_type: SftpFileType::File,
                        size: None,
                        permissions: None,
                        owner: String::new(),
                        group: String::new(),
                        modified_at: None,
                        raw_path_token: None,
                        symlink_target_is_directory: false,
                    },
                });
        let name = if state.entry.name.trim().is_empty() {
            remote_file_name(&state.entry.path)
        } else {
            state.entry.name.clone()
        };
        let title = t!("fileExplorer.unknownFileTypeTitle").to_string();
        let description = t!("fileExplorer.unknownFileTypeDesc", name = name);
        let cancel_label = t!("common.cancel").to_string();
        let internal_label = t!("fileExplorer.unknownFileTypeOpenInternal").to_string();
        let external_label = t!("fileExplorer.unknownFileTypeOpenExternal").to_string();
        let width = transfer_dialog_width(self.shell.viewport_size().0, 512.);
        let app = cx.weak_entity();

        window.open_nya_dialog(cx, move |dialog, _, _| {
            let cancel_app = app.clone();
            let internal_app = app.clone();
            let external_app = app.clone();
            let close_app = app.clone();
            dialog
                .title(title.clone())
                .width(width)
                .content(
                    div()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap_4()
                        .child(
                            div()
                                .text_sm()
                                .text_color(rgb(palette.text_muted))
                                .child(description.clone()),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .justify_end()
                                .gap_2()
                                .child(
                                    NyaButton::new("transfer-unknown-cancel", cancel_label.clone())
                                        .small()
                                        .on_click(move |_, window, cx| {
                                            let _ = cancel_app.update(cx, |app, cx| {
                                                app.cancel_transfer_unknown_file(cx);
                                            });
                                            window.close_nya_dialog(cx);
                                        }),
                                )
                                .child(
                                    NyaButton::new(
                                        "transfer-unknown-internal",
                                        internal_label.clone(),
                                    )
                                    .small()
                                    .on_click(
                                        move |_, window, cx| {
                                            let _ = internal_app.update(cx, |app, cx| {
                                                app.open_unknown_transfer_file_internal(window, cx);
                                            });
                                            window.close_nya_dialog(cx);
                                        },
                                    ),
                                )
                                .child(
                                    NyaButton::new(
                                        "transfer-unknown-external",
                                        external_label.clone(),
                                    )
                                    .small()
                                    .variant(NyaButtonVariant::Primary)
                                    .on_click(
                                        move |_, window, cx| {
                                            let _ = external_app.update(cx, |app, cx| {
                                                app.open_unknown_transfer_file_external(window, cx);
                                            });
                                            window.close_nya_dialog(cx);
                                        },
                                    ),
                                ),
                        ),
                )
                .on_close(move |_, _, cx| {
                    let _ = close_app.update(cx, |app, cx| {
                        app.cancel_transfer_unknown_file(cx);
                    });
                })
        });
    }
}
