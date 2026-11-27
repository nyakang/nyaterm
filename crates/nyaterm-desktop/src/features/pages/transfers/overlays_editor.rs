use rust_i18n::t;

use gpui::{
    AnyElement, App, ClickEvent, Context, FontWeight, IntoElement, KeyDownEvent, SharedString,
    Window, div, prelude::*, px, rgb, rgba,
};
use nyaterm_core::truncate_preview;
use nyaterm_ui::tab_close::NyaTabCloseButton;
use nyaterm_ui::{NyaScrollable, NyaTooltip};

use crate::features::NyaTermApp;
use crate::models::TransferExternalSyncPromptState;

#[derive(Clone, Copy)]
enum ExternalSyncButtonStyle {
    Ghost,
    Outline,
    Primary,
}

#[cfg(test)]
fn editor_surface_shell(standalone: bool) -> gpui::Stateful<gpui::Div> {
    div()
        .id("transfer-editor-overlay")
        .when(!standalone, |this| {
            this.absolute().top_0().bottom_0().left_0().right_0()
        })
        .when(standalone, |this| this.size_full())
}

#[cfg(test)]
fn editor_dialog_shell() -> gpui::Stateful<gpui::Div> {
    div()
        .id("transfer-editor-dialog")
        .on_click(|_, _, cx| cx.stop_propagation())
}

fn external_sync_button(
    palette: crate::theme::ThemePalette,
    id: &'static str,
    label: impl Into<SharedString>,
    style: ExternalSyncButtonStyle,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let label: SharedString = label.into();
    let transparent = rgba(0x00000000);
    let (background, border, text) = match style {
        ExternalSyncButtonStyle::Ghost => (transparent, transparent, rgb(palette.text_muted)),
        ExternalSyncButtonStyle::Outline => {
            (rgb(palette.bg), rgb(palette.border), rgb(palette.text))
        }
        ExternalSyncButtonStyle::Primary => (rgb(palette.link), rgb(palette.link), rgb(palette.bg)),
    };
    div()
        .id(SharedString::from(id))
        .h(px(28.))
        .px_3()
        .flex()
        .items_center()
        .rounded_sm()
        .border_1()
        .border_color(border)
        .bg(background)
        .text_color(text)
        .text_xs()
        .cursor_pointer()
        .hover(move |this| match style {
            ExternalSyncButtonStyle::Primary => this.bg(rgba((palette.link << 8) | 0xe8)),
            ExternalSyncButtonStyle::Ghost | ExternalSyncButtonStyle::Outline => {
                this.bg(rgb(palette.hover)).text_color(rgb(palette.text))
            }
        })
        .child(label)
        .on_click(on_click)
}

impl NyaTermApp {
    pub(in crate::features) fn transfer_external_sync_prompt_overlay(
        &mut self,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some((prompt_id, prompt)) = self.active_external_editor_sync_prompt() else {
            return div().into_any_element();
        };
        self.transfer_external_sync_prompt_surface(prompt_id, prompt, false, cx)
    }

    pub(in crate::features) fn transfer_external_sync_window_view(
        &mut self,
        prompt_id: String,
        prompt: TransferExternalSyncPromptState,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.transfer_external_sync_prompt_surface(prompt_id, prompt, true, cx)
    }

    fn transfer_external_sync_prompt_surface(
        &mut self,
        prompt_id: String,
        prompt: TransferExternalSyncPromptState,
        standalone: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = self.theme_palette();
        let title = t!("fileExplorer.fileModified");
        let conflict = self
            .transfer
            .external_edit_target(&prompt.job_id)
            .is_some_and(|target| target.has_conflict());
        let prompt_label = if conflict {
            t!("fileTransfer.externalConflict")
        } else {
            t!("fileExplorer.uploadPrompt")
        };
        let cancel_label = t!("common.cancel");
        let always_label = t!("fileExplorer.alwaysUpload");
        let upload_once_label = t!("fileExplorer.uploadOnce");
        let ignore_prompt_id = prompt_id.clone();
        let always_prompt_id = prompt_id.clone();
        let upload_prompt_id = prompt_id.clone();

        div()
            .id(SharedString::from("transfer-external-sync-overlay"))
            .when(!standalone, |this| {
                this.absolute()
                    .top_0()
                    .bottom_0()
                    .left_0()
                    .right_0()
                    .bg(rgba(0x00000080))
                    .p_3()
            })
            .when(standalone, |this| this.size_full().bg(rgb(palette.bg)))
            .flex()
            .items_center()
            .justify_center()
            .track_focus(self.transfer.external_sync_focus())
            .on_click(cx.listener(|this, _, window, cx| {
                window.focus(this.transfer.external_sync_focus(), cx);
                cx.notify();
            }))
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _, cx| {
                cx.stop_propagation();
                if event.keystroke.key.as_str() == "escape" {
                    this.ignore_external_editor_sync_prompt(&ignore_prompt_id, cx);
                }
            }))
            .child(
                div()
                    .w_full()
                    .when(standalone, |this| this.size_full())
                    .when(!standalone, |this| {
                        this.max_w(px(440.))
                            .rounded_md()
                            .border_1()
                            .border_color(rgb(palette.border))
                            .shadow_lg()
                    })
                    .bg(if standalone {
                        rgb(palette.bg)
                    } else {
                        self.shell_surface_color(palette.bg)
                    })
                    .p_4()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .when(!standalone, |this| {
                        this.child(div().text_sm().font_weight(FontWeight(700.)).child(title))
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .text_color(rgb(palette.text))
                                    .child(prompt_label),
                            )
                            .child(
                                div()
                                    .rounded_md()
                                    .border_1()
                                    .border_color(rgb(palette.border))
                                    .bg(rgb(palette.input))
                                    .px_3()
                                    .py_2()
                                    .font_family(crate::features::shell::gpui_code_font_family())
                                    .text_xs()
                                    .text_color(rgb(palette.text_muted))
                                    .child(truncate_preview(&prompt.remote_path, 120)),
                            ),
                    )
                    .child(
                        div()
                            .pt_3()
                            .border_t_1()
                            .border_color(rgb(palette.border))
                            .flex()
                            .justify_end()
                            .gap_2()
                            .child(external_sync_button(
                                palette,
                                "transfer-external-sync-ignore",
                                cancel_label,
                                ExternalSyncButtonStyle::Ghost,
                                cx.listener(move |this, _, _, cx| {
                                    this.ignore_external_editor_sync_prompt(&prompt_id, cx);
                                }),
                            ))
                            .child(external_sync_button(
                                palette,
                                "transfer-external-sync-always",
                                always_label,
                                ExternalSyncButtonStyle::Outline,
                                cx.listener(move |this, _, _, cx| {
                                    this.upload_external_editor_sync_prompt(
                                        &always_prompt_id,
                                        true,
                                        cx,
                                    );
                                }),
                            ))
                            .child(external_sync_button(
                                palette,
                                "transfer-external-sync-upload",
                                upload_once_label,
                                ExternalSyncButtonStyle::Primary,
                                cx.listener(move |this, _, _, cx| {
                                    this.upload_external_editor_sync_prompt(
                                        &upload_prompt_id,
                                        false,
                                        cx,
                                    );
                                }),
                            )),
                    ),
            )
            .into_any_element()
    }

    pub(in crate::features) fn transfer_editor_window_view(
        &mut self,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(workspace) = self.transfer.window_editor_workspace() else {
            return div().into_any_element();
        };
        let palette = self.theme_palette();
        let mut tabs = div()
            .id("editor-window-tabs")
            .h(px(36.))
            .flex_none()
            .flex()
            .items_center()
            .overflow_x_scrollbar();
        for document in &workspace.tabs {
            let id = document.id.clone();
            let close_id = id.clone();
            let active = id == workspace.active_tab_id;
            let tab_group = SharedString::from(format!("editor-window-tab-group-{id}"));
            let path = document.remote_path.clone();
            tabs = tabs.child(
                div()
                    .id(SharedString::from(format!("editor-window-tab-{id}")))
                    .group(tab_group.clone())
                    .h_full()
                    .flex_none()
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_xs()
                    .bg(rgb(if active { palette.bg } else { palette.surface }))
                    .hover(move |style| style.bg(rgb(palette.hover)))
                    .tooltip(move |window, cx| NyaTooltip::new(path.clone()).build(window, cx))
                    .on_click(
                        cx.listener(move |app, _, _, cx| app.activate_transfer_editor_tab(&id, cx)),
                    )
                    .child(document.name.clone())
                    .child(
                        NyaTabCloseButton::new(
                            format!("editor-window-tab-close-{close_id}"),
                            tab_group,
                            palette,
                            if document.dirty {
                                format!("{} · {}", t!("common.close"), t!("fileEditor.unsaved"))
                            } else {
                                t!("common.close").to_string()
                            },
                        )
                        .active(active)
                        .dirty(document.dirty)
                        .on_click(cx.listener(move |app, _, _, cx| {
                            app.close_transfer_editor_tab(&close_id, cx);
                        })),
                    ),
            );
        }
        div()
            .id("editor-window-surface")
            .size_full()
            .min_h_0()
            .min_w_0()
            .flex()
            .flex_col()
            .child(tabs)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .child(self.workspace_editor_content(&workspace.active_tab_id, cx)),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use gpui::{
        Context, InteractiveElement as _, IntoElement, Modifiers, ParentElement as _, Render,
        StatefulInteractiveElement as _, Styled as _, TestAppContext, VisualTestContext, Window,
        div, point, px,
    };

    use super::{editor_dialog_shell, editor_surface_shell};

    struct EditorWindowLayoutFixture {
        header_clicks: Arc<AtomicUsize>,
    }

    impl Render for EditorWindowLayoutFixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let header_clicks = self.header_clicks.clone();
            div()
                .size_full()
                .flex()
                .flex_col()
                .child(
                    div()
                        .id("editor-window-header")
                        .h(px(40.))
                        .flex_none()
                        .debug_selector(|| "editor-window-header".to_string())
                        .on_click(move |_, _, _| {
                            header_clicks.fetch_add(1, Ordering::SeqCst);
                        }),
                )
                .child(
                    div().flex_1().min_h_0().overflow_hidden().child(
                        editor_surface_shell(true)
                            .debug_selector(|| "editor-window-surface".to_string())
                            .on_click(|_, _, _| {}),
                    ),
                )
        }
    }

    #[gpui::test]
    fn standalone_editor_surface_stays_below_window_header(cx: &mut TestAppContext) {
        let header_clicks = Arc::new(AtomicUsize::new(0));
        let fixture = EditorWindowLayoutFixture {
            header_clicks: header_clicks.clone(),
        };
        let (_, cx) = cx.add_window_view(|_, _| fixture);
        let cx: &mut VisualTestContext = cx;
        cx.run_until_parked();
        cx.update(|window, cx| {
            _ = window.draw(cx);
        });

        let header = cx.debug_bounds("editor-window-header").unwrap();
        let surface = cx.debug_bounds("editor-window-surface").unwrap();
        assert_eq!(surface.top(), header.bottom());
        cx.simulate_click(point(px(20.), px(20.)), Modifiers::default());
        assert_eq!(header_clicks.load(Ordering::SeqCst), 1);
    }

    struct EditorFocusBoundaryFixture {
        shell_clicks: Arc<AtomicUsize>,
        content_clicks: Arc<AtomicUsize>,
    }

    impl Render for EditorFocusBoundaryFixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let shell_clicks = self.shell_clicks.clone();
            let content_clicks = self.content_clicks.clone();
            div()
                .id("editor-focus-boundary-shell")
                .size_full()
                .on_click(move |_, _, _| {
                    shell_clicks.fetch_add(1, Ordering::SeqCst);
                })
                .child(
                    editor_dialog_shell().size_full().child(
                        div()
                            .id("editor-content-click-target")
                            .size_full()
                            .debug_selector(|| "editor-content-click-target".to_string())
                            .on_click(move |_, _, _| {
                                content_clicks.fetch_add(1, Ordering::SeqCst);
                            }),
                    ),
                )
        }
    }

    #[gpui::test]
    fn editor_content_click_does_not_reach_shell_focus_handler(cx: &mut TestAppContext) {
        let shell_clicks = Arc::new(AtomicUsize::new(0));
        let content_clicks = Arc::new(AtomicUsize::new(0));
        let fixture = EditorFocusBoundaryFixture {
            shell_clicks: shell_clicks.clone(),
            content_clicks: content_clicks.clone(),
        };
        let (_, cx) = cx.add_window_view(|_, _| fixture);
        let cx: &mut VisualTestContext = cx;
        cx.run_until_parked();
        cx.update(|window, cx| {
            _ = window.draw(cx);
        });
        let bounds = cx.debug_bounds("editor-content-click-target").unwrap();
        cx.simulate_click(bounds.center(), Modifiers::default());
        assert_eq!(content_clicks.load(Ordering::SeqCst), 1);
        assert_eq!(shell_clicks.load(Ordering::SeqCst), 0);
    }
}
