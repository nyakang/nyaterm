use crate::features::NyaTermApp;
use crate::features::plugins::process::PluginProcess;
use crate::features::plugins::view::PluginPanel;
use futures::StreamExt;
use gpui::{AppContext, Context, Entity, WeakEntity};

pub(in crate::features) struct PluginFeatureState {
    _process_subscription: gpui::Subscription,
    pub(in crate::features) details: std::collections::BTreeMap<
        nyaterm_core::plugins::management::PluginKey,
        Entity<PluginPanel>,
    >,
    host_task: Option<gpui::Task<()>>,
    // Keep the editing state alive when the side panel is hidden or moved.
    pub panel: Entity<PluginPanel>,
    #[cfg(test)]
    pub native_launch_fixture: Option<nyaterm_core::plugins::native::NativeLaunch>,
}

impl PluginFeatureState {
    pub fn new(
        app: WeakEntity<NyaTermApp>,
        process: Entity<PluginProcess>,
        cx: &mut Context<NyaTermApp>,
    ) -> Self {
        let host_task = process.read(cx).service().map(|service| {
            let mut requests = service.desktop.register();
            let app = app.clone();
            cx.spawn(async move |_, cx| {
                while let Some(message) = requests.next().await {
                    if app
                        .update(cx, |app, cx| app.dispatch_extension_host(message, cx))
                        .is_err()
                    {
                        break;
                    }
                }
            })
        });
        let subscription = cx.observe(&process, |_, _, cx| cx.notify());
        let panel = cx.new(|cx| PluginPanel::new(app, process, cx));
        Self {
            host_task,
            details: Default::default(),
            _process_subscription: subscription,
            panel,
            #[cfg(test)]
            native_launch_fixture: None,
        }
    }

    pub fn shutdown(&mut self, cx: &mut Context<NyaTermApp>) {
        self.host_task.take();
        for detail in self.details.values() {
            detail.update(cx, |detail, cx| detail.cancel(cx));
        }
        self.details.clear();
        self.panel.update(cx, |panel, cx| panel.cancel(cx));
    }
}
