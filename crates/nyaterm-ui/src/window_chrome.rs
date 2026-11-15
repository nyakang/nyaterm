//! Window chrome widgets. Application policy and close handling stay with callers.

use std::rc::Rc;

use gpui::{
    App, ClickEvent, DispatchPhase, Div, IntoElement, MouseButton, MouseMoveEvent, Pixels, Point,
    PointerCancelEvent, SharedString, Stateful, Window, WindowControlArea, canvas, div, prelude::*,
    px, rgb, svg,
};

use crate::theme::ThemePalette;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TitleDragBehavior {
    Disabled,
    Native,
    MacOS,
    Linux { can_maximize: bool },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TitleDragEvent {
    Pressed,
    StartMove,
    DoubleClick,
}

#[derive(Default)]
struct TitleDragState {
    press: Option<(Point<Pixels>, usize)>,
    moved: bool,
}

impl TitleDragState {
    fn press(&mut self, position: Point<Pixels>, click_count: usize) {
        self.press = Some((position, click_count));
        self.moved = false;
    }

    fn move_pointer(&mut self, position: Point<Pixels>, left_pressed: bool) -> bool {
        if !left_pressed {
            self.cancel();
            return false;
        }
        let Some((origin, _)) = self.press else {
            return false;
        };
        let delta = position - origin;
        let distance_squared = f32::from(delta.x).powi(2) + f32::from(delta.y).powi(2);
        if !self.moved && distance_squared > 16. {
            self.moved = true;
            return true;
        }
        false
    }

    fn release(&mut self, can_maximize: bool) -> bool {
        let double_click =
            self.press.is_some_and(|(_, count)| count == 2) && !self.moved && can_maximize;
        self.cancel();
        double_click
    }

    fn cancel(&mut self) {
        *self = Self::default();
    }
}

/// Attach window dragging to an ordinary title region without adding a layout node.
/// Keyed state belongs to this region in this window; nested regions stop the press
/// before an ancestor can arm a second drag. Interactive children must consume it.
pub fn title_drag_region(
    element: Div,
    id: &'static str,
    behavior: TitleDragBehavior,
    window: &mut Window,
    cx: &mut App,
    on_event: impl Fn(&TitleDragEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let element = element.id(id);
    if behavior == TitleDragBehavior::Disabled {
        return element;
    }
    let element = element.window_control_area(WindowControlArea::Drag);
    let on_event = Rc::new(on_event);
    let TitleDragBehavior::Linux { can_maximize } = behavior else {
        return element.on_mouse_down(MouseButton::Left, move |event, window, cx| {
            on_event(&TitleDragEvent::Pressed, window, cx);
            if behavior == TitleDragBehavior::MacOS {
                cx.stop_propagation();
                if event.click_count == 2
                    && window.is_resizable()
                    && window.window_controls().maximize
                    && !window.is_fullscreen()
                    && !window.is_simple_fullscreen()
                {
                    on_event(&TitleDragEvent::DoubleClick, window, cx);
                }
            }
        });
    };

    let state = window.use_keyed_state(id, cx, |_, _| TitleDragState::default());
    element
        .on_mouse_down(MouseButton::Left, {
            let state = state.clone();
            let on_event = on_event.clone();
            move |event, window, cx| {
                state.update(cx, |state, _| {
                    state.press(event.position, event.click_count)
                });
                on_event(&TitleDragEvent::Pressed, window, cx);
                cx.stop_propagation();
            }
        })
        // Once armed, track the pointer across the window, including outside this
        // region. The canvas registers listeners during paint and owns no hitbox.
        .child(
            canvas(|_, _, _| {}, {
                let state = state.clone();
                let on_event = on_event.clone();
                move |_, _, window, _| {
                    let move_state = state.clone();
                    let on_event = on_event.clone();
                    window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                        if phase != DispatchPhase::Capture {
                            return;
                        }
                        let left_pressed = event.pressed_button == Some(MouseButton::Left)
                            && window.is_window_active()
                            && !window.is_fullscreen()
                            && !window.is_simple_fullscreen();
                        if move_state.update(cx, |state, _| {
                            state.move_pointer(event.position, left_pressed)
                        }) {
                            on_event(&TitleDragEvent::StartMove, window, cx);
                            cx.stop_propagation();
                        }
                    });
                    let cancel_state = state.clone();
                    window.on_mouse_event(move |_: &PointerCancelEvent, phase, _, cx| {
                        if phase == DispatchPhase::Capture {
                            cancel_state.update(cx, |state, _| state.cancel());
                        }
                    });
                }
            })
            .absolute()
            .inset_0(),
        )
        .on_mouse_up(MouseButton::Left, {
            let state = state.clone();
            move |_, window, cx| {
                let can_maximize = can_maximize
                    && window.is_resizable()
                    && window.window_controls().maximize
                    && !window.is_fullscreen()
                    && !window.is_simple_fullscreen();
                if state.update(cx, |state, _| state.release(can_maximize)) {
                    on_event(&TitleDragEvent::DoubleClick, window, cx);
                    cx.stop_propagation();
                }
            }
        })
        .on_mouse_up_out(MouseButton::Left, {
            let state = state.clone();
            move |_, _, cx| state.update(cx, |state, _| state.cancel())
        })
        .on_mouse_down_out(move |_, _, cx| state.update(cx, |state, _| state.cancel()))
}

/// `native_control` selects native caption hit testing, independently of drawing.
/// Explicit child-window callbacks can therefore keep their existing close policy.
/// On Windows the native maximize control owns maximize/restore and does not
/// invoke `on_click`: GPUI's Windows `zoom` only maximizes. Other controls consume
/// the click before native fallback so their application policy runs once.
pub fn window_control_button(
    palette: ThemePalette,
    id: &'static str,
    icon_path: &'static str,
    area: WindowControlArea,
    native_control: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let close = matches!(area, WindowControlArea::Close);
    let hovered_color = if close { 0xffffff } else { palette.text };
    div()
        .id(SharedString::from(id))
        .group(SharedString::from(id))
        .w(px(46.))
        .h_full()
        .flex()
        .items_center()
        .justify_center()
        .text_color(rgb(palette.text_muted))
        .when(native_control, |this| this.window_control_area(area))
        .when(!native_control || !cfg!(target_os = "windows"), |this| {
            this.on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        })
        .cursor_default()
        .hover(move |this| {
            if close {
                this.bg(rgb(0xe81123)).text_color(rgb(0xffffff))
            } else {
                this.bg(rgb(palette.hover)).text_color(rgb(palette.text))
            }
        })
        .child(
            svg()
                .size(px(16.))
                .flex_none()
                .path(icon_path)
                .text_color(rgb(palette.text_muted))
                .group_hover(SharedString::from(id), move |this| {
                    this.text_color(rgb(hovered_color))
                }),
        )
        .when(
            !(native_control
                && cfg!(target_os = "windows")
                && matches!(area, WindowControlArea::Max)),
            |this| {
                this.on_click(move |event, window, cx| {
                    cx.stop_propagation();
                    on_click(event, window, cx);
                })
            },
        )
}

#[cfg(test)]
mod tests;
