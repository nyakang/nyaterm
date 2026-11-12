use super::labels::AndroidError;
use crate::features::NyaTermApp;
use crate::features::view_widgets::color_icon;
use crate::widgets::small_button;
use gpui::{
    Bounds, Context, DevicePixels, IntoElement, KeyDownEvent, MouseButton, Pixels, Point, Size,
    Window, canvas, div, prelude::*, px, rgb,
};
use nyaterm_android::ipc::Control;
use nyaterm_core::models::android::{AndroidViewKind, screen_point};
use rust_i18n::t;
use std::time::{Duration, Instant};

impl NyaTermApp {
    pub(in crate::features) fn android_screen_button(
        &self,
        id: String,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let available = self.android_screen_button_available(&id);
        div()
            .id(format!("android-screen-button-overlay-{id}"))
            .absolute()
            .occlude()
            .top_2()
            .right_2()
            .on_any_mouse_down(|_, _, cx| cx.stop_propagation())
            .on_mouse_up(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(|_, _, cx| cx.stop_propagation())
            .child(
                nyaterm_ui::NyaButton::new(
                    format!("android-screen-button-{id}"),
                    t!("android.screen"),
                )
                .content(color_icon("color/session/scrcpy.svg", 22.))
                .small()
                .height(px(30.))
                .tooltip(t!("android.screen"))
                .disabled(!available)
                .on_click(cx.listener(move |app, _, _, cx| {
                    cx.stop_propagation();
                    app.open_android_screen_from_terminal(&id, cx);
                })),
            )
    }

    pub(in crate::features) fn android_view(
        &mut self,
        id: String,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        if self.android.kind(&id) == Some(AndroidViewKind::Files) {
            return self
                .file_manager
                .workspace(&format!("sftp-{id}"))
                .map(|w| w.into_any_element())
                .unwrap_or_else(|| {
                    div()
                        .child(t!("android.filesUnavailable"))
                        .into_any_element()
                });
        }
        let app = cx.weak_entity();
        self.android
            .surfaces
            .entry(id.clone())
            .or_insert_with(|| cx.new(|cx| super::surface::AndroidSurface::new(app, id, cx)))
            .clone()
            .into_any_element()
    }
    pub(super) fn poll_android_screen(&mut self, id: &str, cx: &mut Context<Self>) -> bool {
        let expected_binding = self
            .android_config_for_session(id)
            .map(|config| config.binding.generation);
        let Some(screen) = self.android.screens.get_mut(id) else {
            return false;
        };
        let Some(handle) = screen.handle.as_ref() else {
            return false;
        };
        let mut changed = false;
        let mut input_rejected = None;
        for event in handle.events() {
            match event {
                Control::Ready {
                    width,
                    height,
                    generation,
                } if generation > screen.generation => {
                    let _ = handle.send(Control::ReleaseInputs);
                    screen.drag = false;
                    screen.physical_keys.clear();
                    screen.bounds = None;
                    screen.generation = generation;
                    screen.dimensions = (width, height);
                    screen.layout_epoch += 1;
                    screen.pan_drag = None;
                    screen.viewport.pan = (0.0, 0.0);
                    if let Some(texture) = screen.texture.take() {
                        self.android.retired_textures.push(texture);
                    }
                    screen.texture_size = None;
                    if let Some(frame) = screen.frame.take() {
                        handle.release_frame(frame);
                    }
                    changed = true;
                }
                Control::Fatal { code } => {
                    screen.error = Some(AndroidError::Fatal(code));
                    screen.drag = false;
                    screen.physical_keys.clear();
                    changed = true;
                }
                Control::InputRejected { code, .. } => {
                    input_rejected = Some(code);
                    changed = true;
                }
                Control::Disconnected => {
                    screen.error = Some(AndroidError::Disconnected);
                    changed = true;
                }
                _ => {}
            }
        }
        if let Some(frame) = handle.take_frame() {
            if Some(frame.binding_generation) != expected_binding
                || frame.generation < screen.generation
            {
                handle.release_frame(frame);
                return changed;
            }
            if frame.generation != screen.generation {
                let _ = handle.send(Control::ReleaseInputs);
                screen.drag = false;
                screen.physical_keys.clear();
                screen.bounds = None;
                screen.layout_epoch += 1;
                screen.pan_drag = None;
                if let Some(texture) = screen.texture.take() {
                    self.android.retired_textures.push(texture);
                }
                screen.texture_size = None;
            }
            screen.generation = frame.generation;
            screen.dimensions = (frame.capture_width, frame.capture_height);
            if let Some(old) = screen.frame.replace(frame) {
                handle.release_frame(old);
            }
            if !screen.visible
                && let Some(frame) = screen.frame.take()
            {
                handle.release_frame(frame);
            }
            changed = true;
        }
        if screen.error.is_some()
            && let Some(metadata) = self.session.metadata_mut(id)
        {
            metadata.disconnected = true;
        }
        let failure = screen
            .error
            .as_ref()
            .filter(|_| !screen.error_reported)
            .map(|error| (screen.attempt.to_string(), error.code(), error.message()));
        if failure.is_some() {
            screen.error_reported = true;
        }
        if let Some((request, code, message)) = failure {
            self.report_android_failure("screen", &request, code, message, cx);
        }
        if let Some(code) = input_rejected {
            self.android_input_notice(id, false, Some(code), cx);
        }
        changed
    }
    pub(in crate::features) fn sync_android_visibility(&mut self, cx: &mut Context<Self>) {
        let visible: std::collections::HashSet<String> = self
            .visible_terminal_session_ids()
            .into_iter()
            .map(str::to_owned)
            .collect();
        let mut failures = Vec::new();
        for (id, screen) in &mut self.android.screens {
            let next = visible.contains(id)
                && self.shell.selected_nav() == crate::models::NavItem::Workspace;
            if screen.visible != next {
                screen.visible = next;
                if let Some(handle) = &screen.handle {
                    if handle
                        .send(Control::Presentation { visible: next })
                        .is_err()
                    {
                        failures.push(id.clone());
                    }
                    if !next && let Some(frame) = screen.frame.take() {
                        handle.release_frame(frame);
                    }
                }
            }
        }
        for id in failures {
            self.android_input_notice(&id, false, None, cx);
        }
    }
    pub(super) fn prepare_android_texture(&mut self, id: &str, window: &mut Window) {
        for texture in self.android.retired_textures.drain(..) {
            window.remove_dynamic_texture(texture);
        }
        let Some(screen) = self.android.screens.get_mut(id) else {
            return;
        };
        if !screen.visible {
            return;
        }
        let Some(frame) = screen.frame.take() else {
            return;
        };
        let dimensions = (frame.width, frame.height);
        let size = Size::new(
            DevicePixels(frame.width as i32),
            DevicePixels(frame.height as i32),
        );
        let result = if screen.texture_size != Some(dimensions) {
            if let Some(texture) = screen.texture.take() {
                window.remove_dynamic_texture(texture);
            }
            window
                .create_dynamic_texture(size, &frame.pixels, frame.stride)
                .map(|texture| {
                    screen.texture = Some(texture);
                    screen.texture_size = Some(dimensions);
                })
        } else if let Some(texture) = screen.texture {
            window.update_dynamic_texture(
                texture,
                Bounds::new(Point::new(DevicePixels(0), DevicePixels(0)), size),
                &frame.pixels,
                frame.stride,
            )
        } else {
            Ok(())
        };
        if result.is_err() {
            screen.error = Some(AndroidError::TextureUpload);
            if let Some(handle) = &screen.handle {
                handle.close();
            }
        }
        if let Some(handle) = &screen.handle {
            handle.release_frame(frame);
        }
    }
    pub(super) fn android_screen_content(
        &mut self,
        id: String,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let palette = self.theme_palette();
        let Some(screen) = self.android.screens.get(&id) else {
            return div().child(t!("android.screenClosed")).into_any_element();
        };
        let texture = screen.texture;
        let dimensions = screen.dimensions;
        let geometry_generation = screen.generation;
        let viewport = screen.viewport;
        let layout_epoch = screen.layout_epoch;
        let error = screen.error.as_ref().map(AndroidError::message);
        let mut toolbar = div()
            .id(format!("android-toolbar-{id}"))
            .flex()
            .items_center()
            .gap_1()
            .h(px(36.))
            .px_2()
            .bg(rgb(palette.surface));
        for (action, label, keycode) in [
            ("back", t!("android.back"), 4),
            ("home", t!("android.home"), 3),
            ("recent", t!("android.recent"), 187),
        ] {
            let target = id.clone();
            toolbar = toolbar.child(small_button(
                palette,
                format!("android-{action}-{id}"),
                label,
                cx.listener(move |app, _, _, cx| {
                    cx.stop_propagation();
                    app.android_key(&target, keycode, 0, 0, cx);
                    app.android_key(&target, keycode, 1, 0, cx);
                }),
            ));
        }
        let rotate = id.clone();
        let paste = id.clone();
        let reconnect = id.clone();
        let fit = id.clone();
        let zoom_in = id.clone();
        let zoom_out = id.clone();
        toolbar = toolbar
            .child(small_button(
                palette,
                format!("android-rotate-{id}"),
                t!("android.rotate"),
                cx.listener(move |app, _, _, cx| {
                    cx.stop_propagation();
                    app.android_send(&rotate, Control::Rotate, cx);
                }),
            ))
            .child(small_button(
                palette,
                format!("android-fit-{id}"),
                t!("android.fit"),
                cx.listener(move |app, _, _, cx| {
                    cx.stop_propagation();
                    if let Some(screen) = app.android.screens.get_mut(&fit) {
                        screen.viewport = Default::default();
                        screen.bounds = None;
                        screen.layout_epoch += 1;
                    }
                    cx.notify();
                }),
            ))
            .child(small_button(
                palette,
                format!("android-zoom-{id}"),
                t!("android.zoom"),
                cx.listener(move |app, _, _, cx| {
                    cx.stop_propagation();
                    app.android_zoom(&zoom_in, 1.25, None, cx);
                    cx.notify();
                }),
            ))
            .child(small_button(
                palette,
                format!("android-zoom-out-{id}"),
                t!("android.zoomOut"),
                cx.listener(move |app, _, _, cx| {
                    cx.stop_propagation();
                    app.android_zoom(&zoom_out, 0.8, None, cx);
                }),
            ))
            .child(small_button(
                palette,
                format!("android-paste-{id}"),
                t!("android.paste"),
                cx.listener(move |app, _, _, cx| {
                    cx.stop_propagation();
                    app.android_paste(&paste, cx);
                }),
            ))
            .child(small_button(
                palette,
                format!("android-reconnect-{id}"),
                t!("android.reconnect"),
                cx.listener(move |app, _, window, cx| {
                    cx.stop_propagation();
                    app.reconnect_session(reconnect.clone(), window, cx);
                }),
            ));
        toolbar = toolbar.child(div().flex_1()).child(
            nyaterm_ui::NyaIconButton::new(
                format!("android-input-help-{id}"),
                "icons/menu/info.svg",
            )
            .size(px(28.))
            .icon_size(px(18.))
            .tooltip(t!("android.inputHint")),
        );
        let app = cx.weak_entity();
        let bounds_id = id.clone();
        let image = canvas(
            move |bounds, window, cx| {
                let (x, y, width, height) = viewport
                    .layout(
                        (f32::from(bounds.size.width), f32::from(bounds.size.height)),
                        dimensions,
                    )
                    .unwrap_or_default();
                let image = Bounds::new(
                    Point::new(bounds.origin.x + px(x), bounds.origin.y + px(y)),
                    Size::new(px(width), px(height)),
                );
                let app = app.clone();
                let bounds_id = bounds_id.clone();
                window.defer(cx, move |_, cx| {
                    let _ = app.update(cx, |app, _| {
                        if let Some(screen) = app.android.screens.get_mut(&bounds_id)
                            && screen.generation == geometry_generation
                            && screen.layout_epoch == layout_epoch
                        {
                            screen.viewport_bounds = Some(bounds);
                            screen.bounds = Some(image);
                            screen.bounds_generation = geometry_generation;
                        }
                    });
                });
                image
            },
            move |_, image, window, _| {
                if let Some(texture) = texture {
                    let _ = window.paint_dynamic_texture(image, texture);
                }
            },
        )
        .size_full();
        let down = id.clone();
        let up = id.clone();
        let movement = id.clone();
        let scroll = id.clone();
        let keyboard = id.clone();
        let key_up = id.clone();
        let outside_up = id.clone();
        let pan_down = id.clone();
        let pan_up = id.clone();
        let pan_out = id.clone();
        let focus = screen.focus.clone();
        let surface = div()
            .id(format!("android-input-{id}"))
            .flex_1()
            .min_h_0()
            .min_w_0()
            .overflow_hidden()
            .bg(rgb(0))
            .track_focus(&focus)
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(move |app, event: &gpui::MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    app.activate_workspace_pane(pan_down.clone(), cx);
                    app.focus_android_screen(&pan_down, window, cx);
                    app.android.release_inputs(&pan_down);
                    if let Some(screen) = app.android.screens.get_mut(&pan_down) {
                        screen.pan_drag = Some(event.position);
                    }
                }),
            )
            .on_mouse_up(
                MouseButton::Middle,
                cx.listener(move |app, _: &gpui::MouseUpEvent, _, cx| {
                    cx.stop_propagation();
                    if let Some(screen) = app.android.screens.get_mut(&pan_up) {
                        screen.pan_drag = None;
                    }
                }),
            )
            .on_mouse_up_out(
                MouseButton::Middle,
                cx.listener(move |app, _: &gpui::MouseUpEvent, _, _| {
                    if let Some(screen) = app.android.screens.get_mut(&pan_out) {
                        screen.pan_drag = None;
                    }
                }),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |app, event: &gpui::MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    app.activate_workspace_pane(down.clone(), cx);
                    app.focus_android_screen(&down, window, cx);
                    app.android_pointer(&down, event.position, 0, cx);
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |app, event: &gpui::MouseUpEvent, _, cx| {
                    cx.stop_propagation();
                    app.android_pointer(&up, event.position, 1, cx);
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(move |app, _: &gpui::MouseUpEvent, _, _| {
                    app.android.release_inputs(&outside_up);
                }),
            )
            .on_mouse_move(
                cx.listener(move |app, event: &gpui::MouseMoveEvent, _, cx| {
                    if let Some(screen) = app.android.screens.get_mut(&movement)
                        && let Some(previous) = screen.pan_drag
                        && let Some(bounds) = screen.viewport_bounds
                    {
                        screen.viewport.pan_by(
                            (
                                f32::from(event.position.x - previous.x),
                                f32::from(event.position.y - previous.y),
                            ),
                            (f32::from(bounds.size.width), f32::from(bounds.size.height)),
                            screen.dimensions,
                        );
                        screen.pan_drag = Some(event.position);
                        screen.bounds = None;
                        screen.layout_epoch += 1;
                        cx.stop_propagation();
                        cx.notify();
                        return;
                    }
                    if app.android.screens.get(&movement).is_some_and(|s| s.drag) {
                        app.android_pointer(&movement, event.position, 2, cx);
                    }
                }),
            )
            .on_scroll(cx.listener(move |app, event: &gpui::ScrollEvent, _, cx| {
                cx.stop_propagation();
                if event.modifiers.control || event.modifiers.platform {
                    let delta = event.delta.pixel_delta(px(20.));
                    app.android_zoom(
                        &scroll,
                        (f32::from(delta.y) / 240.0).exp(),
                        Some(event.position),
                        cx,
                    );
                    return;
                }
                if let Some((x, y, width, height, generation)) =
                    app.android_position(&scroll, event.position)
                {
                    let delta = event.delta.pixel_delta(px(20.));
                    app.android_send(
                        &scroll,
                        Control::Scroll {
                            x,
                            y,
                            width,
                            height,
                            generation,
                            horizontal: f32::from(delta.x) / 40.0,
                            vertical: f32::from(delta.y) / 40.0,
                        },
                        cx,
                    );
                }
            }))
            .on_key_down(cx.listener(move |app, event: &KeyDownEvent, _, cx| {
                cx.stop_propagation();
                let mods = event.keystroke.modifiers;
                if event.keystroke.key.eq_ignore_ascii_case("v")
                    && app
                        .android
                        .screens
                        .get(&keyboard)
                        .is_some_and(|screen| screen.paste_shortcut)
                {
                    return;
                }
                if event.keystroke.key.eq_ignore_ascii_case("v")
                    && (mods.control || mods.platform)
                    && !mods.alt
                {
                    if let Some(screen) = app.android.screens.get_mut(&keyboard) {
                        screen.paste_shortcut = true;
                    }
                    if !event.is_held {
                        app.android_paste(&keyboard, cx);
                    }
                    return;
                }
                let metastate = u32::from(mods.shift)
                    | (u32::from(mods.alt) << 1)
                    | (u32::from(mods.control) << 12)
                    | (u32::from(mods.platform) << 16);
                if !mods.control
                    && !mods.alt
                    && !mods.platform
                    && let Some(text) = event
                        .keystroke
                        .key_char
                        .as_ref()
                        .filter(|s| s.is_ascii() && !s.chars().any(char::is_control))
                {
                    app.android_send(&keyboard, Control::Text { text: text.clone() }, cx);
                } else if let Some(code) =
                    nyaterm_core::models::android::android_keycode(&event.keystroke.key)
                {
                    if let Some(screen) = app.android.screens.get_mut(&keyboard) {
                        screen.physical_keys.insert(code);
                    }
                    app.android_key(&keyboard, code, 0, metastate, cx);
                }
            }))
            .on_key_up(cx.listener(move |app, event: &gpui::KeyUpEvent, _, cx| {
                cx.stop_propagation();
                if event.keystroke.key.eq_ignore_ascii_case("v")
                    && let Some(screen) = app.android.screens.get_mut(&key_up)
                    && std::mem::take(&mut screen.paste_shortcut)
                {
                    return;
                }
                if let Some(code) =
                    nyaterm_core::models::android::android_keycode(&event.keystroke.key)
                    && app
                        .android
                        .screens
                        .get_mut(&key_up)
                        .is_some_and(|screen| screen.physical_keys.remove(&code))
                {
                    app.android_key(&key_up, code, 1, 0, cx);
                }
            }))
            .child(image);
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(toolbar)
            .when_some(error, |el, error| {
                el.child(div().px_2().text_color(rgb(palette.danger)).child(error))
            })
            .child(surface)
            .into_any_element()
    }
    fn android_paste(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(text) = cx
            .read_from_clipboard()
            .and_then(|item| item.text())
            .filter(|text| !text.is_empty())
        else {
            return;
        };
        let keys: Vec<_> = self
            .android
            .screens
            .get_mut(id)
            .map(|screen| {
                [113, 114, 117, 118]
                    .into_iter()
                    .filter(|key| screen.physical_keys.remove(key))
                    .collect()
            })
            .unwrap_or_default();
        for key in keys {
            self.android_key(id, key, 1, 0, cx);
        }
        self.android_send(
            id,
            Control::Paste {
                text,
                sequence: nyaterm_core::models::android::next_paste_sequence(),
            },
            cx,
        );
    }
    fn android_zoom(
        &mut self,
        id: &str,
        factor: f32,
        anchor: Option<Point<Pixels>>,
        cx: &mut Context<Self>,
    ) {
        self.android.release_inputs(id);
        if let Some(screen) = self.android.screens.get_mut(id)
            && let Some(bounds) = screen.viewport_bounds
        {
            let size = (f32::from(bounds.size.width), f32::from(bounds.size.height));
            let point = anchor
                .map(|p| {
                    (
                        f32::from(p.x - bounds.origin.x),
                        f32::from(p.y - bounds.origin.y),
                    )
                })
                .unwrap_or((size.0 / 2.0, size.1 / 2.0));
            screen
                .viewport
                .zoom_at(factor, point, size, screen.dimensions);
            screen.bounds = None;
            screen.layout_epoch += 1;
        }
        cx.notify();
    }
    fn android_send(&mut self, id: &str, control: Control, cx: &mut Context<Self>) {
        let result = control
            .validate_input()
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))
            .and_then(|()| {
                self.android
                    .screens
                    .get(id)
                    .and_then(|s| s.handle.as_ref())
                    .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotConnected))?
                    .send(control)
            });
        if let Err(error) = result {
            let rejection = error
                .get_ref()
                .and_then(|error| error.downcast_ref::<nyaterm_android::ipc::InputRejection>())
                .copied();
            self.android_input_notice(
                id,
                error.kind() == std::io::ErrorKind::WouldBlock,
                rejection,
                cx,
            );
        }
    }
    fn android_input_notice(
        &mut self,
        id: &str,
        queue_full: bool,
        rejection: Option<nyaterm_android::ipc::InputRejection>,
        cx: &mut Context<Self>,
    ) {
        if let Some(screen) = self.android.screens.get_mut(id) {
            if queue_full {
                screen.drag = false;
                screen.physical_keys.clear();
            }
            if screen
                .last_input_notice
                .is_some_and(|last| last.elapsed() < Duration::from_secs(2))
            {
                return;
            }
            screen.last_input_notice = Some(Instant::now());
        }
        let message = if queue_full {
            t!("android.errors.inputQueueFull").into_owned()
        } else {
            rejection
                .map(super::labels::input_rejection_message)
                .unwrap_or_else(|| t!("android.errors.inputFailed").into_owned())
        };
        if queue_full {
            self.shell.set_status(message.clone());
            self.notify_background_operation(
                "android-input",
                nyaterm_ui::notification::NyaNotificationKind::Warning,
                message,
                cx,
            );
        } else {
            self.report_android_failure(
                "screen-input",
                &nyaterm_core::uuid(),
                rejection.map_or("InputRejected", |code| code.code()),
                message,
                cx,
            );
        }
    }
    fn android_key(
        &mut self,
        id: &str,
        code: u32,
        action: u8,
        metastate: u32,
        cx: &mut Context<Self>,
    ) {
        self.android_send(
            id,
            Control::Key {
                action,
                keycode: code,
                repeat: 0,
                metastate,
            },
            cx,
        );
    }
    fn android_position(
        &self,
        id: &str,
        position: Point<Pixels>,
    ) -> Option<(u32, u32, u32, u32, u64)> {
        let screen = self.android.screens.get(id)?;
        if screen.pan_drag.is_some() {
            return None;
        }
        if screen.bounds_generation != screen.generation {
            return None;
        }
        let bounds = screen.bounds?;
        if !screen.viewport_bounds?.contains(&position) {
            return None;
        }
        let (width, height) = screen.dimensions;
        let (x, y) = screen_point(
            f32::from(position.x - bounds.origin.x),
            f32::from(position.y - bounds.origin.y),
            f32::from(bounds.size.width),
            f32::from(bounds.size.height),
            width,
            height,
        )?;
        Some((x, y, width, height, screen.generation))
    }
    fn android_pointer(
        &mut self,
        id: &str,
        position: Point<Pixels>,
        action: u8,
        cx: &mut Context<Self>,
    ) {
        if let Some((x, y, width, height, generation)) = self.android_position(id, position) {
            if let Some(screen) = self.android.screens.get_mut(id) {
                screen.drag = action != 1;
            }
            self.android_send(
                id,
                Control::Touch {
                    action,
                    x,
                    y,
                    width,
                    height,
                    generation,
                },
                cx,
            );
        } else if action == 1 {
            self.android_send(id, Control::ReleaseInputs, cx);
            if let Some(screen) = self.android.screens.get_mut(id) {
                screen.drag = false;
            }
        }
    }
}
