use rust_i18n::t;

use gpui::{
    App, ClickEvent, Context, InteractiveElement as _, IntoElement, MouseDownEvent,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    px, rgb, svg,
};
use nyaterm_ui::NyaScrollable;

use crate::features::NyaTermApp;
use crate::models::{TransferBrowserUploadMenuState, TransferPathPromptKind};
use crate::theme::ThemePalette;

use super::transfer_menu_position;

impl NyaTermApp {
    pub(in crate::features::pages::transfers) fn open_transfer_browser_upload_menu(
        &mut self,
        event: &MouseDownEvent,
        cx: &mut Context<Self>,
    ) {
        self.transfer
            .open_browser_upload_menu(TransferBrowserUploadMenuState {
                x: event.position.x,
                y: event.position.y + px(22.),
            });
        cx.notify();
    }

    pub(in crate::features) fn close_transfer_browser_upload_menu(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        self.transfer.close_browser_upload_menu();
        cx.notify();
    }

    pub(in crate::features) fn transfer_browser_upload_menu_overlay(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let state =
            self.transfer
                .browser_view()
                .upload_menu
                .unwrap_or(TransferBrowserUploadMenuState {
                    x: px(24.),
                    y: px(24.),
                });
        let (viewport_w, viewport_h) = self.shell.viewport_size();
        let (menu_x, menu_y, menu_max_height) = transfer_menu_position(
            f32::from(state.x),
            f32::from(state.y),
            176.,
            80.,
            viewport_w,
            viewport_h,
        );

        div()
            .id(SharedString::from("transfer-browser-upload-menu-overlay"))
            .absolute()
            .top_0()
            .bottom_0()
            .left_0()
            .right_0()
            .on_click(cx.listener(|this, _, _, cx| {
                this.close_transfer_browser_upload_menu(cx);
            }))
            .child(
                div()
                    .id(SharedString::from("transfer-browser-upload-menu"))
                    .absolute()
                    .top(px(menu_y))
                    .left(px(menu_x))
                    .w(px(176.))
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(palette.border))
                    .bg(self.shell_surface_color(palette.surface))
                    .shadow_lg()
                    .on_click(|_, _, cx| {
                        cx.stop_propagation();
                    })
                    .child(
                        div()
                            .max_h(px(menu_max_height))
                            .overflow_y_scrollbar()
                            .py_1()
                            .flex()
                            .flex_col()
                            .child(upload_menu_item(
                                palette,
                                "transfer-browser-upload-menu-files",
                                "icons/fe/upload.svg",
                                t!("fileExplorer.upload"),
                                cx.listener(|this, _, _, cx| {
                                    this.close_transfer_browser_upload_menu(cx);
                                    this.prompt_transfer_browser_upload_path(
                                        TransferPathPromptKind::UploadFile,
                                        cx,
                                    );
                                }),
                            ))
                            .child(upload_menu_item(
                                palette,
                                "transfer-browser-upload-menu-folder",
                                "icons/fe/upload-folder.svg",
                                t!("fileExplorer.uploadFolder"),
                                cx.listener(|this, _, _, cx| {
                                    this.close_transfer_browser_upload_menu(cx);
                                    this.prompt_transfer_browser_upload_path(
                                        TransferPathPromptKind::UploadDirectory,
                                        cx,
                                    );
                                }),
                            ))
                            .child(upload_menu_item(
                                palette,
                                "transfer-browser-upload-menu-folder-contents",
                                "icons/fe/upload-folder.svg",
                                t!("fileExplorer.uploadFolderContents"),
                                cx.listener(|this, _, _, cx| {
                                    this.close_transfer_browser_upload_menu(cx);
                                    this.prompt_transfer_browser_upload_path(
                                        TransferPathPromptKind::UploadDirectoryContents,
                                        cx,
                                    );
                                }),
                            )),
                    ),
            )
    }
}

fn upload_menu_item(
    palette: ThemePalette,
    id: impl Into<String>,
    icon_path: &'static str,
    label: impl Into<SharedString>,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let label: SharedString = label.into();
    div()
        .id(SharedString::from(id.into()))
        .h(px(30.))
        .px_2()
        .mx_1()
        .rounded_sm()
        .flex()
        .items_center()
        .gap_2()
        .cursor_pointer()
        .text_size(px(12.))
        .text_color(rgb(palette.text))
        .hover(|this| {
            this.bg(rgb(palette.surface_elevated))
                .text_color(rgb(0xffffff))
        })
        .child(
            svg()
                .size(px(16.))
                .flex_none()
                .path(icon_path)
                .text_color(rgb(palette.text_muted)),
        )
        .child(label)
        .on_click(on_click)
}
