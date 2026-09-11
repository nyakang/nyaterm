use rust_i18n::t;

use gpui::prelude::*;
use gpui::{App, ClickEvent, Context, FontWeight, IntoElement, SharedString, Window, div, px, rgb};

use crate::features::{NyaTermApp, selects::NO_SELECTION_VALUE, text_inputs::TextInputSetup};
use crate::models::{NetworkTunnelEditorField, NetworkTunnelEditorState};
use nyaterm_core::ConnectionType;
use nyaterm_ui::{NyaNumberInputOptions, NyaSelectOption};

pub(in crate::features::pages::tunnels) fn network_tunnel_editor_content(
    palette: crate::theme::ThemePalette,
    editor: NetworkTunnelEditorState,
    app: &mut NyaTermApp,
    cx: &mut Context<NyaTermApp>,
) -> impl IntoElement {
    let type_options = vec![
        NyaSelectOption::new("local", t!("network.localTunnel")),
        NyaSelectOption::new("remote", t!("network.remoteTunnel")),
        NyaSelectOption::new("dynamic", t!("network.dynamicTunnel")),
    ];
    let selected_type = match editor.tunnel_type.as_str() {
        "remote" => "remote",
        "dynamic" => "dynamic",
        _ => "local",
    }
    .to_string();
    let mut group_options = vec![NyaSelectOption::new(
        NO_SELECTION_VALUE,
        t!("network.ungrouped"),
    )];
    group_options.extend(
        app.tunnel_state
            .tunnel_groups()
            .iter()
            .map(|group| NyaSelectOption::new(group.id.clone(), group.name.clone())),
    );
    let selected_group = editor
        .group_id
        .clone()
        .filter(|id| group_options.iter().any(|option| option.value() == id))
        .unwrap_or_else(|| NO_SELECTION_VALUE.to_string());
    let mut connection_options = vec![NyaSelectOption::new(
        NO_SELECTION_VALUE,
        t!("network.connectionPickerPlaceholder"),
    )];
    connection_options.extend(
        app.connection_state
            .connections()
            .iter()
            .filter(|connection| matches!(&connection.config, ConnectionType::Ssh { .. }))
            .map(|connection| NyaSelectOption::new(connection.id.clone(), connection.name.clone())),
    );
    let selected_connection = editor
        .connection_id
        .clone()
        .filter(|id| connection_options.iter().any(|option| option.value() == id))
        .unwrap_or_else(|| NO_SELECTION_VALUE.to_string());
    let preview = tunnel_editor_preview(&editor);
    // Built up front: the card is one long builder chain that only reads `app`,
    // and creating an input needs it mutably.
    let name_input = tunnel_editor_input(
        app,
        NetworkTunnelEditorField::Name,
        t!("network.tunnelName"),
        editor.name.clone(),
        TextInputSetup::placeholder(t!("network.tunnelNamePlaceholder")),
        cx,
    );
    let listen_port_input = tunnel_editor_input(
        app,
        NetworkTunnelEditorField::ListenPort,
        match editor.tunnel_type.as_str() {
            "remote" => t!("network.listenPortRemote"),
            "dynamic" => t!("network.listenPortDynamic"),
            _ => t!("network.listenPortLocal"),
        },
        editor.listen_port.clone(),
        TextInputSetup::default(),
        cx,
    );
    let dynamic = editor.is_dynamic();
    let target_port_input = (!dynamic).then(|| {
        tunnel_editor_input(
            app,
            NetworkTunnelEditorField::TargetPort,
            match editor.tunnel_type.as_str() {
                "remote" => t!("network.targetPortRemote"),
                _ => t!("network.targetPortLocal"),
            },
            editor.target_port.clone(),
            TextInputSetup::default(),
            cx,
        )
    });
    let target_host_input = (!dynamic).then(|| {
        tunnel_editor_input(
            app,
            NetworkTunnelEditorField::TargetHost,
            match editor.tunnel_type.as_str() {
                "remote" => t!("network.targetHostRemote"),
                _ => t!("network.targetHostLocal"),
            },
            editor.target_host.clone(),
            TextInputSetup::placeholder("127.0.0.1"),
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
                .child(t!("network.tunnelDialogDescription")),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap_3()
                .child(div().min_w(px(220.)).flex_1().child(name_input))
                .child(div().w(px(132.)).flex_none().child(tunnel_editor_selector(
                    app,
                    palette,
                    "network-tunnel-editor-type",
                    t!("network.tunnelType"),
                    type_options,
                    selected_type,
                    cx,
                )))
                .child(div().w(px(132.)).flex_none().child(tunnel_editor_selector(
                    app,
                    palette,
                    "network-tunnel-editor-group",
                    t!("network.group"),
                    group_options,
                    selected_group,
                    cx,
                ))),
        )
        .child(tunnel_editor_selector(
            app,
            palette,
            "network-tunnel-editor-connection",
            t!("network.selectedConnection"),
            connection_options,
            selected_connection,
            cx,
        ))
        .child(
            div()
                .grid()
                .grid_cols(2)
                .gap_2()
                .child(listen_port_input)
                .when(!editor.is_dynamic(), |this| {
                    this.children(target_port_input)
                }),
        )
        .when(!editor.is_dynamic(), |this| {
            this.children(target_host_input)
        })
        .child(
            div()
                .grid()
                .grid_cols(2)
                .gap_2()
                .child(tunnel_editor_option(
                    palette,
                    "network-tunnel-editor-bind-local",
                    t!("network.bindLocalhostOnly"),
                    t!("network.bindLocalhostOnlyHint"),
                    editor.bind_localhost,
                    cx.listener(|this, _, _, cx| {
                        this.set_network_tunnel_bind_localhost(true, cx);
                    }),
                ))
                .child(tunnel_editor_option(
                    palette,
                    "network-tunnel-editor-bind-all",
                    t!("network.bindAllInterfaces"),
                    t!("network.bindAllInterfacesHint"),
                    !editor.bind_localhost,
                    cx.listener(|this, _, _, cx| {
                        this.set_network_tunnel_bind_localhost(false, cx);
                    }),
                )),
        )
        .child(tunnel_editor_option(
            palette,
            "network-tunnel-editor-auto",
            t!("network.autoOpen"),
            t!("network.tunnelConnectionHint"),
            editor.auto_open,
            cx.listener(|this, _, _, cx| {
                this.toggle_network_tunnel_auto_open(cx);
            }),
        ))
        .child(
            div()
                .rounded_sm()
                .border_1()
                .border_color(rgb(palette.border))
                .bg(rgb(palette.input))
                .p_3()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(palette.text_muted))
                        .child(t!("network.tunnelPreview")),
                )
                .child(
                    div()
                        .font_family(crate::features::shell::gpui_code_font_family())
                        .text_xs()
                        .text_color(rgb(palette.text))
                        .child(preview),
                ),
        )
        .when_some(editor.error.clone(), |this, error| {
            this.child(div().text_xs().text_color(rgb(palette.danger)).child(error))
        })
}

pub(in crate::features::pages::tunnels) fn tunnel_editor_input(
    app: &mut NyaTermApp,
    field: NetworkTunnelEditorField,
    caption: impl Into<SharedString>,
    value: String,
    setup: TextInputSetup,
    cx: &mut Context<NyaTermApp>,
) -> gpui::AnyElement {
    let caption: SharedString = caption.into();
    if matches!(
        field,
        NetworkTunnelEditorField::ListenPort | NetworkTunnelEditorField::TargetPort
    ) {
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
                format!("network.tunnel-editor.{}", tunnel_editor_field_key(field)),
                &value,
                NyaNumberInputOptions::default().range(1.0, 65535.0),
                cx,
            ))
            .into_any_element();
    }

    app.text_input_field(
        format!("network.tunnel-editor.{}", tunnel_editor_field_key(field)),
        caption,
        &value,
        setup,
        cx,
    )
    .into_any_element()
}

/// The stable part of a tunnel field's input id.
pub(in crate::features::pages::tunnels) fn tunnel_editor_field_key(
    field: NetworkTunnelEditorField,
) -> &'static str {
    match field {
        NetworkTunnelEditorField::Name => "name",
        NetworkTunnelEditorField::ListenPort => "listen-port",
        NetworkTunnelEditorField::TargetHost => "target-host",
        NetworkTunnelEditorField::TargetPort => "target-port",
    }
}

pub(in crate::features::pages::tunnels) fn tunnel_editor_selector<I>(
    app: &mut NyaTermApp,
    palette: crate::theme::ThemePalette,
    id: I,
    label: impl Into<SharedString>,
    options: Vec<NyaSelectOption>,
    selected_value: String,
    cx: &mut Context<NyaTermApp>,
) -> impl IntoElement
where
    I: Into<String>,
{
    let label: SharedString = label.into();
    div()
        .min_w_0()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .text_xs()
                .text_color(rgb(palette.text_muted))
                .child(label),
        )
        .child(app.form_select_control(id.into(), options, Some(selected_value), false, cx))
}

pub(in crate::features::pages::tunnels) fn tunnel_editor_option(
    palette: crate::theme::ThemePalette,
    id: impl Into<String>,
    title: impl Into<SharedString>,
    detail: impl Into<SharedString>,
    active: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let title: SharedString = title.into();
    let detail: SharedString = detail.into();
    // Tauri-like selectable option cards for bind host / auto open.
    div()
        .id(gpui::SharedString::from(id.into()))
        .rounded_md()
        .border_1()
        .border_color(if active {
            rgb(palette.link)
        } else {
            rgb(palette.border)
        })
        .bg(if active {
            rgb(palette.hover)
        } else {
            rgb(palette.bg)
        })
        .px_3()
        .py_2()
        .flex()
        .flex_col()
        .gap_1()
        .cursor_pointer()
        .hover(|this| this.bg(rgb(palette.surface)))
        .child(
            div()
                .text_size(px(12.))
                .font_weight(FontWeight(600.))
                .text_color(if active {
                    rgb(palette.link)
                } else {
                    rgb(palette.text)
                })
                .child(title),
        )
        .child(
            div()
                .text_size(px(11.))
                .text_color(rgb(palette.text_muted))
                .child(detail),
        )
        .on_click(on_click)
}

pub(super) fn tunnel_editor_preview(editor: &NetworkTunnelEditorState) -> String {
    let bind_host = if editor.bind_localhost {
        "127.0.0.1"
    } else {
        "0.0.0.0"
    };
    let listen_port = editor.listen_port.trim();
    let listen_port = if listen_port.is_empty() {
        "?"
    } else {
        listen_port
    };
    if editor.is_dynamic() {
        return format!("SOCKS {bind_host}:{listen_port}");
    }

    let target_host = editor.target_host.trim();
    let target_host = if target_host.is_empty() {
        "?"
    } else {
        target_host
    };
    let target_port = editor.target_port.trim();
    let target_port = if target_port.is_empty() {
        "?"
    } else {
        target_port
    };
    if editor.tunnel_type == "remote" {
        format!("remote {bind_host}:{listen_port} -> {target_host}:{target_port}")
    } else {
        format!("local {bind_host}:{listen_port} -> {target_host}:{target_port}")
    }
}
