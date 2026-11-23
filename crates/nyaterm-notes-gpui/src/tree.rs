use gpui::{
    App, AppContext as _, Context, Entity, FocusHandle, IntoElement, MouseButton, MouseDownEvent,
    Render, SharedString, Window, div, prelude::*, px, rgb, rgba,
};
use nyaterm_core::NoteNodeKind;
use nyaterm_ui::{NyaInputShell, NyaInputState, ThemePalette, icons::mono_icon};
use std::{cell::RefCell, rc::Rc};
pub enum TreeAction {
    Select(String),
    Toggle(String),
    Open(String),
    Move(String, Option<String>),
}
pub type TreeActionHandler = Rc<dyn Fn(TreeAction, &mut App)>;
pub type CanMoveHandler = Rc<dyn Fn(&str, Option<&str>, &App) -> bool>;
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteTreeRow {
    pub id: String,
    pub kind: NoteNodeKind,
    pub parent_id: Option<String>,
    pub name: String,
    pub depth: usize,
    pub expanded: bool,
    pub has_children: bool,
}

#[derive(Clone)]
pub struct NotesDragPayload {
    pub id: String,
    pub label: String,
}

struct NotesDragPreview {
    payload: NotesDragPayload,
    position: gpui::Point<gpui::Pixels>,
    palette: ThemePalette,
}

impl Render for NotesDragPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .pl(self.position.x - px(70.))
            .pt(self.position.y - px(16.))
            .child(
                div()
                    .w(px(140.))
                    .h(px(32.))
                    .px_2()
                    .flex()
                    .items_center()
                    .rounded_sm()
                    .border_1()
                    .border_color(rgb(self.palette.border))
                    .bg(rgb(self.palette.surface_elevated))
                    .text_color(rgb(self.palette.text))
                    .child(self.payload.label.clone()),
            )
    }
}

pub fn note_row_drop_parent(row: &NoteTreeRow, source_id: &str) -> Option<Option<String>> {
    if source_id == row.id {
        return None;
    }
    Some(match row.kind {
        NoteNodeKind::Folder => Some(row.id.clone()),
        NoteNodeKind::Note => row.parent_id.clone(),
    })
}

pub struct TreeHandlers {
    pub action: TreeActionHandler,
    pub can_move: CanMoveHandler,
}

pub fn note_row(
    row: NoteTreeRow,
    selected_id: Option<&str>,
    palette: ThemePalette,
    rename: Option<&Entity<NyaInputState>>,
    focus: FocusHandle,
    context_target: Rc<RefCell<Option<String>>>,
    handlers: TreeHandlers,
) -> gpui::AnyElement {
    let TreeHandlers {
        action: handler,
        can_move,
    } = handlers;
    let selected = selected_id == Some(row.id.as_str());
    let click_id = row.id.clone();
    let kind = row.kind;
    let click_handler = handler.clone();
    let drag_payload = NotesDragPayload {
        id: row.id.clone(),
        label: row.name.clone(),
    };
    if let Some(rename) = rename {
        return div()
            .h(px(28.))
            .w_full()
            .flex_none()
            .pl(px(48. + row.depth as f32 * 14.))
            .pr_1()
            .flex()
            .items_center()
            .child(
                div()
                    .w_full()
                    .h(px(24.))
                    .child(NyaInputShell::new("notes-inline-rename", rename)),
            )
            .into_any_element();
    }
    let expander = if row.kind == NoteNodeKind::Folder {
        let toggle_handler = handler.clone();
        let toggle_id = row.id.clone();
        let mut chevron = mono_icon(
            "icons/menu/chevron-right.svg",
            rgb(palette.text_dimmed).into(),
            16.,
        );
        if row.expanded {
            chevron =
                chevron.with_transformation(gpui::Transformation::rotate(gpui::percentage(0.25)));
        }
        div()
            .id(SharedString::from(format!("notes-toggle-{}", row.id)))
            .size(px(20.))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded_sm()
            .hover(move |this| this.bg(rgb(palette.hover)))
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                toggle_handler(TreeAction::Toggle(toggle_id.clone()), cx);
            })
            .child(chevron)
            .into_any_element()
    } else {
        div().size(px(20.)).flex_none().into_any_element()
    };
    let icon_path = match (row.kind, row.expanded) {
        (NoteNodeKind::Folder, true) => "icons/session/folder-open.svg",
        (NoteNodeKind::Folder, false) => "icons/conn/folder.svg",
        (NoteNodeKind::Note, _) => "icons/notes.svg",
    };
    let icon_color = if row.kind == NoteNodeKind::Folder && row.expanded {
        palette.primary
    } else {
        palette.text_muted
    };
    let context_id = row.id.clone();
    let row_selector = format!("notes-row-{}", row.id);
    let drop_row_for_style = row.clone();
    let drop_row = row.clone();
    let drop_handler = handler.clone();
    let base = div()
        .id(SharedString::from(format!("notes-row-{}", row.id)))
        .debug_selector(move || row_selector.clone())
        .h(px(28.))
        .w_full()
        .flex_none()
        .flex()
        .items_center()
        .gap_1()
        .pr_1()
        .pl(px(4. + row.depth as f32 * 14.))
        .rounded_sm()
        .bg(if selected {
            rgba((palette.primary << 8) | 0x2e)
        } else {
            rgba(0x00000000)
        })
        .text_color(rgb(if selected {
            palette.text
        } else {
            palette.text_muted
        }))
        .hover(move |this| this.bg(rgb(palette.hover)).text_color(rgb(palette.text)))
        .cursor_pointer()
        .cursor_move()
        .capture_any_mouse_down(move |event: &MouseDownEvent, _, _| {
            if event.button == MouseButton::Right {
                *context_target.borrow_mut() = Some(context_id.clone());
            }
        })
        .on_drag(drag_payload, move |payload, position, _, cx| {
            cx.new(|_| NotesDragPreview {
                payload: payload.clone(),
                position,
                palette,
            })
        })
        .drag_over::<NotesDragPayload>(move |this, payload, _, cx| {
            let valid = note_row_drop_parent(&drop_row_for_style, &payload.id)
                .is_some_and(|parent_id| can_move(&payload.id, parent_id.as_deref(), cx));
            if valid {
                this.border_1().border_color(rgb(palette.primary))
            } else {
                this
            }
        })
        .on_drop(
            move |payload: &NotesDragPayload, _: &mut Window, cx: &mut App| {
                cx.stop_propagation();
                let id = payload.id.clone();
                let Some(parent_id) = note_row_drop_parent(&drop_row, &id) else {
                    return;
                };
                drop_handler(TreeAction::Move(id, parent_id), cx);
            },
        )
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            cx.stop_propagation();
            window.focus(&focus, cx);
        })
        .on_click(move |event, _, cx| {
            click_handler(TreeAction::Select(click_id.clone()), cx);
            if event.click_count() >= 2 {
                click_handler(
                    match kind {
                        NoteNodeKind::Folder => TreeAction::Toggle(click_id.clone()),
                        NoteNodeKind::Note => TreeAction::Open(click_id.clone()),
                    },
                    cx,
                );
            }
        })
        .child(expander)
        .child(
            div()
                .size(px(20.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .child(mono_icon(icon_path, rgb(icon_color).into(), 14.)),
        )
        .child(
            div()
                .min_w_0()
                .flex_1()
                .truncate()
                .text_size(px(12.))
                .child(row.name),
        );
    base.into_any_element()
}

#[cfg(test)]
mod tests {
    use super::{NoteTreeRow, note_row_drop_parent};
    #[test]
    fn row_drop_targets_match_tauri_parent_rules() {
        let folder = NoteTreeRow {
            id: "folder".into(),
            kind: nyaterm_core::NoteNodeKind::Folder,
            parent_id: None,
            name: "Folder".into(),
            depth: 0,
            expanded: false,
            has_children: false,
        };
        let note = NoteTreeRow {
            id: "note".into(),
            kind: nyaterm_core::NoteNodeKind::Note,
            parent_id: Some("folder".into()),
            name: "Note".into(),
            depth: 1,
            expanded: false,
            has_children: false,
        };

        assert_eq!(
            note_row_drop_parent(&folder, "source"),
            Some(Some("folder".into()))
        );
        assert_eq!(
            note_row_drop_parent(&note, "source"),
            Some(Some("folder".into()))
        );
        assert_eq!(note_row_drop_parent(&folder, "folder"), None);
        assert_eq!(note_row_drop_parent(&note, "note"), None);
    }
}
