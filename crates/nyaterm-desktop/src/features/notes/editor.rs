use std::rc::Rc;
use std::time::Duration;

use gpui::{
    App, AppContext as _, Context, Entity, FocusHandle, IntoElement, Render, SharedString,
    Subscription, Task, WeakEntity, Window, div, prelude::*, px, rgb,
};
use nyaterm_core::{NoteDocument, NotesSnapshot};
use nyaterm_store::{StoreDomain, store_request};
use nyaterm_ui::{
    NyaConfirmDialog, NyaDialogFooter, NyaDialogWindowExt as _, NyaDocumentEditor,
    NyaDocumentEditorEvent, NyaDocumentEditorState, NyaDropdownMenu, NyaInput, NyaInputEvent,
    NyaInputShell, NyaInputState, NyaMenuItem, NyaScrollable as _, NyaWindowHandle, nya_root,
};
use rust_i18n::t;

use crate::features::{
    NyaTermApp,
    notes::NotesCatalogEvent,
    view_widgets::{
        ChildWindowChrome, ChildWindowCloseHandler, ChildWindowSpec, child_window_header,
        child_window_options, child_window_root, focus_child_window_shell_if_idle,
        markdown_content_view,
    },
};

const AUTOSAVE_DELAY: Duration = Duration::from_millis(800);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum NoteViewMode {
    Source,
    #[default]
    Split,
    Preview,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SaveStatus {
    Saved,
    Saving,
    Unsaved,
    Failed,
    Conflict,
    Deleted,
    ExternalUpdate,
}

fn editor_blocks_export(dirty: bool, saving: bool, refreshing: bool, status: SaveStatus) -> bool {
    dirty
        || saving
        || refreshing
        || matches!(
            status,
            SaveStatus::Conflict | SaveStatus::Deleted | SaveStatus::ExternalUpdate
        )
}

fn matches_export_revision(note_id: &str, revision: u64, snapshot: &NotesSnapshot) -> bool {
    snapshot
        .notes
        .iter()
        .any(|note| note.id == note_id && note.revision == revision)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExternalNoteAction {
    None,
    Reload,
    Conflict,
    Deleted,
}

fn external_note_action(
    dirty: bool,
    local_revision: u64,
    remote_revision: Option<u64>,
) -> ExternalNoteAction {
    match remote_revision {
        None => ExternalNoteAction::Deleted,
        Some(remote) if remote > local_revision && dirty => ExternalNoteAction::Conflict,
        Some(remote) if remote > local_revision => ExternalNoteAction::Reload,
        Some(_) => ExternalNoteAction::None,
    }
}

fn event_revision_for_note(event: &NotesCatalogEvent, note_id: &str) -> Option<Option<u64>> {
    match event {
        NotesCatalogEvent::NoteUpserted { id, revision } if id == note_id => Some(Some(*revision)),
        NotesCatalogEvent::NodesDeleted { ids } if ids.iter().any(|id| id == note_id) => Some(None),
        NotesCatalogEvent::CatalogReplaced { revisions } => Some(revisions.get(note_id).copied()),
        _ => None,
    }
}

#[derive(Clone, Copy)]
enum MarkdownCommand {
    Paragraph,
    Heading(u8),
    Bold,
    Italic,
    Underline,
    InlineCode,
    Quote,
    Bullet,
    Numbered,
    Table,
    Rule,
    CodeBlock(&'static str),
}

pub(super) struct NoteEditorWindow {
    app: Entity<NyaTermApp>,
    note: NoteDocument,
    editor: Entity<NyaDocumentEditorState>,
    title: Entity<NyaInputState>,
    mode: NoteViewMode,
    status: SaveStatus,
    dirty: bool,
    saving: bool,
    save_again: bool,
    close_requested: bool,
    edit_generation: u64,
    external_refresh_pending: bool,
    debounce: Option<Task<()>>,
    shell_focus: FocusHandle,
    chrome: ChildWindowChrome,
    _editor_subscription: Subscription,
    _title_subscription: Subscription,
    _app_subscription: Subscription,
}

impl NoteEditorWindow {
    pub(super) fn blocks_export(&self) -> bool {
        editor_blocks_export(
            self.dirty,
            self.saving,
            self.external_refresh_pending,
            self.status,
        )
    }

    pub(super) fn matches_export_snapshot(&self, snapshot: &NotesSnapshot) -> bool {
        matches_export_revision(&self.note.id, self.note.revision, snapshot)
    }

    fn new(
        app: Entity<NyaTermApp>,
        note: NoteDocument,
        chrome: ChildWindowChrome,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let editor = cx.new(|cx| NyaDocumentEditorState::new(window, cx, note.markdown.clone()));
        let title = cx.new(|cx| NyaInputState::new(cx, note.title.clone()).max_chars(Some(120)));
        let editor_subscription = cx.subscribe(
            &editor,
            |this, _, event: &NyaDocumentEditorEvent, cx| match event {
                NyaDocumentEditorEvent::Changed(value) => {
                    this.note.markdown = value.clone();
                    this.mark_dirty(cx);
                }
                NyaDocumentEditorEvent::Blurred(value) => {
                    let changed = this.note.markdown != *value;
                    this.note.markdown = value.clone();
                    if changed {
                        this.mark_dirty_without_debounce();
                    }
                    if this.dirty {
                        this.save_now(false, cx);
                    }
                }
                NyaDocumentEditorEvent::Updated => {}
            },
        );
        let title_subscription =
            cx.subscribe(&title, |this, _, event: &NyaInputEvent, cx| match event {
                NyaInputEvent::Changed(value) => {
                    this.note.title = value.clone();
                    this.mark_dirty(cx);
                }
                NyaInputEvent::Blurred(value) | NyaInputEvent::Submitted(value) => {
                    let changed = this.note.title != *value;
                    this.note.title = value.clone();
                    if changed {
                        this.mark_dirty_without_debounce();
                    }
                    if this.dirty {
                        this.save_now(false, cx);
                    }
                }
            });
        let app_subscription = cx.subscribe(&app, |this, _, event: &NotesCatalogEvent, cx| {
            if this.external_refresh_pending || this.saving {
                return;
            }
            let Some(remote_revision) = event_revision_for_note(event, &this.note.id) else {
                return;
            };
            match external_note_action(this.dirty, this.note.revision, remote_revision) {
                ExternalNoteAction::Reload => this.reload(cx),
                ExternalNoteAction::Conflict => {
                    this.status = SaveStatus::Conflict;
                    cx.notify();
                }
                ExternalNoteAction::Deleted => {
                    this.status = SaveStatus::Deleted;
                    cx.notify();
                }
                ExternalNoteAction::None => {}
            }
        });
        Self {
            app,
            note,
            editor,
            title,
            mode: NoteViewMode::Split,
            status: SaveStatus::Saved,
            dirty: false,
            saving: false,
            save_again: false,
            close_requested: false,
            edit_generation: 0,
            external_refresh_pending: false,
            debounce: None,
            shell_focus: cx.focus_handle(),
            chrome,
            _editor_subscription: editor_subscription,
            _title_subscription: title_subscription,
            _app_subscription: app_subscription,
        }
    }

    fn mark_dirty_without_debounce(&mut self) {
        self.dirty = true;
        self.status = SaveStatus::Unsaved;
        self.edit_generation = self.edit_generation.wrapping_add(1);
        if self.saving {
            self.save_again = true;
        }
    }

    fn mark_dirty(&mut self, cx: &mut Context<Self>) {
        self.mark_dirty_without_debounce();
        let generation = self.edit_generation;
        self.debounce = Some(cx.spawn(async move |this: WeakEntity<Self>, cx| {
            cx.background_executor().timer(AUTOSAVE_DELAY).await;
            let _ = this.update(cx, |this, cx| {
                if this.edit_generation == generation {
                    this.save_now(false, cx);
                }
            });
        }));
        cx.notify();
    }

    fn save_now(&mut self, force: bool, cx: &mut Context<Self>) {
        self.debounce = None;
        if self.saving {
            self.save_again = true;
            return;
        }
        if !self.dirty && !force {
            if self.close_requested {
                self.finish_close(cx);
            }
            return;
        }
        self.saving = true;
        self.status = SaveStatus::Saving;
        let generation = self.edit_generation;
        let note = self.note.clone();
        let editor = cx.weak_entity();
        self.app.update(cx, move |app, cx| {
            app.save_note_document(
                note,
                force,
                move |_, result, cx| {
                    let _ = editor.update(cx, |editor, cx| {
                        editor.saving = false;
                        let mut save_again = false;
                        match result {
                            Ok(saved) => {
                                let current_title = editor.note.title.clone();
                                let current_markdown = editor.note.markdown.clone();
                                editor.note = saved;
                                if editor.edit_generation == generation {
                                    editor.dirty = false;
                                    editor.status = SaveStatus::Saved;
                                    if editor.close_requested {
                                        editor.finish_close(cx);
                                        return;
                                    }
                                } else {
                                    editor.note.title = current_title;
                                    editor.note.markdown = current_markdown;
                                    editor.dirty = true;
                                    editor.status = SaveStatus::Unsaved;
                                    editor.save_again = true;
                                }
                            }
                            Err(error) if error.contains("Revision conflict") => {
                                editor.status = SaveStatus::Conflict;
                                editor.dirty = true;
                            }
                            Err(error) if error.contains("does not exist") => {
                                editor.status = SaveStatus::Deleted;
                                editor.dirty = true;
                            }
                            Err(_) => {
                                editor.status = SaveStatus::Failed;
                                editor.dirty = true;
                            }
                        }
                        if editor.save_again
                            && !matches!(editor.status, SaveStatus::Conflict | SaveStatus::Deleted)
                        {
                            editor.save_again = false;
                            save_again = true;
                        }
                        if save_again {
                            let editor = cx.weak_entity();
                            cx.defer(move |cx| {
                                let _ = editor.update(cx, |editor, cx| editor.save_now(false, cx));
                            });
                        }
                        cx.notify();
                    });
                },
                cx,
            );
        });
        cx.notify();
    }

    fn request_close(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.dirty && !self.saving {
            return true;
        }
        self.close_requested = true;
        self.save_now(false, cx);
        false
    }

    fn finish_close(&mut self, cx: &mut Context<Self>) {
        let note_id = self.note.id.clone();
        let app = self.app.clone();
        cx.defer(move |cx| {
            let handle = app.update(cx, |app, cx| {
                let handle = app.notes.take_editor_window(&note_id);
                cx.notify();
                handle
            });
            if let Some(handle) = handle {
                let _ = handle.update(cx, |_, window, _| window.remove_window());
            }
        });
    }

    fn discard_and_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let id = self.note.id.clone();
        self.app.update(cx, |app, cx| {
            app.notes.remove_editor_window(&id);
            cx.notify();
        });
        window.remove_window();
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        if self.external_refresh_pending {
            return;
        }
        self.external_refresh_pending = true;
        self.status = SaveStatus::ExternalUpdate;
        let editor = cx.weak_entity();
        cx.defer(move |cx| {
            let _ = editor.update(cx, |editor, cx| editor.submit_reload(cx));
        });
        cx.notify();
    }

    fn submit_reload(&mut self, cx: &mut Context<Self>) {
        let id = self.note.id.clone();
        let editor_window = cx.weak_entity();
        self.app.update(cx, |app, cx| {
            app.submit_store_request(
                0,
                store_request(StoreDomain::Notes, move |store| store.get_note(&id)),
                move |_, event, cx| {
                    let _ = editor_window.update(cx, |editor, cx| match event.outcome {
                        Ok(Some(note)) => {
                            editor.external_refresh_pending = false;
                            editor.note = note.clone();
                            editor.dirty = false;
                            editor.status = SaveStatus::Saved;
                            editor
                                .title
                                .update(cx, |title, cx| title.set_content_silent(&note.title, cx));
                            editor.editor.update(cx, |surface, cx| {
                                surface.set_content(&note.markdown, cx);
                            });
                            if editor.close_requested {
                                editor.finish_close(cx);
                                return;
                            }
                            cx.notify();
                        }
                        Ok(None) => {
                            editor.external_refresh_pending = false;
                            editor.status = SaveStatus::Deleted;
                        }
                        Err(_) => {
                            editor.external_refresh_pending = false;
                            editor.status = SaveStatus::Failed;
                        }
                    });
                },
                cx,
            );
        });
    }

    fn save_copy(&mut self, cx: &mut Context<Self>) {
        let old_id = self.note.id.clone();
        let parent_id = self.note.parent_id.clone();
        let title = format!("{} ({})", self.note.title, t!("notes.conflictCopySuffix"));
        let markdown = self.note.markdown.clone();
        let editor_window = cx.weak_entity();
        self.status = SaveStatus::Saving;
        self.app.update(cx, |app, cx| {
            app.submit_store_request(
                0,
                store_request(StoreDomain::Notes, move |store| {
                    store.create_note(parent_id, Some(title), Some(markdown))
                }),
                move |app, event, cx| match event.outcome {
                    Ok(note) => {
                        let id = note.id.clone();
                        let revision = note.revision;
                        app.notes
                            .upsert_note(nyaterm_core::NoteSummary::from(&note));
                        app.notes.rekey_editor_window(&old_id, note.id.clone());
                        let _ = editor_window.update(cx, |editor, cx| {
                            editor.note = note.clone();
                            editor.dirty = false;
                            editor.status = SaveStatus::Saved;
                            editor
                                .title
                                .update(cx, |title, cx| title.set_content_silent(&note.title, cx));
                            if editor.close_requested {
                                editor.finish_close(cx);
                                return;
                            }
                            cx.notify();
                        });
                        cx.emit(NotesCatalogEvent::NoteUpserted { id, revision });
                    }
                    Err(_) => {
                        let _ = editor_window.update(cx, |editor, cx| {
                            editor.status = SaveStatus::Failed;
                            cx.notify();
                        });
                    }
                },
                cx,
            );
        });
    }

    fn cancel_close(&mut self, cx: &mut Context<Self>) {
        self.close_requested = false;
        cx.notify();
    }

    fn apply_command(
        &mut self,
        command: MarkdownCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let content = self.editor.read(cx).value(cx);
        let selection = self.editor.read(cx).selected_range(cx);
        let edit = markdown_edit(&content, selection, command);
        self.editor.update(cx, |editor, cx| {
            editor.apply_edit(edit.replacement, edit.selected_after, window, cx)
        });
    }

    fn open_link_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let content = self.editor.read(cx).value(cx);
        let selection = self.editor.read(cx).selected_range(cx);
        let start = floor_char_boundary(&content, selection.start.min(content.len()));
        let end = ceil_char_boundary(&content, selection.end.min(content.len()));
        let label_seed = if start == end {
            t!("notes.linkTextPlaceholder").to_string()
        } else {
            content[start..end].to_string()
        };
        let label = cx.new(|cx| NyaInputState::new(cx, label_seed.clone()));
        let url = cx.new(|cx| {
            NyaInputState::new(cx, "https://").placeholder(t!("notes.linkUrlPlaceholder"))
        });
        let editor = self.editor.clone();
        window.open_nya_dialog(cx, move |dialog, _, _| {
            let confirm_label = label.clone();
            let confirm_url = url.clone();
            let confirm_editor = editor.clone();
            NyaConfirmDialog::new(
                dialog
                    .title(t!("notes.linkDialogTitle").to_string())
                    .width(420.),
                NyaDialogFooter::new(
                    t!("common.cancel").to_string(),
                    t!("common.confirm").to_string(),
                ),
            )
            .content(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .h(px(34.))
                            .child(NyaInputShell::new("note-link-label", &label)),
                    )
                    .child(
                        div()
                            .h(px(34.))
                            .child(NyaInputShell::new("note-link-url", &url)),
                    ),
            )
            .on_confirm(move |_, window, cx| {
                let label = confirm_label.read(cx).value(cx);
                let url = confirm_url.read(cx).value(cx);
                if label.trim().is_empty() || url.trim().is_empty() {
                    return false;
                }
                let replacement = format!("[{}]({})", label.trim(), url.trim());
                let selected_after = start + 1..start + 1 + label.trim().len();
                confirm_editor.update(cx, |editor, cx| {
                    editor.apply_edit(replacement, selected_after, window, cx)
                });
                true
            })
            .on_cancel(|_, _, _| true)
            .into_dialog()
        });
    }
}

impl Render for NoteEditorWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = self.app.read(cx).theme_palette();
        let title = format!("{}{}", if self.dirty { "* " } else { "" }, self.note.title);
        window.set_window_title(&title);
        focus_child_window_shell_if_idle(&self.shell_focus, window, cx);
        let status = match self.status {
            SaveStatus::Saved => t!("notes.saved"),
            SaveStatus::Saving => t!("notes.saving"),
            SaveStatus::Unsaved => t!("notes.unsaved"),
            SaveStatus::Failed => t!("notes.saveFailed"),
            SaveStatus::Conflict => t!("notes.revisionConflict"),
            SaveStatus::Deleted => t!("notes.deletedStatus"),
            SaveStatus::ExternalUpdate => t!("notes.externalUpdate"),
        };
        let close = cx.weak_entity();
        let on_close: ChildWindowCloseHandler = Rc::new(move |window, cx| {
            let should_close = close
                .update(cx, |editor, cx| editor.request_close(cx))
                .unwrap_or(true);
            if should_close {
                window.remove_window();
            }
        });
        let header_close = on_close.clone();
        let editor = self.editor.clone();
        let markdown = self.note.markdown.clone();
        let mode = self.mode;
        let mut body = div().flex_1().min_h_0().min_w_0().flex();
        if matches!(mode, NoteViewMode::Source | NoteViewMode::Split) {
            body = body.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .border_r_1()
                    .border_color(rgb(palette.border))
                    .child(NyaDocumentEditor::new(&editor)),
            );
        }
        if matches!(mode, NoteViewMode::Preview | NoteViewMode::Split) {
            body = body.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .p_4()
                    .overflow_y_scrollbar()
                    .child(markdown_content_view(palette, &markdown)),
            );
        }
        let toolbar = self.toolbar(palette, window, cx);
        let conflict = matches!(
            self.status,
            SaveStatus::Conflict | SaveStatus::Deleted | SaveStatus::Failed
        );
        child_window_root(&self.shell_focus, false, on_close)
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                let modifiers = event.keystroke.modifiers;
                if event.keystroke.key.eq_ignore_ascii_case("s")
                    && (modifiers.platform || modifiers.control)
                {
                    this.save_now(false, cx);
                    cx.stop_propagation();
                }
            }))
            .bg(rgb(palette.bg))
            .text_color(rgb(palette.text))
            .child(child_window_header(
                palette,
                title,
                Some("icons/notes.svg"),
                self.chrome,
                window,
                move |_, window, cx| header_close(window, cx),
            ))
            .child(
                div()
                    .h(px(42.))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .border_b_1()
                    .border_color(rgb(palette.border))
                    .child(
                        div()
                            .w(px(220.))
                            .h(px(30.))
                            .child(NyaInput::new(&self.title)),
                    )
                    .child(toolbar)
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(palette.text_muted))
                            .child(status.to_string()),
                    ),
            )
            .when(conflict, |root| {
                root.child(self.conflict_bar(palette, window, cx))
            })
            .child(body)
            .into_any_element()
    }
}

impl NoteEditorWindow {
    fn toolbar(
        &mut self,
        palette: crate::theme::ThemePalette,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let mut bar = div().flex().items_center().gap_1();
        let undo_editor = self.editor.clone();
        let redo_editor = self.editor.clone();
        bar = bar
            .child(
                div()
                    .id("note-undo")
                    .px_2()
                    .h(px(26.))
                    .flex()
                    .items_center()
                    .rounded_sm()
                    .cursor_pointer()
                    .hover(move |this| this.bg(rgb(palette.hover)))
                    .on_click(move |_, window, cx| {
                        undo_editor.update(cx, |editor, cx| editor.undo(window, cx))
                    })
                    .child("↶"),
            )
            .child(
                div()
                    .id("note-redo")
                    .px_2()
                    .h(px(26.))
                    .flex()
                    .items_center()
                    .rounded_sm()
                    .cursor_pointer()
                    .hover(move |this| this.bg(rgb(palette.hover)))
                    .on_click(move |_, window, cx| {
                        redo_editor.update(cx, |editor, cx| editor.redo(window, cx))
                    })
                    .child("↷"),
            );
        for (label, command) in [
            ("P", MarkdownCommand::Paragraph),
            ("H1", MarkdownCommand::Heading(1)),
            ("H2", MarkdownCommand::Heading(2)),
            ("H3", MarkdownCommand::Heading(3)),
            ("B", MarkdownCommand::Bold),
            ("I", MarkdownCommand::Italic),
            ("U", MarkdownCommand::Underline),
            ("`", MarkdownCommand::InlineCode),
            (">", MarkdownCommand::Quote),
            ("•", MarkdownCommand::Bullet),
            ("1.", MarkdownCommand::Numbered),
            ("▦", MarkdownCommand::Table),
            ("—", MarkdownCommand::Rule),
        ] {
            bar = bar.child(
                div()
                    .id(SharedString::from(format!("note-format-{label}")))
                    .px_2()
                    .h(px(26.))
                    .flex()
                    .items_center()
                    .rounded_sm()
                    .cursor_pointer()
                    .hover(move |this| this.bg(rgb(palette.hover)))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.apply_command(command, window, cx)
                    }))
                    .child(label),
            );
        }
        bar = bar.child(
            div()
                .id("note-format-link")
                .px_2()
                .h(px(26.))
                .flex()
                .items_center()
                .rounded_sm()
                .cursor_pointer()
                .hover(move |this| this.bg(rgb(palette.hover)))
                .on_click(cx.listener(|this, _, window, cx| this.open_link_dialog(window, cx)))
                .child("↗"),
        );
        let editor_window = cx.weak_entity();
        let code_languages = [
            ("Plain text", "text"),
            ("Rust", "rust"),
            ("JavaScript", "javascript"),
            ("TypeScript", "typescript"),
            ("Python", "python"),
            ("Shell", "shell"),
            ("JSON", "json"),
            ("YAML", "yaml"),
        ];
        bar = bar.child(
            NyaDropdownMenu::new("note-format-code-block")
                .label("{}")
                .tooltip(t!("notes.codeBlockLanguage"))
                .items(code_languages.into_iter().map(|(label, language)| {
                    let editor_window = editor_window.clone();
                    NyaMenuItem::action(label).on_click(move |_, window, cx| {
                        let _ = editor_window.update(cx, |editor, cx| {
                            editor.apply_command(MarkdownCommand::CodeBlock(language), window, cx)
                        });
                    })
                })),
        );
        for (label, mode) in [
            ("S", NoteViewMode::Source),
            ("S|P", NoteViewMode::Split),
            ("P", NoteViewMode::Preview),
        ] {
            bar = bar.child(
                div()
                    .id(SharedString::from(format!("note-view-{label}")))
                    .px_2()
                    .h(px(26.))
                    .flex()
                    .items_center()
                    .rounded_sm()
                    .cursor_pointer()
                    .bg(if self.mode == mode {
                        rgb(palette.surface_elevated)
                    } else {
                        rgb(palette.bg)
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.mode = mode;
                        cx.notify();
                    }))
                    .child(label),
            );
        }
        bar.into_any_element()
    }

    fn conflict_bar(
        &mut self,
        palette: crate::theme::ThemePalette,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let mut bar = div()
            .px_3()
            .py_2()
            .flex()
            .items_center()
            .gap_2()
            .bg(rgb(palette.surface_elevated));
        if self.status == SaveStatus::Conflict {
            bar = bar
                .child(t!("notes.revisionConflictDescription").to_string())
                .child(action_button(
                    "note-reload",
                    t!("notes.reload"),
                    palette,
                    cx.listener(|this, _, _, cx| this.reload(cx)),
                ))
                .child(action_button(
                    "note-save-copy",
                    t!("notes.saveCopy"),
                    palette,
                    cx.listener(|this, _, _, cx| this.save_copy(cx)),
                ))
                .child(action_button(
                    "note-overwrite",
                    t!("notes.overwrite"),
                    palette,
                    cx.listener(|this, _, _, cx| this.save_now(true, cx)),
                ));
        } else if self.status == SaveStatus::Failed {
            bar = bar
                .child(t!("notes.closeBlockedDescription").to_string())
                .child(action_button(
                    "note-retry",
                    t!("notes.retry"),
                    palette,
                    cx.listener(|this, _, _, cx| this.save_now(false, cx)),
                ));
            if self.close_requested {
                bar = bar.child(action_button(
                    "note-cancel-close",
                    t!("notes.cancelClose"),
                    palette,
                    cx.listener(|this, _, _, cx| this.cancel_close(cx)),
                ));
            }
        } else {
            bar = bar
                .child(t!("notes.deletedDescription").to_string())
                .child(action_button(
                    "note-save-deleted-copy",
                    t!("notes.saveCopy"),
                    palette,
                    cx.listener(|this, _, _, cx| this.save_copy(cx)),
                ));
        }
        bar.child(action_button(
            "note-discard",
            t!("notes.discardAndClose"),
            palette,
            cx.listener(|this, _, window, cx| this.discard_and_close(window, cx)),
        ))
        .into_any_element()
    }
}

fn action_button(
    id: &'static str,
    label: impl Into<SharedString>,
    palette: crate::theme::ThemePalette,
    handler: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui::AnyElement {
    div()
        .id(id)
        .px_2()
        .py_1()
        .rounded_sm()
        .cursor_pointer()
        .bg(rgb(palette.surface))
        .hover(move |this| this.bg(rgb(palette.hover)))
        .on_click(handler)
        .child(label.into())
        .into_any_element()
}

struct MarkdownEdit {
    replacement: String,
    selected_after: std::ops::Range<usize>,
}

fn markdown_edit(
    content: &str,
    range: std::ops::Range<usize>,
    command: MarkdownCommand,
) -> MarkdownEdit {
    let start = floor_char_boundary(content, range.start.min(content.len()));
    let end = ceil_char_boundary(content, range.end.min(content.len()));
    let selected = &content[start..end];
    let (replacement, inner_start, inner_len) = match command {
        MarkdownCommand::Paragraph => {
            let inner = if selected.is_empty() {
                "Paragraph"
            } else {
                selected
            };
            let replacement = inner
                .lines()
                .map(|line| line.trim_start_matches('#').trim_start().to_string())
                .collect::<Vec<_>>()
                .join("\n");
            let len = replacement.len();
            (replacement, 0, len)
        }
        MarkdownCommand::Bold => wrapped(selected, "**", "**", "bold"),
        MarkdownCommand::Italic => wrapped(selected, "*", "*", "italic"),
        MarkdownCommand::Underline => wrapped(selected, "<u>", "</u>", "underline"),
        MarkdownCommand::InlineCode => wrapped(selected, "`", "`", "code"),
        MarkdownCommand::Heading(level) => line_prefixed(
            selected,
            &format!("{} ", "#".repeat(level as usize)),
            "Heading",
        ),
        MarkdownCommand::Quote => line_prefixed(selected, "> ", "Quote"),
        MarkdownCommand::Bullet => line_prefixed(selected, "- ", "List item"),
        MarkdownCommand::Numbered => line_prefixed(selected, "1. ", "List item"),
        MarkdownCommand::Table => wrapped(
            selected,
            "",
            "",
            "| Column 1 | Column 2 |\n| --- | --- |\n| Value | Value |",
        ),
        MarkdownCommand::Rule => wrapped(selected, "", "", "\n---\n"),
        MarkdownCommand::CodeBlock(language) => {
            wrapped(selected, &format!("```{language}\n"), "\n```", "code")
        }
    };
    MarkdownEdit {
        replacement,
        selected_after: start + inner_start..start + inner_start + inner_len,
    }
}

fn wrapped(selected: &str, before: &str, after: &str, placeholder: &str) -> (String, usize, usize) {
    let inner = if selected.is_empty() {
        placeholder
    } else {
        selected
    };
    (format!("{before}{inner}{after}"), before.len(), inner.len())
}

fn line_prefixed(selected: &str, prefix: &str, placeholder: &str) -> (String, usize, usize) {
    let inner = if selected.is_empty() {
        placeholder
    } else {
        selected
    };
    let replacement = inner
        .lines()
        .map(|line| format!("{prefix}{line}"))
        .collect::<Vec<_>>()
        .join("\n");
    (replacement, prefix.len(), inner.len())
}

fn floor_char_boundary(text: &str, mut offset: usize) -> usize {
    while offset > 0 && !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

fn ceil_char_boundary(text: &str, mut offset: usize) -> usize {
    while offset < text.len() && !text.is_char_boundary(offset) {
        offset += 1;
    }
    offset
}

impl NyaTermApp {
    pub(in crate::features) fn open_note_editor(
        &mut self,
        note_id: String,
        cx: &mut Context<Self>,
    ) {
        if let Some(handle) = self.notes.editor_window(&note_id) {
            let app_entity = cx.entity();
            cx.defer(move |cx| {
                let raised = handle.update(cx, |_, window, _| window.activate_window());
                if raised.is_err() {
                    app_entity.update(cx, |app, cx| {
                        app.notes.remove_editor_window(&note_id);
                        cx.notify();
                    });
                }
            });
            return;
        }
        if !self.notes.begin_editor_window(&note_id) {
            return;
        }
        let request_id = note_id.clone();
        self.submit_store_request(
            0,
            store_request(StoreDomain::Notes, move |store| store.get_note(&request_id)),
            move |app, event, cx| match event.outcome {
                Ok(Some(note)) => {
                    let app = cx.entity();
                    cx.defer(move |cx| open_note_editor_window(app, note, cx));
                }
                Ok(None) => {
                    app.notes.finish_editor_window(note_id.clone(), None);
                    app.refresh_notes(cx);
                }
                Err(error) => {
                    app.notes.finish_editor_window(note_id.clone(), None);
                    app.shell.set_status(error.to_string());
                    cx.notify();
                }
            },
            cx,
        );
    }
}

fn open_note_editor_window(app: Entity<NyaTermApp>, note: NoteDocument, cx: &mut App) {
    let note_id = note.id.clone();
    let title = note.title.clone();
    let spec = ChildWindowSpec::document(title, 980., 760.).min_size(640., 480.);
    let chrome = spec.chrome();
    let parent = app.read(cx).shell.main_window();
    let options = child_window_options(&spec, parent, cx);
    let view_app = app.clone();
    let close_app = app.clone();
    let close_note_id = note_id.clone();
    let result: anyhow::Result<NyaWindowHandle> = cx.open_window(options, move |window, cx| {
        let note_for_view = note.clone();
        let view_app_for_close = close_app.clone();
        let close_id = close_note_id.clone();
        let view =
            cx.new(|cx| NoteEditorWindow::new(view_app.clone(), note_for_view, chrome, window, cx));
        let weak_view = view.downgrade();
        view_app.update(cx, |app, _| {
            app.notes
                .register_editor_view(close_note_id.clone(), weak_view.clone());
        });
        window.on_window_should_close(cx, move |_, cx| {
            let should_close = weak_view
                .update(cx, |editor, cx| editor.request_close(cx))
                .unwrap_or(true);
            if should_close {
                view_app_for_close.update(cx, |app, cx| {
                    app.notes.remove_editor_window(&close_id);
                    cx.notify();
                });
            }
            should_close
        });
        cx.new(|cx| nya_root(view, window, cx))
    });
    app.update(cx, |app, _cx| match result {
        Ok(handle) => app.notes.finish_editor_window(note_id, Some(handle)),
        Err(error) => {
            app.notes.finish_editor_window(note_id, None);
            app.shell
                .set_status(format!("failed to open note editor: {error}"));
        }
    });
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{
        AUTOSAVE_DELAY, ExternalNoteAction, MarkdownCommand, NotesCatalogEvent, SaveStatus,
        editor_blocks_export, event_revision_for_note, external_note_action, markdown_edit,
        matches_export_revision,
    };

    #[test]
    fn unsaved_saving_conflicted_or_refreshing_editors_block_export() {
        assert!(!editor_blocks_export(
            false,
            false,
            false,
            SaveStatus::Saved
        ));
        assert!(editor_blocks_export(
            true,
            false,
            false,
            SaveStatus::Unsaved
        ));
        assert!(editor_blocks_export(false, true, false, SaveStatus::Saving));
        assert!(editor_blocks_export(false, false, true, SaveStatus::Saved));
        for status in [
            SaveStatus::Conflict,
            SaveStatus::Deleted,
            SaveStatus::ExternalUpdate,
        ] {
            assert!(editor_blocks_export(false, false, false, status));
        }
    }

    #[test]
    fn a_save_completed_after_snapshot_read_cannot_export_the_older_revision() {
        let snapshot: nyaterm_core::NotesSnapshot = serde_json::from_value(serde_json::json!({
            "notes": [{ "id": "n", "parent_id": null, "title": "N", "markdown": "saved",
                "sort_order": 0, "revision": 7, "created_at_ms": 0, "updated_at_ms": 0 }]
        }))
        .expect("snapshot");
        assert!(matches_export_revision("n", 7, &snapshot));
        assert!(!matches_export_revision("n", 8, &snapshot));
        assert!(!matches_export_revision("missing", 7, &snapshot));
    }

    #[test]
    fn markdown_commands_wrap_selection_and_snap_unicode_boundaries() {
        let edit = markdown_edit("a中b", 2..3, MarkdownCommand::Bold);
        assert_eq!(edit.replacement, "**中**");
        assert_eq!(edit.selected_after, 3..6);
    }

    #[test]
    fn markdown_commands_insert_and_select_placeholder() {
        let edit = markdown_edit("", 0..0, MarkdownCommand::CodeBlock("rust"));
        assert_eq!(edit.replacement, "```rust\ncode\n```");
        assert_eq!(edit.selected_after, 8..12);
    }

    #[test]
    fn autosave_delay_and_external_update_policy_match_notes_contract() {
        assert_eq!(AUTOSAVE_DELAY, Duration::from_millis(800));
        assert_eq!(
            external_note_action(false, 2, Some(3)),
            ExternalNoteAction::Reload
        );
        assert_eq!(
            external_note_action(true, 2, Some(3)),
            ExternalNoteAction::Conflict
        );
        assert_eq!(
            external_note_action(true, 2, None),
            ExternalNoteAction::Deleted
        );
        assert_eq!(
            external_note_action(false, 3, Some(3)),
            ExternalNoteAction::None
        );
    }

    #[test]
    fn catalog_events_target_only_the_matching_editor() {
        assert_eq!(
            event_revision_for_note(
                &NotesCatalogEvent::NoteUpserted {
                    id: "note-1".to_string(),
                    revision: 4,
                },
                "note-1",
            ),
            Some(Some(4))
        );
        assert_eq!(
            event_revision_for_note(
                &NotesCatalogEvent::NoteUpserted {
                    id: "note-2".to_string(),
                    revision: 4,
                },
                "note-1",
            ),
            None
        );
        assert_eq!(
            event_revision_for_note(
                &NotesCatalogEvent::NodesDeleted {
                    ids: vec!["folder-1".to_string(), "note-1".to_string()],
                },
                "note-1",
            ),
            Some(None)
        );
        assert_eq!(
            event_revision_for_note(
                &NotesCatalogEvent::CatalogReplaced {
                    revisions: std::collections::HashMap::from([("note-2".to_string(), 9)]),
                },
                "note-1",
            ),
            Some(None)
        );
    }
}
