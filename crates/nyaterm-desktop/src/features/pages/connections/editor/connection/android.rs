use super::super::super::list::{editor_stepper_field, required};
use super::{ConnectionEditorSectionContext, editor_field};
use crate::features::NyaTermApp;
use crate::features::android::labels::device_status_label;
use crate::models::{ConnectionEditorField, ConnectionEditorSection};
use gpui::{Context, div, prelude::*, px, rgb};
use nyaterm_ui::NyaSwitch;
use rust_i18n::t;

pub(super) fn section(
    section: ConnectionEditorSectionContext<'_>,
    snapshot: nyaterm_android::devices::DeviceSnapshot,
    cx: &mut Context<NyaTermApp>,
) -> gpui::Div {
    let ConnectionEditorSectionContext {
        palette,
        editor,
        fields,
        narrow,
        ..
    } = section;
    if editor.section == ConnectionEditorSection::AndroidScreen {
        return div()
            .flex()
            .flex_col()
            .gap_3()
            .child(editor_stepper_field(
                palette,
                t!("android.maxSize"),
                ConnectionEditorField::AndroidMaxSize,
                fields,
                cx,
            ))
            .child(editor_stepper_field(
                palette,
                t!("android.maxFps"),
                ConnectionEditorField::AndroidFps,
                fields,
                cx,
            ))
            .child(editor_stepper_field(
                palette,
                t!("android.bitRate"),
                ConnectionEditorField::AndroidBitRate,
                fields,
                cx,
            ));
    }
    if editor.section != ConnectionEditorSection::Basic {
        return div();
    }
    let editor_usb = editor.android_usb;
    let mut content =
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(t!("android.usbDevice"))
                    .child(NyaSwitch::new("android-usb").checked(editor_usb).on_click(
                        cx.listener(move |app, _, _, cx| {
                            app.connection_state.set_android_editor_usb(!editor_usb);
                            app.connection_state.sync_editor_fields_from_draft(cx);
                            cx.notify();
                        }),
                    )),
            )
            .child(
                div()
                    .flex()
                    .when(narrow, |this| this.flex_col())
                    .gap_3()
                    .child(div().min_w_0().flex_1().child(editor_field(
                        palette,
                        required(if editor_usb {
                            t!("android.deviceSerial")
                        } else {
                            t!("dialog.host")
                        }),
                        ConnectionEditorField::Host,
                        fields,
                        cx,
                    )))
                    .when(!editor_usb, |this| {
                        this.child(
                            div()
                                .when(!narrow, |this| this.w(px(150.)).flex_none())
                                .when(narrow, |this| this.w_full())
                                .child(editor_stepper_field(
                                    palette,
                                    required(t!("dialog.port")),
                                    ConnectionEditorField::Port,
                                    fields,
                                    cx,
                                )),
                        )
                    }),
            );
    for device in snapshot.devices {
        let serial = device.serial.clone();
        let label = format!(
            "{} · {}",
            device.model.as_deref().unwrap_or(&device.serial),
            device_status_label(device.status)
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
            t!("android.directory"),
            ConnectionEditorField::WorkingDir,
            fields,
            cx,
        ))
        .child(
            div()
                .text_xs()
                .text_color(rgb(palette.text_muted))
                .child(t!("android.authorizationHint")),
        )
}
