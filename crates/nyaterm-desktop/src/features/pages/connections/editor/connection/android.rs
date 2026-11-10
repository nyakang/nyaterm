use super::{ConnectionEditorSectionContext, editor_field};
use crate::features::NyaTermApp;
use crate::models::ConnectionEditorField;
use gpui::{Context, div, prelude::*, rgb};
use nyaterm_ui::NyaSwitch;

pub(super) fn section(
    section: ConnectionEditorSectionContext<'_>,
    snapshot: nyaterm_android::devices::DeviceSnapshot,
    cx: &mut Context<NyaTermApp>,
) -> gpui::Div {
    let ConnectionEditorSectionContext {
        palette,
        editor,
        fields,
        ..
    } = section;
    let editor_usb = editor.android_usb;
    let read_only = editor.android_screen.view_only;
    let mut content = div()
        .flex()
        .flex_col()
        .gap_3()
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child("USB device")
                .child(
                    NyaSwitch::new("android-usb")
                        .checked(editor.android_usb)
                        .on_click(cx.listener(move |app, _, _, cx| {
                            app.connection_state.set_android_editor_usb(!editor_usb);
                            app.connection_state.sync_editor_fields_from_draft(cx);
                            cx.notify();
                        })),
                ),
        )
        .child(editor_field(
            palette,
            if editor.android_usb {
                "Device serial"
            } else {
                "Host"
            },
            ConnectionEditorField::Host,
            fields,
            cx,
        ));
    if !editor.android_usb {
        content = content.child(editor_field(
            palette,
            "Port",
            ConnectionEditorField::Port,
            fields,
            cx,
        ));
    }
    for device in snapshot.devices {
        let serial = device.serial.clone();
        let label = format!(
            "{} · {:?}",
            device.model.as_deref().unwrap_or(&device.serial),
            device.status
        );
        content = content.child(crate::widgets::small_button(
            palette,
            format!("android-device-{}", device.serial),
            label,
            cx.listener(move |app, _, _, cx| {
                if let Some((host, port)) = serial.rsplit_once(':')
                    && port.parse::<u16>().is_ok()
                {
                    app.connection_state.set_android_editor_usb(false);
                    app.apply_connection_editor_field_text(
                        ConnectionEditorField::Host,
                        host.trim_matches(['[', ']']).into(),
                        cx,
                    );
                    app.apply_connection_editor_field_text(
                        ConnectionEditorField::Port,
                        port.into(),
                        cx,
                    );
                } else {
                    app.connection_state.set_android_editor_usb(true);
                    app.apply_connection_editor_field_text(
                        ConnectionEditorField::Host,
                        serial.clone(),
                        cx,
                    );
                }
                app.connection_state.sync_editor_fields_from_draft(cx);
                cx.notify();
            }),
        ));
    }
    content
        .child(editor_field(
            palette,
            "Directory",
            ConnectionEditorField::WorkingDir,
            fields,
            cx,
        ))
        .child(editor_field(
            palette,
            "Screen maximum edge",
            ConnectionEditorField::AndroidMaxSize,
            fields,
            cx,
        ))
        .child(editor_field(
            palette,
            "Maximum FPS",
            ConnectionEditorField::AndroidFps,
            fields,
            cx,
        ))
        .child(editor_field(
            palette,
            "Bit rate (bits/sec)",
            ConnectionEditorField::AndroidBitRate,
            fields,
            cx,
        ))
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child("Read only screen")
                .child(
                    NyaSwitch::new("android-read-only")
                        .checked(editor.android_screen.view_only)
                        .on_click(cx.listener(move |app, _, _, cx| {
                            app.connection_state
                                .set_android_editor_read_only(!read_only);
                            cx.notify();
                        })),
                ),
        )
        .child(div().text_xs().text_color(rgb(palette.text_muted)).child(
            "Authorize USB debugging on the device. TCP connections reuse existing pairing.",
        ))
}
