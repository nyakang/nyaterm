use rust_i18n::t;

use std::borrow::Cow;

use gpui::prelude::*;
use gpui::{Context, IntoElement, SharedString, div, px, rgb};

use super::super::tunnel::tunnel_editor_selector;
use super::helpers::proxy_protocol_label;
use crate::features::{NyaTermApp, selects::NO_SELECTION_VALUE, text_inputs::TextInputSetup};
use crate::models::{NetworkProxyEditorField, NetworkProxyEditorState};
use nyaterm_ui::{NyaNumberInputOptions, NyaSelectOption};

pub(in crate::features::pages::tunnels) fn network_proxy_editor_content(
    palette: crate::theme::ThemePalette,
    editor: NetworkProxyEditorState,
    app: &mut NyaTermApp,
    cx: &mut Context<NyaTermApp>,
) -> impl IntoElement {
    let protocol_options = ["socks5", "http", "proxycommand"]
        .into_iter()
        .map(|protocol| NyaSelectOption::new(protocol, proxy_protocol_label(protocol)))
        .collect();
    let selected_protocol = match editor.protocol.as_str() {
        "http" => "http",
        "proxycommand" => "proxycommand",
        _ => "socks5",
    }
    .to_string();
    let mut group_options = vec![NyaSelectOption::new(
        NO_SELECTION_VALUE,
        t!("network.ungrouped"),
    )];
    group_options.extend(
        app.tunnel_state
            .proxy_groups()
            .iter()
            .map(|group| NyaSelectOption::new(group.id.clone(), group.name.clone())),
    );
    let selected_group = editor
        .group_id
        .clone()
        .filter(|id| group_options.iter().any(|option| option.value() == id))
        .unwrap_or_else(|| NO_SELECTION_VALUE.to_string());
    // A stored password is never shown, so the box says so in its placeholder
    // rather than putting a row of asterisks where the text would go.
    let password_placeholder = if editor.existing_password.is_some() || editor.password_id.is_some()
    {
        t!("network.proxyPasswordKeep")
    } else {
        Cow::Borrowed("")
    };
    let name_input = proxy_editor_input(
        app,
        NetworkProxyEditorField::Name,
        t!("network.proxyName"),
        editor.name.clone(),
        TextInputSetup::placeholder(t!("network.proxyNamePlaceholder")),
        cx,
    );
    let is_command = editor.is_proxy_command();
    let command_input = is_command.then(|| {
        proxy_editor_input(
            app,
            NetworkProxyEditorField::Command,
            t!("network.proxyCommand"),
            editor.command.clone(),
            TextInputSetup::multi_line(t!("network.proxyCommandPlaceholder")),
            cx,
        )
    });
    let host_input = (!is_command).then(|| {
        proxy_editor_input(
            app,
            NetworkProxyEditorField::Host,
            t!("settings.proxyHost"),
            editor.host.clone(),
            TextInputSetup::placeholder("127.0.0.1"),
            cx,
        )
    });
    let port_input = (!is_command).then(|| {
        proxy_editor_input(
            app,
            NetworkProxyEditorField::Port,
            t!("settings.proxyPort"),
            editor.port.clone(),
            TextInputSetup::default(),
            cx,
        )
    });
    let username_input = (!is_command).then(|| {
        proxy_editor_input(
            app,
            NetworkProxyEditorField::Username,
            t!("network.proxyUsername"),
            editor.username.clone(),
            TextInputSetup::placeholder(t!("network.proxyUsernamePlaceholder")),
            cx,
        )
    });
    let password_input = (!is_command).then(|| {
        proxy_editor_input(
            app,
            NetworkProxyEditorField::Password,
            t!("network.proxyPassword"),
            editor.password.expose_secret().to_owned(),
            TextInputSetup {
                placeholder: password_placeholder.into(),
                masked: true,
                multi_line: false,
                submit_on_enter: false,
                code: false,
            },
            cx,
        )
    });

    div()
        .flex()
        .flex_col()
        .gap_4()
        .child(
            div()
                .text_size(px(12.))
                .text_color(rgb(palette.text_muted))
                .child(t!("network.proxyDialogDescription")),
        )
        .child(
            div()
                .flex()
                .gap_3()
                .child(div().w(px(144.)).flex_none().child(tunnel_editor_selector(
                    app,
                    palette,
                    "network-proxy-editor-protocol",
                    t!("settings.proxyProtocol"),
                    protocol_options,
                    selected_protocol,
                    cx,
                )))
                .child(div().flex_1().min_w_0().child(name_input)),
        )
        .child(tunnel_editor_selector(
            app,
            palette,
            "network-proxy-editor-group",
            t!("network.group"),
            group_options,
            selected_group,
            cx,
        ))
        .when(editor.is_proxy_command(), |this| {
            this.child(div().min_h(px(104.)).children(command_input))
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(palette.text_muted))
                        .child(t!("network.proxyCommandHint")),
                )
        })
        .when(!editor.is_proxy_command(), |this| {
            this.child(
                div()
                    .grid()
                    .grid_cols(2)
                    .gap_2()
                    .children(host_input)
                    .children(port_input),
            )
            .child(
                div()
                    .grid()
                    .grid_cols(2)
                    .gap_2()
                    .children(username_input)
                    .children(password_input),
            )
        })
        .when_some(editor.error.clone(), |this, error| {
            this.child(div().text_xs().text_color(rgb(palette.danger)).child(error))
        })
}

pub(in crate::features::pages::tunnels) fn proxy_editor_input(
    app: &mut NyaTermApp,
    field: NetworkProxyEditorField,
    caption: impl Into<SharedString>,
    value: String,
    setup: TextInputSetup,
    cx: &mut Context<NyaTermApp>,
) -> gpui::AnyElement {
    let caption: SharedString = caption.into();
    if field == NetworkProxyEditorField::Port {
        let palette = app.theme_palette();
        return div()
            .min_w_0()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(palette.text_muted))
                    .child(caption),
            )
            .child(app.number_input_box(
                format!("network.proxy-editor.{}", proxy_editor_field_key(field)),
                &value,
                NyaNumberInputOptions::default().range(1.0, 65535.0),
                cx,
            ))
            .into_any_element();
    }

    app.text_input_field(
        format!("network.proxy-editor.{}", proxy_editor_field_key(field)),
        caption,
        &value,
        setup,
        cx,
    )
    .into_any_element()
}

/// The stable part of a proxy field's input id.
fn proxy_editor_field_key(field: NetworkProxyEditorField) -> &'static str {
    match field {
        NetworkProxyEditorField::Name => "name",
        NetworkProxyEditorField::Host => "host",
        NetworkProxyEditorField::Port => "port",
        NetworkProxyEditorField::Command => "command",
        NetworkProxyEditorField::Username => "username",
        NetworkProxyEditorField::Password => "password",
    }
}
