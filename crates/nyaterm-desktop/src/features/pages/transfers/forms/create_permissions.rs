use gpui::{
    Context, InteractiveElement as _, IntoElement, ParentElement as _, Styled as _, div, px, rgb,
};
use nyaterm_ui::NyaCheckbox;
use rust_i18n::t;

use crate::features::{NyaTermApp, shell::gpui_code_font_family};
use crate::models::TransferPermissionTarget;
use crate::theme::ThemePalette;

use super::{create_option_section, format_permissions_octal};

impl NyaTermApp {
    pub(super) fn transfer_create_permissions(
        &self,
        palette: ThemePalette,
        mode: u32,
        target: TransferPermissionTarget,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let app = cx.weak_entity();
        let toggle = move |id: &str, label: String, mask: u32| {
            let app = app.clone();
            let checkbox = NyaCheckbox::new(id.to_string())
                .checked(mode & mask != 0)
                .on_click(move |_, _, cx| {
                    let _ = app.update(cx, |app, cx| {
                        app.toggle_transfer_permission_bit(target, mask, cx);
                    });
                });
            if label.is_empty() {
                checkbox
            } else {
                checkbox.label(label)
            }
        };
        let label_cell = |label| {
            div()
                .w(px(64.))
                .flex_none()
                .text_sm()
                .text_color(rgb(palette.text_muted))
                .child(label)
        };
        let permission_cell = || div().flex_1().min_w_0().flex().justify_center();
        let header = div()
            .flex()
            .items_center()
            .gap_2()
            .text_xs()
            .text_color(rgb(palette.text_muted))
            .child(label_cell(String::new()))
            .children(
                [
                    t!("fileExplorer.permRead").to_string(),
                    t!("fileExplorer.permWrite").to_string(),
                    t!("fileExplorer.permExecute").to_string(),
                ]
                .map(|label| permission_cell().child(label)),
            );
        let rows = [
            ("user", t!("fileExplorer.permUser"), [0o400, 0o200, 0o100]),
            ("group", t!("fileExplorer.permGroup"), [0o040, 0o020, 0o010]),
            ("other", t!("fileExplorer.permOther"), [0o004, 0o002, 0o001]),
        ]
        .map(|(owner, label, masks)| {
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(label_cell(label.to_string()))
                .children(
                    ["r", "w", "x"]
                        .into_iter()
                        .zip(masks)
                        .map(|(access, mask)| {
                            let id = format!("transfer-create-perm-{owner}-{access}");
                            permission_cell()
                                .debug_selector({
                                    let id = id.clone();
                                    move || id.clone()
                                })
                                .child(toggle(&id, String::new(), mask))
                        }),
                )
        });

        div()
            .w_full()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(palette.text))
                            .child(t!("fileExplorer.permissions")),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_xs()
                            .text_color(rgb(palette.text_muted))
                            .child(t!("fileExplorer.octal"))
                            .child(
                                div()
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .bg(rgb(palette.surface_elevated))
                                    .font_family(gpui_code_font_family())
                                    .text_color(rgb(palette.text))
                                    .debug_selector(|| "transfer-create-permissions-octal".into())
                                    .child(format_permissions_octal(mode)),
                            ),
                    ),
            )
            .child(div().flex().flex_col().gap_3().child(header).children(rows))
            .child(
                create_option_section(palette)
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(palette.text))
                            .child(t!("fileExplorer.specialPermissions")),
                    )
                    .child(
                        div().flex().flex_wrap().gap_4().children(
                            [
                                ("setuid", "Setuid", 0o4000),
                                ("setgid", "Setgid", 0o2000),
                                ("sticky", "Sticky", 0o1000),
                            ]
                            .map(|(id, label, mask)| {
                                let id = format!("transfer-create-perm-{id}");
                                div()
                                    .debug_selector({
                                        let id = id.clone();
                                        move || id.clone()
                                    })
                                    .child(toggle(&id, label.to_string(), mask))
                            }),
                        ),
                    ),
            )
    }
}
