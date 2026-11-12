use rust_i18n::t;

use std::borrow::Cow;

use gpui::prelude::*;
use gpui::{App, ClickEvent, Context, IntoElement, Window, div, px, rgb};
use nyaterm_ui::{NyaDropdownMenu, NyaMenuItem};

use crate::features::{NyaTermApp, text_inputs::TextInputSetup};
use crate::models::{NetworkGroupEditorState, NetworkTab};

pub(super) struct NetworkItemMenuConfig {
    pub(super) palette: crate::theme::ThemePalette,
    pub(super) id: String,
    pub(super) more_label: Cow<'static, str>,
    pub(super) edit_label: Cow<'static, str>,
    pub(super) move_label: Cow<'static, str>,
    pub(super) delete_label: Cow<'static, str>,
    pub(super) move_items: Vec<NyaMenuItem>,
}

pub(super) fn network_item_overflow_menu(
    config: NetworkItemMenuConfig,
    on_edit: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    on_delete: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let NetworkItemMenuConfig {
        id,
        more_label,
        edit_label,
        move_label,
        delete_label,
        move_items,
        ..
    } = config;
    let mut items = vec![
        NyaMenuItem::action(edit_label)
            .icon("icons/net/edit.svg")
            .on_click(on_edit),
    ];
    if !move_items.is_empty() {
        items.push(NyaMenuItem::submenu(move_label, move_items).icon("icons/net/move.svg"));
    }
    items.extend([
        NyaMenuItem::separator(),
        NyaMenuItem::action(delete_label)
            .icon("icons/net/delete.svg")
            .danger()
            .on_click(on_delete),
    ]);

    NyaDropdownMenu::new(id)
        .icon("icons/session/more.svg")
        .icon_size(px(14.))
        .tooltip(more_label)
        .min_width(px(164.))
        .items(items)
}

pub(super) fn network_move_menu_items(
    tab: NetworkTab,
    item_id: &str,
    current_group_id: Option<&str>,
    groups: impl IntoIterator<Item = (String, String)>,
    cx: &mut Context<NyaTermApp>,
) -> Vec<NyaMenuItem> {
    let groups = groups.into_iter().collect::<Vec<_>>();
    let current_group_id =
        current_group_id.filter(|id| groups.iter().any(|(group_id, _)| group_id == id));
    let targets = std::iter::once((None, t!("network.ungrouped").to_string()))
        .chain(groups.into_iter().map(|(id, name)| (Some(id), name)));
    let mut items = Vec::new();
    for (group_id, name) in targets {
        if items.len() == 1 {
            items.push(NyaMenuItem::separator());
        }
        let is_current = group_id.as_deref() == current_group_id;
        let label = if is_current {
            format!("{name} · {}", t!("network.current"))
        } else {
            name
        };
        let item_id = item_id.to_string();
        items.push(
            NyaMenuItem::action(label)
                .icon("icons/conn/folder.svg")
                .disabled(is_current)
                .on_click(cx.listener(move |this, _, _, cx| match tab {
                    NetworkTab::Tunnels => {
                        this.move_tunnel_to_group(item_id.clone(), group_id.clone(), cx)
                    }
                    NetworkTab::Proxies => {
                        this.move_proxy_to_group(item_id.clone(), group_id.clone(), cx)
                    }
                })),
        );
    }
    items
}

pub(super) fn network_group_editor_content(
    app: &mut NyaTermApp,
    editor: NetworkGroupEditorState,
    cx: &mut Context<NyaTermApp>,
) -> impl IntoElement {
    let name_input = app
        .text_input_field(
            "network.group-editor.name",
            t!("network.groupName"),
            &editor.name,
            TextInputSetup::default(),
            cx,
        )
        .into_any_element();
    div().flex().flex_col().gap_4().child(name_input).when_some(
        editor.error.clone(),
        |this, error| {
            this.child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(0xfda4af))
                    .child(error),
            )
        },
    )
}
