use std::collections::HashSet;

use crate::models::{
    NetworkGroupEditorState, NetworkProxyEditorField, NetworkProxyEditorState, NetworkTab,
    NetworkTunnelEditorField, NetworkTunnelEditorState,
};

/// Write the group draft's name.
pub(super) fn set_network_group_editor_name(
    group_editor: &mut Option<NetworkGroupEditorState>,
    text: String,
) -> bool {
    let Some(editor) = group_editor.as_mut() else {
        return false;
    };
    editor.name = text;
    editor.error = None;
    true
}

pub(super) fn set_network_group_editor_error(
    group_editor: &mut Option<NetworkGroupEditorState>,
    error: String,
) -> bool {
    let Some(editor) = group_editor.as_mut() else {
        return false;
    };
    editor.error = Some(error);
    true
}

pub(super) fn remove_network_item_references(
    tunnel_editor: &mut Option<NetworkTunnelEditorState>,
    proxy_editor: &mut Option<NetworkProxyEditorState>,
    tab: NetworkTab,
    id: &str,
) {
    match tab {
        NetworkTab::Tunnels => {
            if tunnel_editor
                .as_ref()
                .is_some_and(|editor| editor.id.as_deref() == Some(id))
            {
                *tunnel_editor = None;
            }
        }
        NetworkTab::Proxies => {
            if proxy_editor
                .as_ref()
                .is_some_and(|editor| editor.id.as_deref() == Some(id))
            {
                *proxy_editor = None;
            }
        }
    }
}

pub(super) fn remove_network_group_references(
    group_editor: &mut Option<NetworkGroupEditorState>,
    expanded_sections: &mut HashSet<String>,
    tab: NetworkTab,
    group_id: &str,
) {
    expanded_sections.remove(&network_section_key(tab, group_id));
    if group_editor
        .as_ref()
        .is_some_and(|editor| editor.tab == tab && editor.id.as_deref() == Some(group_id))
    {
        *group_editor = None;
    }
}

pub(super) fn clear_network_tunnel_editor(tunnel_editor: &mut Option<NetworkTunnelEditorState>) {
    *tunnel_editor = None;
}

/// Write one field of the tunnel draft.
///
/// A port field keeps only digits: the boxes accept anything typed, and the
/// draft is what gets validated and saved.
pub(super) fn set_network_tunnel_editor_field(
    tunnel_editor: &mut Option<NetworkTunnelEditorState>,
    field: NetworkTunnelEditorField,
    text: String,
) -> bool {
    let Some(editor) = tunnel_editor.as_mut() else {
        return false;
    };
    editor.focused_field = field;
    let text = match field {
        NetworkTunnelEditorField::ListenPort | NetworkTunnelEditorField::TargetPort => {
            text.chars().filter(char::is_ascii_digit).collect()
        }
        _ => text,
    };
    *network_tunnel_editor_field_mut(editor) = text;
    editor.error = None;
    true
}

fn network_tunnel_editor_field_mut(editor: &mut NetworkTunnelEditorState) -> &mut String {
    match editor.focused_field {
        NetworkTunnelEditorField::Name => &mut editor.name,
        NetworkTunnelEditorField::ListenPort => &mut editor.listen_port,
        NetworkTunnelEditorField::TargetHost => &mut editor.target_host,
        NetworkTunnelEditorField::TargetPort => &mut editor.target_port,
    }
}

pub(super) fn set_network_tunnel_type(
    tunnel_editor: &mut Option<NetworkTunnelEditorState>,
    tunnel_type: &str,
) -> Option<String> {
    let editor = tunnel_editor.as_mut()?;
    editor.tunnel_type = match tunnel_type {
        "remote" => "remote",
        "dynamic" => "dynamic",
        _ => "local",
    }
    .to_string();
    if editor.is_dynamic() {
        editor.focused_field = match editor.focused_field {
            NetworkTunnelEditorField::TargetHost | NetworkTunnelEditorField::TargetPort => {
                NetworkTunnelEditorField::ListenPort
            }
            field => field,
        };
    }
    editor.error = None;
    Some(editor.tunnel_type.clone())
}

pub(super) fn set_network_tunnel_connection(
    tunnel_editor: &mut Option<NetworkTunnelEditorState>,
    connection_id: Option<String>,
) -> bool {
    let Some(editor) = tunnel_editor.as_mut() else {
        return false;
    };
    editor.connection_id = connection_id;
    editor.error = None;
    true
}

pub(super) fn set_network_tunnel_group(
    tunnel_editor: &mut Option<NetworkTunnelEditorState>,
    group_id: Option<String>,
) -> bool {
    let Some(editor) = tunnel_editor.as_mut() else {
        return false;
    };
    editor.group_id = group_id;
    editor.error = None;
    true
}

pub(super) fn set_network_tunnel_bind_localhost(
    tunnel_editor: &mut Option<NetworkTunnelEditorState>,
    bind_localhost: bool,
) -> bool {
    let Some(editor) = tunnel_editor.as_mut() else {
        return false;
    };
    editor.bind_localhost = bind_localhost;
    editor.error = None;
    true
}

pub(super) fn toggle_network_tunnel_auto_open(
    tunnel_editor: &mut Option<NetworkTunnelEditorState>,
) -> Option<bool> {
    let editor = tunnel_editor.as_mut()?;
    editor.auto_open = !editor.auto_open;
    editor.error = None;
    Some(editor.auto_open)
}

pub(super) fn set_network_tunnel_editor_error(
    tunnel_editor: &mut Option<NetworkTunnelEditorState>,
    error: String,
) -> bool {
    let Some(editor) = tunnel_editor.as_mut() else {
        return false;
    };
    editor.error = Some(error);
    true
}

pub(super) fn clear_network_proxy_editor(proxy_editor: &mut Option<NetworkProxyEditorState>) {
    *proxy_editor = None;
}

/// Write one field of the proxy draft.
///
/// The port keeps only digits: the box accepts anything typed, and the draft is
/// what gets validated and saved.
pub(super) fn set_network_proxy_editor_field(
    proxy_editor: &mut Option<NetworkProxyEditorState>,
    field: NetworkProxyEditorField,
    text: String,
) -> bool {
    let Some(editor) = proxy_editor.as_mut() else {
        return false;
    };
    editor.focused_field = field;
    let text = match field {
        NetworkProxyEditorField::Port => text.chars().filter(char::is_ascii_digit).collect(),
        _ => text,
    };
    *network_proxy_editor_field_mut(editor) = text;
    editor.error = None;
    true
}

fn network_proxy_editor_field_mut(editor: &mut NetworkProxyEditorState) -> &mut String {
    match editor.focused_field {
        NetworkProxyEditorField::Name => &mut editor.name,
        NetworkProxyEditorField::Host => &mut editor.host,
        NetworkProxyEditorField::Port => &mut editor.port,
        NetworkProxyEditorField::Command => &mut editor.command,
        NetworkProxyEditorField::Username => &mut editor.username,
        NetworkProxyEditorField::Password => editor.password.expose_secret_mut(),
    }
}

pub(super) fn set_network_proxy_protocol(
    proxy_editor: &mut Option<NetworkProxyEditorState>,
    protocol: &str,
) -> Option<String> {
    let editor = proxy_editor.as_mut()?;
    editor.protocol = match protocol {
        "http" => "http",
        "proxycommand" => "proxycommand",
        _ => "socks5",
    }
    .to_string();
    if editor.is_proxy_command() {
        editor.focused_field = match editor.focused_field {
            NetworkProxyEditorField::Host
            | NetworkProxyEditorField::Port
            | NetworkProxyEditorField::Username
            | NetworkProxyEditorField::Password => NetworkProxyEditorField::Command,
            field => field,
        };
    } else if editor.focused_field == NetworkProxyEditorField::Command {
        editor.focused_field = NetworkProxyEditorField::Host;
    }
    editor.error = None;
    Some(editor.protocol.clone())
}

pub(super) fn set_network_proxy_group(
    proxy_editor: &mut Option<NetworkProxyEditorState>,
    group_id: Option<String>,
) -> bool {
    let Some(editor) = proxy_editor.as_mut() else {
        return false;
    };
    editor.group_id = group_id;
    editor.error = None;
    true
}

pub(super) fn set_network_proxy_editor_error(
    proxy_editor: &mut Option<NetworkProxyEditorState>,
    error: String,
) -> bool {
    let Some(editor) = proxy_editor.as_mut() else {
        return false;
    };
    editor.error = Some(error);
    true
}

fn network_section_key(tab: NetworkTab, section_id: &str) -> String {
    match tab {
        NetworkTab::Tunnels => format!("tunnel:{section_id}"),
        NetworkTab::Proxies => format!("proxy:{section_id}"),
    }
}
