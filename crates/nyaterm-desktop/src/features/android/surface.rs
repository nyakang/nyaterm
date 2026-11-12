use crate::features::NyaTermApp;
use crate::models::event_wake::{ANY_INTEREST, EventWake};
use futures::StreamExt as _;
use gpui::{Context, IntoElement, Render, Subscription, Task, WeakEntity, Window, div};
use std::sync::Arc;

pub(super) struct AndroidSurface {
    app: WeakEntity<NyaTermApp>,
    id: String,
    wake: EventWake,
    bound_attempt: Option<u64>,
    presentation_scheduled: bool,
    _observe: Option<Subscription>,
    _events: Task<()>,
}
impl AndroidSurface {
    pub(super) fn new(app: WeakEntity<NyaTermApp>, id: String, cx: &mut Context<Self>) -> Self {
        let observe = app
            .upgrade()
            .map(|app| cx.observe(&app, |_, _, cx| cx.notify()));
        let (wake, mut rx) = EventWake::new();
        let event_wake = wake.clone();
        let event_app = app.clone();
        let event_id = id.clone();
        let events = cx.spawn(async move |this, cx| {
            loop {
                event_wake.arm(ANY_INTEREST);
                let changed = event_app
                    .update(cx, |app, cx| app.poll_android_screen(&event_id, cx))
                    .unwrap_or(false);
                if changed && this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
                if rx.next().await.is_none() {
                    break;
                }
            }
        });
        Self {
            app,
            id,
            wake,
            bound_attempt: None,
            presentation_scheduled: false,
            _observe: observe,
            _events: events,
        }
    }
}
impl Render for AndroidSurface {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let wake = self.wake.clone();
        let has_frame = self
            .app
            .update(cx, |app, _| {
                if let Some(screen) = app.android.screens.get(&self.id) {
                    if self.bound_attempt != Some(screen.attempt)
                        && let Some(handle) = &screen.handle
                    {
                        handle.set_waker(Arc::new(move || {
                            wake.signal(ANY_INTEREST);
                        }));
                        self.bound_attempt = Some(screen.attempt);
                    }
                    screen.visible && screen.frame.is_some()
                } else {
                    false
                }
            })
            .unwrap_or(false);
        if has_frame && !self.presentation_scheduled {
            self.presentation_scheduled = true;
            cx.on_next_frame(window, |surface, window, cx| {
                let _ = surface.app.update(cx, |app, _| {
                    app.prepare_android_texture(&surface.id, window)
                });
                surface.presentation_scheduled = false;
                cx.notify();
            });
        }
        self.app
            .update(cx, |app, cx| {
                app.android_screen_content(self.id.clone(), cx)
            })
            .unwrap_or_else(|_| div().into_any_element())
    }
}
