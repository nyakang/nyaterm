mod interaction;
mod menu;
mod navigation;
mod path_navigation;
mod queue;
mod table;

use gpui::{
    AnyElement, App, ClickEvent, Context, CursorStyle, IntoElement, KeyDownEvent, MouseButton,
    MouseDownEvent, ParentElement as _, Render, SharedString, Styled as _, Window, div, prelude::*,
    px, relative, rgb,
};
use nyaterm_ui::{NyaButton, NyaButtonVariant};
use rust_i18n::t;

use super::pane::PaneId;
use super::workspace::FileManagerWorkspace;
use crate::features::NyaTermApp;
use crate::features::transfers::file_operations::FileTransferOperation;
use crate::theme::ThemePalette;

#[derive(Clone, Copy)]
pub(in crate::features) enum FileManagerNameKind {
    Folder,
    File,
    Rename,
}

impl FileManagerWorkspace {
    fn read_app<R: Default>(&self, cx: &App, f: impl FnOnce(&NyaTermApp) -> R) -> R {
        self.app
            .upgrade()
            .map(|app| f(app.read(cx)))
            .unwrap_or_default()
    }

    fn dispatch_app(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut NyaTermApp, &mut Window, &mut Context<NyaTermApp>) + 'static,
    ) {
        let app = self.app.clone();
        // The workspace is leased during listeners. Enter the app only after
        // releasing that lease, with all file-operation arguments already frozen.
        window.defer(cx, move |window, cx| {
            let _ = app.update(cx, |app, cx| f(app, window, cx));
        });
    }

    fn finish_resize(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.dragging_split || self.column_resize.is_some() || self.queue_resize.is_some() {
            self.dragging_split = false;
            self.column_resize = None;
            self.queue_resize = None;
            self.save_layout(window, cx);
            cx.notify();
        }
    }

    fn save_layout(&self, window: &Window, cx: &mut Context<Self>) {
        self.dispatch_app(window, cx, |app, _, cx| {
            app.persist_file_manager_settings(cx)
        });
    }

    fn send_transfer(
        &self,
        operation: FileTransferOperation,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let intent = self.transfer_intent(operation);
        let id = self.id.clone();
        self.dispatch_app(window, cx, move |app, _, cx| {
            app.submit_file_transfer(id, intent, cx)
        });
    }

    fn open_name_dialog(
        &self,
        pane: PaneId,
        kind: FileManagerNameKind,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let selection = self.selection(pane);
        let id = self.id.clone();
        self.dispatch_app(window, cx, move |app, window, cx| {
            app.open_file_manager_name_target(id, selection, kind, window, cx)
        });
    }

    fn delete_selection(&self, window: &Window, cx: &mut Context<Self>) {
        let selection = self.selection(self.active);
        let id = self.id.clone();
        self.dispatch_app(window, cx, move |app, window, cx| {
            app.confirm_file_manager_delete_selection(id, selection, window, cx)
        });
    }

    fn pane_switcher(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut switches = div()
            .h(px(28.))
            .flex_none()
            .flex()
            .items_center()
            .gap_1()
            .px_2();
        for pane in [PaneId::Left, PaneId::Right] {
            switches = switches.child(
                NyaButton::new(
                    format!("fm-show-{}", pane.as_str()),
                    t!(if pane == PaneId::Left {
                        "fileManager.leftPane"
                    } else {
                        "fileManager.rightPane"
                    })
                    .to_string(),
                )
                .small()
                .variant(if self.active == pane {
                    NyaButtonVariant::Primary
                } else {
                    NyaButtonVariant::Ghost
                })
                .on_click(
                    cx.listener(move |this, _, window, cx| this.focus_pane(pane, window, cx)),
                ),
            );
        }
        switches.into_any_element()
    }

    fn measure(&self, pane: Option<PaneId>, cx: &mut Context<Self>) -> impl IntoElement {
        let workspace = cx.weak_entity();
        gpui::canvas(
            move |bounds, _, cx| {
                let Some(workspace) = workspace.upgrade() else {
                    return;
                };
                let previous = match pane {
                    None => workspace.read(cx).bounds,
                    Some(pane) => {
                        workspace.read(cx).pane_bounds[if pane == PaneId::Left { 0 } else { 1 }]
                    }
                };
                if previous == Some(bounds) {
                    return;
                }
                cx.defer(move |cx| {
                    workspace.update(cx, |this, cx| {
                        match pane {
                            None => {
                                this.bounds = Some(bounds);
                                this.single_pane = f32::from(bounds.size.width) < 568.;
                            }
                            Some(pane) => {
                                this.pane_bounds[if pane == PaneId::Left { 0 } else { 1 }] =
                                    Some(bounds)
                            }
                        }
                        cx.notify();
                    })
                });
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0()
    }

    fn pane_width(&self, pane: PaneId) -> f32 {
        self.pane_bounds[if pane == PaneId::Left { 0 } else { 1 }]
            .map(|bounds| f32::from(bounds.size.width))
            .unwrap_or(400.)
    }

    fn panes(&mut self, palette: ThemePalette, cx: &mut Context<Self>) -> AnyElement {
        let mut body = div().flex_1().min_h_0().min_w_0().flex();
        if self.single_pane {
            body = body.child(self.pane_view(palette, self.active, cx));
        } else {
            for pane in [PaneId::Left, PaneId::Right] {
                if pane == PaneId::Right {
                    body = body.child(self.splitter(palette, cx));
                }
                let fraction = if pane == PaneId::Left {
                    self.split_percent
                } else {
                    100 - self.split_percent
                };
                body = body.child(
                    div()
                        .min_w_0()
                        .min_h_0()
                        .flex_grow(1.)
                        .flex_shrink(1.)
                        .flex_basis(relative(f32::from(fraction) / 100.))
                        .child(self.pane_view(palette, pane, cx)),
                );
            }
        }
        body.into_any_element()
    }

    fn splitter(&self, palette: ThemePalette, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("file-manager-splitter")
            .w(px(6.))
            .h_full()
            .flex_none()
            .cursor(CursorStyle::ResizeLeftRight)
            .bg(rgb(palette.border))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    if event.click_count >= 2 {
                        this.split_percent = 50;
                        this.dragging_split = false;
                        this.save_layout(window, cx);
                    } else {
                        this.dragging_split = true;
                    }
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
    }
}

impl Render for FileManagerWorkspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.focus_pending {
            self.focus_pending = false;
            self.focus_pane(self.active, window, cx);
        }
        let Some(app) = self.app.upgrade() else {
            return div().into_any_element();
        };
        let palette = app.read(cx).theme_palette();
        div()
            .id(SharedString::from(format!("file-manager-{}", self.id)))
            .size_full()
            .min_w_0()
            .min_h_0()
            .relative()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(rgb(palette.bg))
            .text_color(rgb(palette.text))
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                this.handle_key(event, window, cx)
            }))
            .on_modifiers_changed(|_, window, _| window.refresh())
            .on_mouse_move(cx.listener(|this, event: &gpui::MouseMoveEvent, _, cx| {
                if this.dragging_split
                    && let Some(bounds) = this.bounds
                {
                    this.split_percent = ((f32::from(event.position.x - bounds.origin.x)
                        / f32::from(bounds.size.width).max(1.))
                        * 100.)
                        .clamp(20., 80.)
                        .round() as u8;
                    cx.notify();
                }
                if let Some((pane, column, x, width)) = this.column_resize {
                    this.pane_mut(pane).column_widths.set(
                        column,
                        px((f32::from(width) + f32::from(event.position.x) - x).clamp(72., 2000.)),
                    );
                    cx.notify();
                }
                if let Some((y, height)) = this.queue_resize {
                    this.queue_height = (height + y - f32::from(event.position.y)).clamp(80., 600.);
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| this.finish_resize(window, cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| this.finish_resize(window, cx)),
            )
            .child(self.measure(None, cx))
            .when(self.single_pane, |view| view.child(self.pane_switcher(cx)))
            .child(self.panes(palette, cx))
            .child(self.queue_view(palette, cx))
            .into_any_element()
    }
}

fn small_button(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> NyaButton {
    NyaButton::new(id, label)
        .small()
        .variant(NyaButtonVariant::Ghost)
        .on_click(on_click)
}

fn icon_button(
    palette: ThemePalette,
    id: impl Into<SharedString>,
    icon: &'static str,
    tooltip: impl Into<SharedString>,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> NyaButton {
    small_button(id, "", on_click)
        .icon(icon)
        .compact()
        .height(px(24.))
        .text_color(rgb(palette.text_muted))
        .tooltip(tooltip)
}

#[cfg(test)]
mod tests;
