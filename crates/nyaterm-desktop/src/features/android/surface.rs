use crate::features::NyaTermApp;
use gpui::{Context, IntoElement, Render, Subscription, Task, WeakEntity, Window, div};
use std::time::Duration;

pub(super) struct AndroidSurface {
    app: WeakEntity<NyaTermApp>,
    id: String,
    _observe: Option<Subscription>,
    _clock: Task<()>,
}
impl AndroidSurface {
    pub(super) fn new(app: WeakEntity<NyaTermApp>, id: String, cx: &mut Context<Self>) -> Self {
        let observe = app
            .upgrade()
            .map(|app| cx.observe(&app, |_, _, cx| cx.notify()));
        let clock_app = app.clone();
        let clock_id = id.clone();
        let clock = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                let changed = clock_app
                    .update(cx, |app, _| app.poll_android_screen(&clock_id))
                    .unwrap_or(false);
                if this
                    .update(cx, |_, cx| {
                        if changed {
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
            app,
            id,
            _observe: observe,
            _clock: clock,
        }
    }
}
impl Render for AndroidSurface {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.app
            .update(cx, |app, cx| {
                app.prepare_android_texture(&self.id, window);
                app.android_screen_content(self.id.clone(), cx)
            })
            .unwrap_or_else(|_| div().into_any_element())
    }
}
