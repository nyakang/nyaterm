use crate::features::NyaTermApp;
use gpui::Context;
use nyaterm_core::plugins::invocation::ActionInput;
use nyaterm_plugin_host::service::{OperationReply, PluginOperation};
use nyaterm_ui::NyaMenuItem;

impl NyaTermApp {
    pub(in crate::features) fn extension_context_menu(
        &self,
        session_id: &str,
        selection: &str,
        file_menu: bool,
        cx: &mut Context<Self>,
    ) -> Vec<NyaMenuItem> {
        let process = self.plugins.panel.read(cx).process.clone();
        let snapshot = &process.read(cx).snapshot;
        let context = serde_json::json!({
            "session": { "id": session_id, "generation": 0,
                "connection_id": self.session.metadata(session_id).and_then(|metadata| metadata.source_connection_id.clone()) },
            "selection": selection,
        }).to_string();
        snapshot
            .contributions
            .iter()
            .filter(|contribution| {
                snapshot
                    .plugins
                    .iter()
                    .find(|plugin| plugin.id == contribution.plugin_id)
                    .and_then(|plugin| plugin.manifest.as_ref())
                    .is_some_and(|manifest| {
                        let declared = if file_menu {
                            &manifest.contributions.file_menu
                        } else {
                            &manifest.contributions.terminal_menu
                        };
                        declared.contains(&contribution.action.id)
                    })
            })
            .map(|contribution| {
                let contribution = contribution.clone();
                let context = context.clone();
                NyaMenuItem::action(contribution.action.name.clone()).on_click(cx.listener(
                    move |this, _, _, cx| {
                        this.open_panel(crate::models::NavItem::Plugins, cx);
                        this.plugins.panel.update(cx, |panel, cx| {
                            panel.run_contribution(contribution.clone(), Some(context.clone()), cx)
                        });
                    },
                ))
            })
            .collect()
    }

    pub(in crate::features) fn extension_session_event(
        &self,
        id: &str,
        connected: bool,
        cx: &mut Context<Self>,
    ) {
        let process = self.plugins.panel.read(cx).process.clone();
        let Some(service) = process.read(cx).service() else {
            return;
        };
        if !service.record_session_event(id, connected) {
            return;
        }
        let snapshot = service.snapshot();
        let text = serde_json::json!({ "session": { "id": id, "generation": 0,
            "connection_id": self.session.metadata(id).and_then(|m| m.source_connection_id.clone()) },
            "event": if connected { "connected" } else { "disconnected" } }).to_string();
        for contribution in &snapshot.contributions {
            let subscribed = snapshot
                .plugins
                .iter()
                .find(|plugin| {
                    plugin.id == contribution.plugin_id && plugin.session_events_authorized
                })
                .and_then(|plugin| plugin.manifest.as_ref())
                .is_some_and(|manifest| {
                    let actions = if connected {
                        &manifest.contributions.session_connected
                    } else {
                        &manifest.contributions.session_disconnected
                    };
                    actions.contains(&contribution.action.id)
                });
            if !subscribed {
                continue;
            }
            if let Ok(task) = service.submit(PluginOperation::Invoke {
                contribution_id: contribution.id.clone(),
                expected_revision: contribution.revision,
                input: ActionInput {
                    text: Some(text.clone()),
                    ..Default::default()
                },
            }) {
                cx.spawn(async move |_, _| {
                    if let Ok(Ok(OperationReply::Invocation(invocation))) = task.await {
                        let _ = invocation.result().await;
                    }
                })
                .detach();
            }
        }
    }
}
