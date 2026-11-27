use gpui::{App, Entity, IntoElement, SharedString, Window, div, prelude::*, px, rgb};
use nyaterm_ui::markdown::{NyaMarkdown, NyaMarkdownImagePolicy};
use nyaterm_ui::text::{NyaTextState, NyaTextStyle};
use nyaterm_ui::{
    NyaDocumentEditor, NyaDocumentEditorState, NyaDropdownMenu, NyaMenuItem, NyaScrollable as _,
    ThemePalette,
};
use rust_i18n::t;
use std::rc::Rc;
#[derive(Clone, Copy)]
pub enum EditorAction {
    Format(MarkdownCommand),
    Link,
    Mode(NoteViewMode),
    Reload,
    SaveCopy,
    Save(bool),
    CancelClose,
    DiscardAndClose,
}
pub type EditorActionHandler = Rc<dyn Fn(EditorAction, &mut Window, &mut App)>;
fn on_action(
    handler: &EditorActionHandler,
    action: EditorAction,
) -> impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static {
    let handler = handler.clone();
    move |_, window, cx| handler(action, window, cx)
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NoteViewMode {
    Source,
    #[default]
    Split,
    Preview,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveStatus {
    Saved,
    Saving,
    Unsaved,
    Failed,
    Conflict,
    Deleted,
    ExternalUpdate,
}

#[derive(Clone, Copy)]
pub enum MarkdownCommand {
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

pub fn toolbar(
    editor: &Entity<NyaDocumentEditorState>,
    mode: NoteViewMode,
    palette: ThemePalette,
    handler: EditorActionHandler,
) -> gpui::AnyElement {
    let mut bar = div().flex().items_center().gap_1();
    let undo_editor = editor.clone();
    let redo_editor = editor.clone();
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
                .on_click(on_action(&handler, EditorAction::Format(command)))
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
            .on_click(on_action(&handler, EditorAction::Link))
            .child("↗"),
    );
    let editor_handler = handler.clone();
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
                let editor_handler = editor_handler.clone();
                NyaMenuItem::action(label).on_click(move |_, window, cx| {
                    editor_handler(
                        EditorAction::Format(MarkdownCommand::CodeBlock(language)),
                        window,
                        cx,
                    );
                })
            })),
    );
    let current_mode = mode;
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
                .bg(if current_mode == mode {
                    rgb(palette.surface_elevated)
                } else {
                    rgb(palette.bg)
                })
                .on_click(on_action(&handler, EditorAction::Mode(mode)))
                .child(label),
        );
    }
    bar.into_any_element()
}

pub fn conflict_bar(
    status: SaveStatus,
    close_requested: bool,
    palette: ThemePalette,
    handler: EditorActionHandler,
) -> gpui::AnyElement {
    let mut bar = div()
        .px_3()
        .py_2()
        .flex()
        .items_center()
        .gap_2()
        .bg(rgb(palette.surface_elevated));
    if status == SaveStatus::Conflict {
        bar = bar
            .child(t!("notes.revisionConflictDescription").to_string())
            .child(action_button(
                "note-reload",
                t!("notes.reload"),
                palette,
                on_action(&handler, EditorAction::Reload),
            ))
            .child(action_button(
                "note-save-copy",
                t!("notes.saveCopy"),
                palette,
                on_action(&handler, EditorAction::SaveCopy),
            ))
            .child(action_button(
                "note-overwrite",
                t!("notes.overwrite"),
                palette,
                on_action(&handler, EditorAction::Save(true)),
            ));
    } else if status == SaveStatus::Failed {
        bar = bar
            .child(t!("notes.closeBlockedDescription").to_string())
            .child(action_button(
                "note-retry",
                t!("notes.retry"),
                palette,
                on_action(&handler, EditorAction::Save(false)),
            ));
        if close_requested {
            bar = bar.child(action_button(
                "note-cancel-close",
                t!("notes.cancelClose"),
                palette,
                on_action(&handler, EditorAction::CancelClose),
            ));
        }
    } else {
        bar = bar
            .child(t!("notes.deletedDescription").to_string())
            .child(action_button(
                "note-save-deleted-copy",
                t!("notes.saveCopy"),
                palette,
                on_action(&handler, EditorAction::SaveCopy),
            ));
    }
    bar.child(action_button(
        "note-discard",
        t!("notes.discardAndClose"),
        palette,
        on_action(&handler, EditorAction::DiscardAndClose),
    ))
    .into_any_element()
}

fn action_button(
    id: &'static str,
    label: impl Into<SharedString>,
    palette: ThemePalette,
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

pub fn body(
    editor: &Entity<NyaDocumentEditorState>,
    markdown: &NyaTextState,
    mode: NoteViewMode,
    palette: ThemePalette,
) -> gpui::AnyElement {
    let mut body = div().flex_1().min_h_0().min_w_0().flex();
    if matches!(mode, NoteViewMode::Source | NoteViewMode::Split) {
        body = body.child(
            div()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .border_r_1()
                .border_color(rgb(palette.border))
                .child(NyaDocumentEditor::new(editor)),
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
                .text_size(px(12.))
                .line_height(px(18.))
                .child(
                    NyaMarkdown::from_state(markdown)
                        .style(NyaTextStyle::document_preview(palette))
                        .image_policy(NyaMarkdownImagePolicy::HttpsOnly),
                ),
        );
    }
    body.into_any_element()
}
pub struct MarkdownEdit {
    pub replacement: String,
    pub selected_after: std::ops::Range<usize>,
}

pub fn markdown_edit(
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

pub fn floor_char_boundary(text: &str, mut offset: usize) -> usize {
    while offset > 0 && !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

pub fn ceil_char_boundary(text: &str, mut offset: usize) -> usize {
    while offset < text.len() && !text.is_char_boundary(offset) {
        offset += 1;
    }
    offset
}

#[cfg(test)]
mod tests {
    use super::{MarkdownCommand, markdown_edit};
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
}
