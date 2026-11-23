use gpui::prelude::{InteractiveElement, ParentElement, Styled};
use gpui::{Context, Div, IntoElement, SharedString, div, px, rgb};
use nyaterm_ui::NyaSwitch;
use rust_i18n::t;

use super::super::super::list::{
    ConnectionEditorRenderContext, connection_editor_select, editor_field,
};
use super::ConnectionEditorSectionContext;
use crate::features::{NyaTermApp, connections::ConnectionEditorToggle};
use crate::models::{ConnectionEditorField, ConnectionEditorSection, ConnectionEditorSelect};

pub(super) fn section(
    section: ConnectionEditorSectionContext<'_>,
    cx: &mut Context<NyaTermApp>,
) -> Div {
    let palette = section.palette;
    let fields = section.fields;
    let settings = &section.editor.spice.settings;
    let field = |label: SharedString, key, cx: &Context<NyaTermApp>| {
        editor_field(palette, label, key, fields, cx).into_any_element()
    };
    let mut content = div()
        .debug_selector(|| "connection-editor-spice-section".to_owned())
        .flex()
        .flex_col()
        .gap_3();
    match section.editor.section {
        ConnectionEditorSection::Basic => {
            content = content
                .child(field(
                    t!("dialog.host").into(),
                    ConnectionEditorField::Host,
                    cx,
                ))
                .child(field(
                    t!("spice.tcpPort").into(),
                    ConnectionEditorField::SpiceTcpPort,
                    cx,
                ))
                .child(field(
                    t!("spice.tlsPort").into(),
                    ConnectionEditorField::SpiceTlsPort,
                    cx,
                ))
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(palette.text_muted))
                        .child(t!("spice.portsHint")),
                )
                .child(super::authentication::password_authentication(section, cx))
                .child(field(
                    t!("credentialManager.usernameOptionalLabel").into(),
                    ConnectionEditorField::Username,
                    cx,
                ));
        }
        ConnectionEditorSection::Network => {
            content = content
                .child(field(
                    t!("spice.httpProxy").into(),
                    ConnectionEditorField::SpiceProxy,
                    cx,
                ))
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(palette.text_muted))
                        .child(t!("spice.proxyHint")),
                );
            for (id, label, select) in [
                (
                    "spice-proxy-profile",
                    t!("dialog.proxySelect"),
                    ConnectionEditorSelect::Proxy,
                ),
                (
                    "spice-jump",
                    t!("dialog.proxyJump"),
                    ConnectionEditorSelect::ProxyJump,
                ),
            ] {
                content = content.child(connection_editor_select(
                    ConnectionEditorRenderContext {
                        palette,
                        fields,
                        cx,
                    },
                    id,
                    label,
                    select,
                ));
            }
        }
        ConnectionEditorSection::Security => {
            content = content
                .debug_selector(|| "connection-editor-spice-security".to_owned())
                .child(switch(
                    "spice-tls",
                    t!("spice.requireTls").into(),
                    settings.security.require_tls,
                    ConnectionEditorToggle::SpiceRequireTls,
                    cx,
                ))
                .child(field(
                    t!("spice.certificateSubject").into(),
                    ConnectionEditorField::SpiceCertificateSubject,
                    cx,
                ))
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(palette.text_muted))
                        .child(t!("spice.caPem")),
                );
            if let Some(input) = fields.get(&ConnectionEditorField::SpiceCaPem) {
                content = content.child(
                    div()
                        .w_full()
                        .h(px(140.))
                        .flex_none()
                        .child(nyaterm_ui::NyaInput::new(input)),
                );
            }
        }
        ConnectionEditorSection::Display => {
            content = content
                .child(switch(
                    "spice-resize",
                    t!("spice.dynamicResolution").into(),
                    settings.dynamic_resolution,
                    ConnectionEditorToggle::SpiceDynamicResolution,
                    cx,
                ))
                .child(switch(
                    "spice-fit",
                    t!("spice.fitWindow").into(),
                    settings.fit_window,
                    ConnectionEditorToggle::SpiceFitWindow,
                    cx,
                ))
                .child(switch(
                    "spice-view-only",
                    t!("spice.viewOnly").into(),
                    settings.view_only,
                    ConnectionEditorToggle::SpiceViewOnly,
                    cx,
                ));
        }
        ConnectionEditorSection::Session => {
            for (id, label, checked, flag) in [
                (
                    "spice-clipboard-send",
                    t!("spice.clipboardToGuest"),
                    settings.clipboard.can_send(),
                    ConnectionEditorToggle::SpiceClipboardToGuest,
                ),
                (
                    "spice-clipboard-receive",
                    t!("spice.clipboardFromGuest"),
                    settings.clipboard.can_receive(),
                    ConnectionEditorToggle::SpiceClipboardFromGuest,
                ),
                (
                    "spice-playback",
                    t!("spice.audioPlayback"),
                    settings.audio_playback,
                    ConnectionEditorToggle::SpiceAudio,
                ),
                (
                    "spice-files",
                    t!("spice.fileTransfer"),
                    settings.file_transfer,
                    ConnectionEditorToggle::SpiceFileTransfer,
                ),
            ] {
                content = content.child(switch(id, label.into(), checked, flag, cx));
            }
            content = content
                .child(field(
                    t!("spice.reconnectAttempts").into(),
                    ConnectionEditorField::SpiceReconnectAttempts,
                    cx,
                ))
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(palette.text_muted))
                        .child(t!("spice.agentHint")),
                );
        }
        _ => {}
    }
    content
}

fn switch(
    id: &'static str,
    label: SharedString,
    checked: bool,
    flag: ConnectionEditorToggle,
    cx: &mut Context<NyaTermApp>,
) -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap_3()
        .child(div().flex_1().text_sm().child(label))
        .child(
            NyaSwitch::new(id)
                .checked(checked)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.toggle_connection_editor_flag(flag, cx);
                })),
        )
}
