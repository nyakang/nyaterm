use crate::features::NyaTermApp;
use crate::models::TransferEditorState;
use gpui::{Context, Entity, Window};
use nyaterm_core::file_encoding::{
    FileEncoding, FileEncodingError, FileReadEncoding, FileTextFormat,
};
use nyaterm_ui::{NyaDropdownMenu, NyaMenuItem};
use rust_i18n::t;

pub(in crate::features) fn encoding_error_message(error: &anyhow::Error) -> String {
    let Some(error) = error.downcast_ref::<FileEncodingError>() else {
        return error.to_string();
    };
    match error {
        FileEncodingError::SelectionRequired => t!("fileEncoding.selectionRequired"),
        FileEncodingError::InvalidSequence => t!("fileEncoding.invalidSequence"),
        FileEncodingError::BomConflict => t!("fileEncoding.bomConflict"),
        FileEncodingError::UnsupportedUtf32 => t!("fileEncoding.unsupportedUtf32"),
        FileEncodingError::InvalidBom => t!("fileEncoding.invalidBom"),
        FileEncodingError::Binary => t!("fileEncoding.binary"),
        FileEncodingError::Unrepresentable => t!("fileEncoding.unrepresentable"),
        FileEncodingError::TooLarge => t!("fileEncoding.tooLarge"),
        FileEncodingError::NonRoundTrip => t!("fileEncoding.nonRoundTrip"),
    }
    .to_string()
}

pub(super) fn editor_encoding_menu(
    app: Entity<NyaTermApp>,
    document: &TransferEditorState,
) -> NyaDropdownMenu {
    let label = if document.revision.is_none() {
        t!("fileEncoding.choose").to_string()
    } else if document.encoding.target != document.encoding.original {
        t!(
            "fileEncoding.pending",
            encoding = document.encoding.target.label()
        )
        .to_string()
    } else {
        document.encoding.target.label()
    };
    let id = document.id.clone();
    NyaDropdownMenu::new("editor-file-encoding")
        .label(label)
        .disabled(document.loading || document.saving)
        .items_dynamic(move |_, cx| {
            let Some(document) = app.read(cx).transfer.editor_tab_snapshot(&id) else {
                return Vec::new();
            };
            let selections = std::iter::once(FileReadEncoding::Auto).chain(
                FileEncoding::ALL
                    .into_iter()
                    .map(FileReadEncoding::Explicit),
            );
            let reopen = selections
                .map(|selection| {
                    let label = match selection {
                        FileReadEncoding::Auto => t!("fileEncoding.auto").to_string(),
                        FileReadEncoding::Explicit(encoding) => encoding.label().to_string(),
                    };
                    let app = app.clone();
                    let id = id.clone();
                    NyaMenuItem::action(label)
                        .checked(document.encoding.read == selection)
                        .on_click(move |_, window, cx| {
                            app.update(cx, |app, cx| {
                                app.reopen_editor_with_encoding(&id, selection, false, window, cx)
                            });
                        })
                })
                .collect();
            let convert = FileTextFormat::all()
                .map(|format| {
                    let app = app.clone();
                    let id = id.clone();
                    NyaMenuItem::action(format.label())
                        .checked(document.encoding.target == format)
                        .disabled(
                            document.revision.is_none()
                                || (document.encoding.non_round_trip
                                    && format.encoding != FileEncoding::Utf8),
                        )
                        .on_click(move |_, _, cx| {
                            app.update(cx, |app, cx| {
                                app.transfer.set_editor_save_format(&id, format);
                                cx.notify();
                            });
                        })
                })
                .collect();
            vec![
                NyaMenuItem::submenu(t!("fileEncoding.reopen").to_string(), reopen),
                NyaMenuItem::submenu(t!("fileEncoding.convert").to_string(), convert),
            ]
        })
}

impl NyaTermApp {
    pub(in crate::features) fn reopen_editor_with_encoding(
        &mut self,
        id: &str,
        selection: FileReadEncoding,
        confirmed: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(document) = self.transfer.editor_tab_snapshot(id) else {
            return;
        };
        if document.loading || document.saving {
            return;
        }
        if document.dirty && !confirmed {
            let id = id.to_string();
            self.open_confirm_dialog_with_cancel(
                (
                    t!("fileEditor.reloadDirtyTitle").to_string(),
                    t!("fileEditor.reloadDirtyDesc").to_string(),
                    t!("fileEditor.discardAndReload").to_string(),
                    true,
                    move |app: &mut Self, window: &mut Window, cx: &mut Context<Self>| {
                        app.reopen_editor_with_encoding(&id, selection, true, window, cx);
                        true
                    },
                    |_, _| {},
                ),
                window,
                cx,
            );
            return;
        }
        self.transfer.set_editor_read_encoding(id, selection);
        self.reload_editor_document(id, true, window, cx);
    }
}
