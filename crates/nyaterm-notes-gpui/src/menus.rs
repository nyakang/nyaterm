//! Note menu projections. Callers validate available destinations and execute actions.
use gpui::{App, Window, px};
use nyaterm_core::{NoteFolder, NoteNodeKind};
use nyaterm_ui::{NyaDropdownMenu, NyaMenuItem};
use rust_i18n::t;
use std::{
    collections::{HashMap, HashSet},
    rc::Rc,
};

#[derive(Clone)]
pub enum MenuAction {
    CreateNote(Option<String>),
    CreateFolder(Option<String>),
    Export,
    Expand(bool),
    Refresh,
    Open(String),
    Rename { id: String, name: String },
    Move { id: String, parent: Option<String> },
    Delete(String),
}
pub type MenuActionHandler = Rc<dyn Fn(MenuAction, &mut Window, &mut App)>;

pub struct NoteMenuTarget {
    pub id: String,
    pub kind: NoteNodeKind,
    pub name: String,
    pub folders: Vec<NoteFolderTarget>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoteFolderTarget {
    pub id: String,
    pub name: String,
    pub depth: usize,
}

pub fn note_folder_targets(folders: &[NoteFolder]) -> Vec<NoteFolderTarget> {
    let mut children = HashMap::<Option<&str>, Vec<&NoteFolder>>::new();
    for folder in folders {
        children
            .entry(folder.parent_id.as_deref())
            .or_default()
            .push(folder);
    }
    for siblings in children.values_mut() {
        siblings.sort_by(|left, right| {
            left.sort_order
                .cmp(&right.sort_order)
                .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
                .then_with(|| left.id.cmp(&right.id))
        });
    }

    fn append(
        parent_id: Option<&str>,
        depth: usize,
        children: &HashMap<Option<&str>, Vec<&NoteFolder>>,
        visited: &mut HashSet<String>,
        targets: &mut Vec<NoteFolderTarget>,
    ) {
        let Some(folders) = children.get(&parent_id) else {
            return;
        };
        for folder in folders {
            if !visited.insert(folder.id.clone()) {
                continue;
            }
            targets.push(NoteFolderTarget {
                id: folder.id.clone(),
                name: folder.name.clone(),
                depth,
            });
            append(
                Some(folder.id.as_str()),
                depth + 1,
                children,
                visited,
                targets,
            );
        }
    }

    let mut targets = Vec::with_capacity(folders.len());
    append(None, 0, &children, &mut HashSet::new(), &mut targets);
    targets
}

fn item(label: String, action: MenuAction, handler: &MenuActionHandler) -> NyaMenuItem {
    let handler = handler.clone();
    NyaMenuItem::action(label).on_click(move |_, window, cx| handler(action.clone(), window, cx))
}

pub fn more_menu(handler: MenuActionHandler) -> NyaDropdownMenu {
    NyaDropdownMenu::new("notes-more")
        .icon("icons/session/more.svg")
        .icon_size(px(16.))
        .tooltip(t!("common.more"))
        .items([
            item(
                t!("notes.exportMarkdown").to_string(),
                MenuAction::Export,
                &handler,
            )
            .icon("icons/fe/download.svg"),
            item(
                t!("notes.expandAll").to_string(),
                MenuAction::Expand(true),
                &handler,
            ),
            item(
                t!("notes.collapseAll").to_string(),
                MenuAction::Expand(false),
                &handler,
            ),
            item(
                t!("common.refresh").to_string(),
                MenuAction::Refresh,
                &handler,
            )
            .icon("icons/fe/refresh.svg"),
        ])
}

pub fn context_menu_items(
    target: Option<NoteMenuTarget>,
    handler: MenuActionHandler,
) -> Vec<NyaMenuItem> {
    let Some(target) = target else {
        return vec![
            item(
                t!("notes.newNote").to_string(),
                MenuAction::CreateNote(None),
                &handler,
            )
            .icon("icons/conn/add.svg"),
            item(
                t!("notes.newFolder").to_string(),
                MenuAction::CreateFolder(None),
                &handler,
            )
            .icon("icons/fe/new-folder.svg"),
            NyaMenuItem::separator(),
            item(
                t!("common.refresh").to_string(),
                MenuAction::Refresh,
                &handler,
            )
            .icon("icons/fe/refresh.svg"),
        ];
    };
    let mut items = Vec::new();
    if target.kind == NoteNodeKind::Note {
        items.push(
            item(
                t!("notes.open").to_string(),
                MenuAction::Open(target.id.clone()),
                &handler,
            )
            .icon("icons/menu/external.svg"),
        );
    } else {
        items.push(
            item(
                t!("notes.newNote").to_string(),
                MenuAction::CreateNote(Some(target.id.clone())),
                &handler,
            )
            .icon("icons/conn/add.svg"),
        );
        items.push(
            item(
                t!("notes.newFolder").to_string(),
                MenuAction::CreateFolder(Some(target.id.clone())),
                &handler,
            )
            .icon("icons/fe/new-folder.svg"),
        );
    }
    items.push(
        item(
            t!("notes.rename").to_string(),
            MenuAction::Rename {
                id: target.id.clone(),
                name: target.name,
            },
            &handler,
        )
        .icon("icons/net/edit.svg"),
    );
    let mut move_items = Vec::with_capacity(target.folders.len() + 1);
    move_items.push(item(
        t!("notes.root").to_string(),
        MenuAction::Move {
            id: target.id.clone(),
            parent: None,
        },
        &handler,
    ));
    for folder in target.folders {
        move_items.push(
            item(
                folder.name,
                MenuAction::Move {
                    id: target.id.clone(),
                    parent: Some(folder.id),
                },
                &handler,
            )
            .label_indent(px(folder.depth as f32 * 10.)),
        );
    }
    items.push(
        NyaMenuItem::submenu(t!("notes.moveTo").to_string(), move_items)
            .icon("icons/net/move.svg")
            .submenu_min_width(px(176.))
            .submenu_max_height(px(288.))
            .submenu_scrollable(true),
    );
    items.push(
        item(
            t!("common.delete").to_string(),
            MenuAction::Delete(target.id),
            &handler,
        )
        .icon("icons/net/delete.svg")
        .danger(),
    );
    items
}

#[cfg(test)]
mod tests {
    use super::{NoteFolderTarget, note_folder_targets};
    use nyaterm_core::NoteFolder;
    use std::collections::BTreeMap;
    #[test]
    fn folder_targets_keep_tree_order_and_depth() {
        let folders = vec![
            NoteFolder {
                id: "root".into(),
                parent_id: None,
                name: "Root".into(),
                sort_order: 0,
                created_at_ms: 1,
                updated_at_ms: 1,
                extra: BTreeMap::new(),
            },
            NoteFolder {
                id: "child".into(),
                parent_id: Some("root".into()),
                name: "Child".into(),
                sort_order: 0,
                created_at_ms: 1,
                updated_at_ms: 1,
                extra: BTreeMap::new(),
            },
            NoteFolder {
                id: "sibling".into(),
                parent_id: None,
                name: "Sibling".into(),
                sort_order: 1,
                created_at_ms: 1,
                updated_at_ms: 1,
                extra: BTreeMap::new(),
            },
        ];

        assert_eq!(
            note_folder_targets(&folders),
            vec![
                NoteFolderTarget {
                    id: "root".into(),
                    name: "Root".into(),
                    depth: 0,
                },
                NoteFolderTarget {
                    id: "child".into(),
                    name: "Child".into(),
                    depth: 1,
                },
                NoteFolderTarget {
                    id: "sibling".into(),
                    name: "Sibling".into(),
                    depth: 0,
                },
            ]
        );
    }
}
