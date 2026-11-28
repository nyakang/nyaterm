mod details;
mod list;

use crate::features::NyaTermApp;
use crate::features::plugins::process::PluginProcess;
use crate::theme::ThemePalette;
use gpui::{
    AppContext, Context, Div, Entity, FontWeight, InteractiveElement, IntoElement, ParentElement,
    Render, Styled, Subscription, Task, WeakEntity, Window, div, prelude::FluentBuilder as _, px,
    rgb, svg,
};
use nyaterm_core::command_draft::DraftOrigin;
use nyaterm_core::plugins::invocation::{ActionInput, ActionResult, parse_parameter};
use nyaterm_core::plugins::manifest::{ParameterKind, ResultKind};
use nyaterm_core::plugins::{ErrorCode, MAX_TEXT_BYTES, PluginError};
use nyaterm_plugin_host::manager::{
    CatalogSnapshot, Contribution, InvocationLease, PluginSnapshot,
};
use nyaterm_plugin_host::service::{OperationReply, PluginOperation};
use nyaterm_ui::notification::{NyaNotificationKind, NyaNotificationWindowExt as _};
use nyaterm_ui::presentation::progress_bar;
use nyaterm_ui::{
    NyaButton, NyaButtonVariant, NyaInput, NyaInputEvent, NyaInputState, NyaScrollable,
};
use rust_i18n::t;

struct ParameterInput {
    id: String,
    name: String,
    kind: ParameterKind,
    required: bool,
    input: Entity<NyaInputState>,
}
struct Preview {
    plugin_id: String,
    contribution_id: String,
    revision: u64,
    kind: ResultKind,
    title: String,
    lease: InvocationLease,
}

struct PendingOperation {
    task: Task<()>,
}

pub(in crate::features) struct PluginPanel {
    app: WeakEntity<NyaTermApp>,
    pub(super) detail_key: Option<nyaterm_core::plugins::management::PluginKey>,
    pub(super) detail_tab: usize,
    pub(in crate::features) focus: gpui::FocusHandle,
    pub(super) process: Entity<PluginProcess>,
    search: Entity<NyaInputState>,
    pub(super) discover: bool,
    pub(super) tool_panels:
        std::collections::BTreeMap<(String, String), super::tool_panel::ToolPanel>,
    developer_tools: bool,
    permission_scope: Entity<NyaInputState>,
    selected_permission: Option<(String, String, u64)>,
    text: Entity<NyaInputState>,
    output: Entity<NyaInputState>,
    parameters: Vec<ParameterInput>,
    selected: Option<Contribution>,
    preview: Option<Preview>,
    pending: Option<PendingOperation>,
    generation: u64,
    #[cfg(test)]
    completion_gate: Option<futures::channel::oneshot::Receiver<()>>,
    #[cfg(test)]
    error_notifications: Vec<String>,
    _subscription: Subscription,
    _search_subscription: Subscription,
}

impl PluginPanel {
    pub(in crate::features) fn catalog(
        &self,
        cx: &gpui::App,
    ) -> nyaterm_plugin_host::manager::CatalogSnapshot {
        self.process.read(cx).snapshot.clone()
    }
    pub(in crate::features) fn run_contribution(
        &mut self,
        contribution: Contribution,
        text: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if self.detail_key.is_none() {
            let app = self.app.clone();
            cx.defer(move |cx| {
                let _ = app.update(cx, |app, cx| {
                    let detail = app.open_plugin_details(
                        nyaterm_core::plugins::management::PluginKey::Wasm(
                            contribution.plugin_id.clone(),
                        ),
                        3,
                        cx,
                    );
                    detail.update(cx, |detail, cx| {
                        detail.run_contribution(contribution, text, cx)
                    });
                });
            });
            return;
        }
        self.detail_tab = 3;
        if contribution
            .action
            .parameters
            .iter()
            .any(|parameter| parameter.required && parameter.default.is_none())
        {
            self.select(contribution, cx);
            if let Some(text) = text {
                self.text
                    .update(cx, |state, cx| state.set_content(&text, cx));
            }
        } else {
            self.operation(
                PluginOperation::Invoke {
                    contribution_id: contribution.id,
                    expected_revision: contribution.revision,
                    input: ActionInput {
                        text,
                        ..Default::default()
                    },
                },
                cx,
            );
        }
    }
    pub fn new(
        app: WeakEntity<NyaTermApp>,
        process: Entity<PluginProcess>,
        cx: &mut Context<Self>,
    ) -> Self {
        let subscription = cx.observe(&process, |this, _, cx| {
            let snapshot = &this.process.read(cx).snapshot;
            this.tool_panels.retain(|(owner, _), panel| {
                snapshot.plugins.iter().any(|plugin| {
                    &plugin.id == owner && plugin.enabled && plugin.revision == panel.revision
                })
            });
            let snapshot = snapshot.clone();
            if this
                .selected_permission
                .as_ref()
                .is_some_and(|(id, _, revision)| {
                    !snapshot
                        .plugins
                        .iter()
                        .any(|plugin| &plugin.id == id && &plugin.revision == revision)
                })
            {
                this.selected_permission = None;
                this.permission_scope
                    .update(cx, |state, cx| state.clear(cx));
            }
            if this.clear_stale_selection(&snapshot, cx) {
                if this.can_cancel() {
                    this.pending.take();
                    this.generation += 1;
                }
                if !this.busy(cx) {
                    this.notify_error(t!("plugins.actionChanged"), cx);
                }
            }
            cx.notify();
        });
        let search = cx.new(|cx| NyaInputState::new(cx, "").placeholder(t!("plugins.search")));
        let search_subscription = cx.subscribe(&search, |_, _, event, cx| {
            if matches!(event, NyaInputEvent::Changed(_)) {
                cx.notify();
            }
        });
        Self {
            app,
            detail_key: None,
            detail_tab: 0,
            focus: cx.focus_handle(),
            process,
            discover: true,
            tool_panels: std::collections::BTreeMap::new(),
            developer_tools: false,
            selected_permission: None,
            permission_scope: cx
                .new(|cx| NyaInputState::new(cx, "").placeholder(t!("plugins.permissionScope"))),
            search,

            text: cx.new(|cx| {
                NyaInputState::new(cx, "")
                    .multi_line(Some(6))
                    .max_chars(Some(MAX_TEXT_BYTES))
            }),
            output: cx.new(|cx| {
                NyaInputState::new(cx, "")
                    .multi_line(Some(6))
                    .max_chars(Some(MAX_TEXT_BYTES))
            }),
            parameters: Vec::new(),
            selected: None,
            preview: None,
            pending: None,
            generation: 0,
            #[cfg(test)]
            completion_gate: None,
            #[cfg(test)]
            error_notifications: Vec::new(),
            _subscription: subscription,
            _search_subscription: search_subscription,
        }
    }

    fn notify_error(&mut self, message: impl Into<String>, cx: &mut Context<Self>) {
        let message = message.into();
        #[cfg(test)]
        self.error_notifications.push(message.clone());
        let target = cx.active_window();
        let app = self.app.clone();
        // The panel may be updated from NyaTermApp; release both entity leases
        // before routing feedback through the application's notification host.
        cx.defer(move |cx| {
            let _ = app.update(cx, |app, cx| {
                app.notify_operation_at(
                    target.or_else(|| app.shell.main_window().map(Into::into)),
                    "plugin-operation-error",
                    NyaNotificationKind::Error,
                    message,
                    cx,
                );
            });
        });
    }

    fn clear_stale_selection(
        &mut self,
        snapshot: &CatalogSnapshot,
        cx: &mut Context<Self>,
    ) -> bool {
        let stale = self.selected.as_ref().is_some_and(|selected| {
            !snapshot
                .contributions
                .iter()
                .any(|c| c.id == selected.id && c.revision == selected.revision)
        });
        if stale {
            self.selected = None;
            self.preview = None;
            self.parameters.clear();
            self.output.update(cx, |state, cx| state.clear(cx));
        }
        stale
    }

    fn close_workbench(&mut self, cx: &mut Context<Self>) {
        if self.can_cancel() {
            self.pending.take();
            self.generation += 1;
        } else if self.busy(cx) {
            return;
        }
        self.selected = None;
        self.preview = None;
        self.parameters.clear();
        self.output.update(cx, |state, cx| state.clear(cx));
        cx.notify();
    }

    fn select(&mut self, contribution: Contribution, cx: &mut Context<Self>) {
        if self.busy(cx) {
            return;
        }
        self.generation += 1;
        self.preview = None;
        self.output.update(cx, |state, cx| state.clear(cx));
        self.parameters = contribution
            .action
            .parameters
            .iter()
            .map(|parameter| ParameterInput {
                id: parameter.id.clone(),
                name: parameter.name.clone(),
                kind: parameter.kind,
                required: parameter.required,
                input: cx.new(|cx| {
                    NyaInputState::new(cx, parameter.default.clone().unwrap_or_default())
                        .max_chars(Some(MAX_TEXT_BYTES))
                }),
            })
            .collect();
        self.selected = Some(contribution);
        cx.notify();
    }

    fn operation(&mut self, operation: PluginOperation, cx: &mut Context<Self>) {
        if !matches!(operation, PluginOperation::Invoke { .. }) {
            self.process.update(cx, |process, cx| {
                process.manage(super::management::ManagementAction::Wasm(operation), cx)
            });
            return;
        }
        if self.busy(cx) {
            return;
        }
        let Some(service) = self.process.read(cx).service.clone() else {
            self.notify_error(t!("plugins.unavailable"), cx);
            cx.notify();
            return;
        };
        let task = match service.submit(operation) {
            Ok(task) => task,
            Err(error) => {
                self.notify_error(error.message, cx);
                cx.notify();
                return;
            }
        };
        self.generation += 1;
        let generation = self.generation;
        #[cfg(test)]
        let completion_gate = self.completion_gate.take();
        let task = cx.spawn(async move |this, cx| {
            let result = match task.await {
                Ok(Ok(OperationReply::Changed)) => Ok(None),
                Ok(Ok(OperationReply::Invocation(invocation))) => {
                    let source = (
                        invocation.plugin_id.clone(),
                        invocation.contribution_id.clone(),
                        invocation.revision,
                        invocation.lease(),
                    );
                    invocation
                        .result()
                        .await
                        .map(|result| Some((source, result)))
                }
                Ok(Err(error)) => Err(error),
                Err(_) => Err(PluginError::new(
                    ErrorCode::Shutdown,
                    "Plugin operation was interrupted",
                )),
            };
            #[cfg(test)]
            if let Some(gate) = completion_gate {
                let _ = gate.await;
            }
            let _ = this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                if let Some(pending) = this.pending.take() {
                    pending.task.detach();
                }
                match result {
                    Ok(Some(((plugin_id, contribution_id, revision, lease), result))) => {
                        let current = service.snapshot();
                        if !lease.is_current()
                            || !current
                                .contributions
                                .iter()
                                .any(|c| c.id == contribution_id && c.revision == revision)
                        {
                            this.notify_error(t!("plugins.actionChanged"), cx);
                            cx.notify();
                            return;
                        }
                        let (kind, title, output) = match result {
                            ActionResult::Command { title, command } => {
                                (ResultKind::Command, title, command)
                            }
                            ActionResult::Text(text) if text.is_empty() => {
                                this.preview = None;
                                cx.notify();
                                return;
                            }
                            ActionResult::Text(text) => {
                                (ResultKind::Text, t!("plugins.textResult").to_string(), text)
                            }
                        };
                        this.output
                            .update(cx, |state, cx| state.set_content(&output, cx));
                        this.preview = Some(Preview {
                            plugin_id,
                            contribution_id,
                            revision,
                            kind,
                            title,
                            lease,
                        });
                    }
                    Ok(None) => {}
                    Err(error) => {
                        this.preview = None;
                        this.notify_error(error.message, cx);
                    }
                }
                cx.notify();
            });
        });
        self.pending = Some(PendingOperation { task });
        cx.notify();
    }

    fn invoke(&mut self, cx: &mut Context<Self>) {
        if self.busy(cx) {
            return;
        }
        let Some(selected) = self.selected.clone() else {
            return;
        };
        let mut input = ActionInput::default();
        for parameter in &self.parameters {
            let value = parameter.input.read(cx).value(cx);
            if value.is_empty() && !parameter.required {
                continue;
            }
            match parse_parameter(parameter.kind, &value) {
                Ok(value) => {
                    input.parameters.insert(parameter.id.clone(), value);
                }
                Err(error) => {
                    self.notify_error(error.message, cx);
                    cx.notify();
                    return;
                }
            }
        }
        if selected.action.result == ResultKind::Text {
            input.text = Some(self.text.read(cx).value(cx));
        }
        self.preview = None;
        self.operation(
            PluginOperation::Invoke {
                contribution_id: selected.id,
                expected_revision: selected.revision,
                input,
            },
            cx,
        );
    }

    pub(super) fn cancel(&mut self, cx: &mut Context<Self>) {
        if !self.can_cancel() {
            return;
        }
        self.pending.take();
        self.generation += 1;
        cx.notify();
    }

    fn busy(&self, cx: &gpui::App) -> bool {
        let process = self.process.read(cx);
        self.pending.is_some()
            || process.job_busy("\0global")
            || self
                .detail_key
                .as_ref()
                .is_some_and(|key| process.job_busy(&key.id()))
    }

    fn can_cancel(&self) -> bool {
        self.pending.is_some()
    }

    fn fill(&mut self, replace: bool, cx: &mut Context<Self>) {
        let Some(preview) = &self.preview else {
            return;
        };
        let Some(service) = &self.process.read(cx).service else {
            return;
        };
        if !preview.lease.is_current()
            || preview.kind != ResultKind::Command
            || !service
                .snapshot()
                .contributions
                .iter()
                .any(|c| c.id == preview.contribution_id && c.revision == preview.revision)
        {
            self.notify_error(t!("plugins.actionChanged"), cx);
            cx.notify();
            return;
        }
        let output = self.output.read(cx).value(cx);
        let origin = DraftOrigin::Plugin {
            plugin_id: preview.plugin_id.clone(),
            action_id: preview
                .contribution_id
                .split_once(':')
                .unwrap()
                .1
                .to_owned(),
            revision: preview.revision,
        };
        match self.app.update(cx, |app, cx| {
            app.fill_plugin_draft(app.send_command.active_id(), &output, replace, origin, cx)
        }) {
            Ok(Ok(())) => {}
            Ok(Err(error)) => self.notify_error(error.message, cx),
            Err(_) => self.notify_error(t!("plugins.unavailable"), cx),
        }
        cx.notify();
    }

    fn native_operation(
        &mut self,
        id: nyaterm_core::plugins::native::NativeComponentId,
        action: nyaterm_plugin_host::native::NativeAction,
        cx: &mut Context<Self>,
    ) {
        self.process.update(cx, |process, cx| {
            process.manage(super::management::ManagementAction::Native(id, action), cx)
        });
    }

    fn browse_native(
        &mut self,
        id: nyaterm_core::plugins::native::NativeComponentId,
        cx: &mut Context<Self>,
    ) {
        let paths = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t!("plugins.chooseSource").into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = paths.await
                && let Some(source) = paths.first().cloned()
            {
                let _ = this.update(cx, |this, cx| {
                    this.native_operation(
                        id,
                        nyaterm_plugin_host::native::NativeAction::Import(source),
                        cx,
                    );
                });
            }
        })
        .detach();
    }

    fn browse_update(&mut self, id: String, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t!("plugins.chooseSource").into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = paths.await
                && let Some(source) = paths.first().cloned()
            {
                let _ = this.update(cx, |this, cx| {
                    this.operation(PluginOperation::Update { id, source }, cx);
                });
            }
        })
        .detach();
    }

    fn browse(&mut self, directories: bool, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: !directories,
            directories,
            multiple: false,
            prompt: Some(t!("plugins.chooseSource").into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = paths.await
                && let Some(path) = paths.first()
            {
                let text = path.to_string_lossy().into_owned();
                let _ = this.update(cx, |this, cx| {
                    this.discover = false;
                    this.operation(
                        PluginOperation::Install {
                            source: std::path::PathBuf::from(&text),
                            development: directories,
                        },
                        cx,
                    );
                });
            }
        })
        .detach();
    }
}

fn capability_label(capability: &str) -> String {
    let key = format!("plugins.capabilities.{capability}");
    t!(&key).to_string()
}

fn parameter_kind_label(kind: ParameterKind) -> &'static str {
    match kind {
        ParameterKind::String => "text",
        ParameterKind::Integer => "int",
        ParameterKind::Boolean => "bool",
    }
}

fn empty_hint(icon_path: &'static str, text: String, palette: ThemePalette) -> Div {
    div()
        .w_full()
        .flex_none()
        .py_6()
        .px_3()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_2()
        .text_center()
        .child(
            svg()
                .size(px(20.))
                .path(icon_path)
                .text_color(rgb(palette.text_dimmed)),
        )
        .child(
            div()
                .text_xs()
                .line_height(px(18.))
                .text_color(rgb(palette.text_muted))
                .child(text),
        )
}

impl PluginPanel {
    fn render_plugin_card(
        &self,
        plugin: &PluginSnapshot,
        snapshot: &CatalogSnapshot,
        palette: ThemePalette,
        cx: &mut Context<Self>,
    ) -> Div {
        let card_selector = format!("plugin-card-{}", plugin.id);
        let mut card = div()
            .debug_selector(move || card_selector.clone())
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap_3();
        let plugin_contributions: Vec<_> = snapshot
            .contributions
            .iter()
            .filter(|c| c.plugin_id == plugin.id)
            .cloned()
            .collect();
        if self.detail_tab == 3 && !plugin_contributions.is_empty() {
            let mut actions_group = div()
                .flex()
                .flex_col()
                .gap_1()
                .pt_1()
                .border_t_1()
                .border_color(rgb(palette.border))
                .child(
                    div()
                        .text_size(px(11.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(palette.text_muted))
                        .child(t!("plugins.actionsSection")),
                );
            for contribution in plugin_contributions {
                let selected = contribution.clone();
                actions_group = actions_group.child(
                    NyaButton::new(
                        format!("plugins-action-{}", contribution.id),
                        contribution.action.name.clone(),
                    )
                    .icon("icons/play.svg")
                    .small()
                    .full_width()
                    .label_left()
                    .tooltip(contribution.action.description.clone())
                    .disabled(self.busy(cx))
                    .on_click(cx.listener(move |this, _, _, cx| this.select(selected.clone(), cx))),
                );
            }
            card = card.child(actions_group);
        }

        let pending_requests = snapshot.permission_requests.get(&plugin.id);
        let grantable_capabilities: Vec<_> = plugin
            .manifest
            .as_ref()
            .map(|manifest| {
                manifest
                    .capabilities
                    .iter()
                    .filter(|capability| !matches!(capability.as_str(), "storage" | "ui"))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();

        if self.detail_tab == 2
            && (pending_requests.is_some_and(|r| !r.is_empty())
                || !grantable_capabilities.is_empty())
        {
            let mut perm_group = div()
                .flex()
                .flex_col()
                .gap_2()
                .pt_1()
                .border_t_1()
                .border_color(rgb(palette.border))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_1()
                        .child(
                            div()
                                .text_size(px(11.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(palette.text_muted))
                                .child(t!("plugins.permissionsSection")),
                        )
                        .child({
                            let revoke_id = plugin.id.clone();
                            NyaButton::new(format!("revoke-{}", plugin.id), t!("plugins.revoke"))
                                .small()
                                .compact()
                                .variant(NyaButtonVariant::Ghost)
                                .disabled(self.busy(cx))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.operation(
                                        PluginOperation::RevokeAuthorizations {
                                            id: revoke_id.clone(),
                                        },
                                        cx,
                                    )
                                }))
                        }),
                );

            if let Some(requests) = pending_requests {
                for (index, grant) in requests.iter().enumerate() {
                    use nyaterm_core::plugins::extension::Scope;
                    let scope = match &grant.scope {
                        Scope::Plugin => String::new(),
                        Scope::Origin { origin } => origin.clone(),
                        Scope::Directory { path } | Scope::Executable { path } => path.clone(),
                        Scope::Connection { id } => id.clone(),
                    };
                    let capability = serde_json::to_value(grant.capability)
                        .unwrap()
                        .as_str()
                        .unwrap()
                        .to_owned();
                    let id = plugin.id.clone();
                    let expected_revision = plugin.revision;
                    let label = if scope.is_empty() {
                        format!(
                            "{} · {}",
                            t!("plugins.grant"),
                            capability_label(&capability)
                        )
                    } else {
                        format!(
                            "{} · {} ({scope})",
                            t!("plugins.grant"),
                            capability_label(&capability)
                        )
                    };
                    perm_group = perm_group.child(
                        NyaButton::new(format!("request-{id}-{index}"), label)
                            .small()
                            .full_width()
                            .label_left()
                            .variant(NyaButtonVariant::Primary)
                            .disabled(self.busy(cx))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.operation(
                                    PluginOperation::Authorize {
                                        id: id.clone(),
                                        expected_revision,
                                        capability: capability.clone(),
                                        scope: scope.clone(),
                                    },
                                    cx,
                                );
                            })),
                    );
                }
            }

            if !grantable_capabilities.is_empty() {
                let mut chips = div().flex().flex_wrap().gap_1();
                for capability in grantable_capabilities {
                    let id = plugin.id.clone();
                    let revision = plugin.revision;
                    let active =
                        self.selected_permission
                            .as_ref()
                            .is_some_and(|(sel_id, sel_cap, _)| {
                                sel_id == &id && sel_cap == &capability
                            });
                    chips = chips.child(
                        NyaButton::new(
                            format!("grant-{id}-{capability}"),
                            capability_label(&capability),
                        )
                        .small()
                        .compact()
                        .selected(active)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if active {
                                this.selected_permission = None;
                            } else {
                                this.selected_permission =
                                    Some((id.clone(), capability.clone(), revision));
                            }
                            cx.notify();
                        })),
                    );
                }
                perm_group = perm_group.child(chips);
            }

            if let Some((sel_id, capability, expected_revision)) = self.selected_permission.clone()
                && sel_id == plugin.id
            {
                perm_group = perm_group.child(
                    div()
                        .p_2()
                        .rounded_sm()
                        .border_1()
                        .border_color(rgb(palette.border))
                        .bg(rgb(palette.surface))
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(palette.text))
                                .child(capability_label(&capability)),
                        )
                        .child(
                            div()
                                .w_full()
                                .h(px(30.))
                                .child(NyaInput::new(&self.permission_scope)),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_1()
                                .child(
                                    div().flex_1().min_w_0().child(
                                        NyaButton::new("extension-grant", t!("plugins.grant"))
                                            .small()
                                            .full_width()
                                            .variant(NyaButtonVariant::Primary)
                                            .disabled(self.busy(cx))
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                let scope = this
                                                    .permission_scope
                                                    .read(cx)
                                                    .value(cx)
                                                    .to_string();
                                                this.operation(
                                                    PluginOperation::Authorize {
                                                        id: sel_id.clone(),
                                                        expected_revision,
                                                        capability: capability.clone(),
                                                        scope,
                                                    },
                                                    cx,
                                                );
                                                this.selected_permission = None;
                                            })),
                                    ),
                                )
                                .child(
                                    NyaButton::new("extension-grant-cancel", t!("common.cancel"))
                                        .small()
                                        .compact()
                                        .variant(NyaButtonVariant::Ghost)
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.selected_permission = None;
                                            cx.notify();
                                        })),
                                ),
                        ),
                );
            }

            card = card.child(perm_group);
        }

        card
    }

    fn render_tool_panels(
        &self,
        snapshot: &CatalogSnapshot,
        palette: ThemePalette,
        cx: &mut Context<Self>,
    ) -> Div {
        use nyaterm_core::plugins::extension::PanelElement;
        let mut container = div()
            .w_full()
            .min_w_0()
            .flex_none()
            .flex()
            .flex_col()
            .gap_2();
        for ((owner, panel_id), panel) in &self.tool_panels {
            let mut view = div()
                .w_full()
                .min_w_0()
                .flex()
                .flex_col()
                .gap_2()
                .p(px(10.))
                .rounded_md()
                .border_1()
                .border_color(rgb(palette.border))
                .bg(rgb(palette.surface_elevated))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(panel.title.clone()),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_size(px(11.))
                                .text_color(rgb(palette.text_dimmed))
                                .child(owner.clone()),
                        ),
                );
            for (index, element) in panel.elements.iter().enumerate() {
                match element {
                    PanelElement::Text { text } => {
                        view = view.child(
                            div()
                                .text_xs()
                                .line_height(px(18.))
                                .text_color(rgb(palette.text))
                                .child(text.clone()),
                        );
                    }
                    PanelElement::Log { text } => {
                        view = view.child(
                            div()
                                .id(format!("log-{owner}-{panel_id}-{index}"))
                                .w_full()
                                .h(px(160.))
                                .p_2()
                                .rounded_sm()
                                .border_1()
                                .border_color(rgb(palette.border))
                                .bg(rgb(palette.surface))
                                .text_xs()
                                .overflow_y_scrollbar()
                                .child(text.clone()),
                        );
                    }
                    PanelElement::Table { columns, rows } => {
                        let mut table = div().flex().flex_col().w_full().text_xs();
                        let mut header = div()
                            .flex()
                            .w_full()
                            .py_1()
                            .border_b_1()
                            .border_color(rgb(palette.border))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(palette.text_muted));
                        for column in columns {
                            header = header.child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .px_2()
                                    .truncate()
                                    .child(column.clone()),
                            );
                        }
                        table = table.child(header);
                        for row in rows {
                            let mut cells = div()
                                .flex()
                                .w_full()
                                .py_1()
                                .border_b_1()
                                .border_color(rgb(palette.border));
                            for cell in row {
                                cells = cells.child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .px_2()
                                        .truncate()
                                        .child(cell.clone()),
                                );
                            }
                            table = table.child(cells);
                        }
                        view = view.child(
                            div()
                                .id(format!("table-{owner}-{panel_id}-{index}"))
                                .w_full()
                                .h(px(180.))
                                .rounded_sm()
                                .border_1()
                                .border_color(rgb(palette.border))
                                .bg(rgb(palette.surface))
                                .overflow_y_scrollbar()
                                .child(table),
                        );
                    }
                    PanelElement::Input { id, label, .. } => {
                        if let Some(input) = panel.inputs.get(id) {
                            view = view
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(palette.text_muted))
                                        .child(label.clone()),
                                )
                                .child(div().w_full().h(px(32.)).child(NyaInput::new(input)));
                        }
                    }
                    PanelElement::Progress {
                        label,
                        completed,
                        total,
                    } => {
                        let ratio = if *total > 0 {
                            *completed as f64 / *total as f64
                        } else {
                            0.
                        };
                        view = view.child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .text_xs()
                                        .text_color(rgb(palette.text_muted))
                                        .child(div().truncate().child(label.clone()))
                                        .child(format!("{completed}/{total}")),
                                )
                                .child(progress_bar(palette, ratio)),
                        );
                    }
                    PanelElement::Button { action, label } => {
                        let contribution = snapshot
                            .contributions
                            .iter()
                            .find(|contribution| contribution.id == format!("{owner}:{action}"))
                            .cloned();
                        let inputs = panel.inputs.clone();
                        let session = panel.session.clone();
                        view = view.child(
                            NyaButton::new(
                                format!("panel-{owner}-{panel_id}-{index}"),
                                label.clone(),
                            )
                            .small()
                            .disabled(self.busy(cx) || contribution.is_none())
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    if let Some(contribution) = &contribution {
                                        let mut input = ActionInput {
                                            text: session.as_ref().map(|session| {
                                                serde_json::json!({"session": session}).to_string()
                                            }),
                                            ..Default::default()
                                        };
                                        for parameter in &contribution.action.parameters {
                                            if let Some(field) = inputs.get(&parameter.id) {
                                                match parse_parameter(
                                                    parameter.kind,
                                                    &field.read(cx).value(cx),
                                                ) {
                                                    Ok(value) => {
                                                        input
                                                            .parameters
                                                            .insert(parameter.id.clone(), value);
                                                    }
                                                    Err(error) => {
                                                        this.notify_error(error.message, cx);
                                                        cx.notify();
                                                        return;
                                                    }
                                                }
                                            }
                                        }
                                        this.operation(
                                            PluginOperation::Invoke {
                                                contribution_id: contribution.id.clone(),
                                                expected_revision: contribution.revision,
                                                input,
                                            },
                                            cx,
                                        );
                                    }
                                },
                            )),
                        );
                    }
                }
            }
            container = container.child(view);
        }
        container
    }

    fn render_workbench(&self, palette: ThemePalette, cx: &mut Context<Self>) -> Div {
        let mut bench = div()
            .w_full()
            .min_w_0()
            .flex_none()
            .pt_1()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .debug_selector(|| "plugins-back-to-list".to_string())
                    .flex()
                    .items_center()
                    .child(
                        NyaButton::new("plugins-back-to-list-btn", t!("plugins.backToList"))
                            .icon("icons/fe/back.svg")
                            .small()
                            .compact()
                            .variant(NyaButtonVariant::Ghost)
                            .disabled(self.busy(cx) && !self.can_cancel())
                            .on_click(cx.listener(|this, _, _, cx| this.close_workbench(cx))),
                    ),
            );

        if let Some(selected) = &self.selected {
            let mut form = div()
                .w_full()
                .min_w_0()
                .flex_none()
                .p(px(10.))
                .rounded_md()
                .border_1()
                .border_color(rgb(palette.border))
                .bg(rgb(palette.surface_elevated))
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(palette.text))
                                .child(selected.action.name.clone()),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_size(px(11.))
                                .text_color(rgb(palette.text_dimmed))
                                .child(selected.id.clone()),
                        ),
                );
            if !selected.action.description.is_empty() {
                form = form.child(
                    div()
                        .text_xs()
                        .line_height(px(18.))
                        .text_color(rgb(palette.text_muted))
                        .child(selected.action.description.clone()),
                );
            }
            for parameter in &self.parameters {
                form = form
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_1()
                            .text_xs()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .min_w_0()
                                    .child(
                                        div()
                                            .truncate()
                                            .text_color(rgb(palette.text))
                                            .child(parameter.name.clone()),
                                    )
                                    .when(parameter.required, |row| {
                                        row.child(div().text_color(rgb(palette.danger)).child("*"))
                                    }),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(px(11.))
                                    .text_color(rgb(palette.text_dimmed))
                                    .child(parameter_kind_label(parameter.kind)),
                            ),
                    )
                    .child(
                        div()
                            .w_full()
                            .flex_none()
                            .h(px(32.))
                            .child(NyaInput::new(&parameter.input)),
                    );
            }
            if selected.action.result == ResultKind::Text {
                form = form
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(palette.text_muted))
                            .child(t!("plugins.explicitText")),
                    )
                    .child(
                        div()
                            .w_full()
                            .flex_none()
                            .h(px(110.))
                            .child(NyaInput::new(&self.text)),
                    );
            }
            let mut action_row = div().flex_none().flex().items_center().gap_1().child(
                div().flex_1().min_w_0().child(
                    NyaButton::new("plugins-invoke", t!("plugins.invoke"))
                        .icon("icons/play.svg")
                        .small()
                        .full_width()
                        .tooltip(t!("plugins.invoke"))
                        .variant(NyaButtonVariant::Primary)
                        .loading(self.can_cancel())
                        .disabled(self.busy(cx))
                        .on_click(cx.listener(|this, _, _, cx| this.invoke(cx))),
                ),
            );
            if self.can_cancel() {
                action_row = action_row.child(
                    NyaButton::new("plugins-cancel", t!("plugins.cancel"))
                        .small()
                        .compact()
                        .on_click(cx.listener(|this, _, _, cx| this.cancel(cx))),
                );
            }
            form = form.child(action_row);
            bench = bench.child(form);
        }

        if let Some(preview) = &self.preview {
            let mut result = div()
                .w_full()
                .min_w_0()
                .flex_none()
                .p(px(10.))
                .rounded_md()
                .border_1()
                .border_color(rgb(palette.border))
                .bg(rgb(palette.surface_elevated))
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(preview.title.clone()),
                        )
                        .child(
                            NyaButton::new("plugins-copy", "")
                                .icon("icons/copy.svg")
                                .small()
                                .compact()
                                .variant(NyaButtonVariant::Ghost)
                                .tooltip(t!("plugins.copy"))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    let value = this.output.read(cx).value(cx);
                                    let copied = !value.is_empty();
                                    if copied {
                                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                            value,
                                        ));
                                    }
                                    window.notify_operation(
                                        "copy-plugin-result",
                                        if copied {
                                            NyaNotificationKind::Success
                                        } else {
                                            NyaNotificationKind::Warning
                                        },
                                        t!(if copied {
                                            "common.copied"
                                        } else {
                                            "common.nothingToCopy"
                                        })
                                        .to_string(),
                                        cx,
                                    );
                                })),
                        ),
                )
                .child(
                    div()
                        .w_full()
                        .flex_none()
                        .h(px(150.))
                        .child(NyaInput::new(&self.output)),
                );
            if preview.kind == ResultKind::Command {
                result = result.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            NyaButton::new("plugins-fill", t!("plugins.fill"))
                                .small()
                                .full_width()
                                .variant(NyaButtonVariant::Primary)
                                .tooltip(t!("plugins.fill"))
                                .on_click(cx.listener(|this, _, _, cx| this.fill(false, cx))),
                        )
                        .child(
                            NyaButton::new("plugins-replace", t!("plugins.replace"))
                                .small()
                                .full_width()
                                .tooltip(t!("plugins.replace"))
                                .on_click(cx.listener(|this, _, _, cx| this.fill(true, cx))),
                        ),
                );
            } else {
                result = result.child(
                    NyaButton::new("plugins-reuse", t!("plugins.reuse"))
                        .small()
                        .full_width()
                        .tooltip(t!("plugins.reuse"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            let value = this.output.read(cx).value(cx);
                            this.text
                                .update(cx, |state, cx| state.set_content(&value, cx));
                        })),
                );
            }
            bench = bench.child(result);
        }

        bench
    }
}

impl Render for PluginPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = self
            .app
            .upgrade()
            .map(|app| app.read(cx).theme_palette())
            .unwrap_or_else(|| crate::theme::theme_palette("dark"));
        if self.detail_key.is_some() {
            return self.render_details(palette, cx).into_any_element();
        }
        let entries = super::projection::entries(self.process.read(cx));
        let query = self.search.read(cx).value(cx).trim().to_lowercase();
        let count = entries.iter().filter(|entry| entry.installed).count();
        let updates = entries
            .iter()
            .filter(|entry| entry.installed && entry.update_available)
            .count();
        let toolbar = self.toolbar(count, updates, palette, cx);
        let content = self.render_list(&entries, &query, palette, cx);
        div()
            .size_full()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(rgb(palette.surface))
            .text_color(rgb(palette.text))
            .text_sm()
            .whitespace_normal()
            .child(toolbar)
            .child(content)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use crate::features::plugins::process::PluginProcess;
    use crate::features::plugins::view::PluginPanel;
    use crate::features::test_support::app_with_visible_local_session;
    use crate::models::{ActivityBarZone, NavItem};
    use futures::executor::block_on;
    use gpui::{AppContext, TestAppContext};
    use nyaterm_core::plugins::native::NativeComponentId;
    use nyaterm_core::runtime::{AppRuntime, RuntimeMode};
    use nyaterm_core::test_support::TestTempDir;
    use nyaterm_plugin_host::service::PluginOperation;
    use std::path::Path;
    use std::time::{Duration, Instant};

    #[test]
    fn native_install_releases_panel_update_before_acquiring_service() {
        let root = TestTempDir::new("nyaterm-native-install-panel");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, root.path(), "native-install");
        let process = cx.new(|_| PluginProcess::empty());
        let panel = cx.new(|cx| PluginPanel::new(app.downgrade(), process, cx));
        cx.update_entity(&app, |app, _| {
            app.plugins.panel = panel.clone();
            app.plugins.native_launch_fixture = None;
        });

        for id in NativeComponentId::ALL {
            cx.update_entity(&app, |app, _| app.shell.set_status("before install"));
            // Exercise the button's handler while the panel is leased, without
            // a launch fixture that would bypass the real panel/service lookup.
            cx.update_entity(&panel, |panel, cx| {
                panel.install_native(id, cx);
                assert_eq!(app.read(cx).shell.status(), "before install");
            });
            cx.run_until_parked();
            assert_eq!(
                cx.read(|cx| app.read(cx).shell.status().to_string()),
                "before install"
            );
        }
    }

    #[test]
    fn management_survives_closed_details_and_reports_errors_in_shared_process() {
        let root = TestTempDir::new("nyaterm-plugin-management-owner");
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, root.path(), "management");
        let runtime = AppRuntime::from_parts_for_test(
            RuntimeMode::Portable,
            root.path().into(),
            root.join("config"),
            root.join("logs"),
            root.join("cache"),
            None,
        );
        let process = cx.new(|cx| PluginProcess::new(runtime, cx));
        let service = cx.read(|cx| process.read(cx).service().unwrap());
        let panel = cx.new(|cx| PluginPanel::new(app.downgrade(), process.clone(), cx));
        cx.update_entity(&app, |app, _| app.plugins.panel = panel.clone());
        let key = nyaterm_core::plugins::management::PluginKey::Wasm("missing".into());
        cx.update_entity(&app, |app, cx| {
            let detail = app.open_plugin_details(key.clone(), 0, cx);
            detail.update(cx, |detail, cx| {
                detail.operation(
                    PluginOperation::Update {
                        id: "missing".into(),
                        source: root.join("missing"),
                    },
                    cx,
                )
            });
            app.close_plugin_details(&key, cx);
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        while cx.read(|cx| process.read(cx).job_busy("missing")) {
            cx.run_until_parked();
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        cx.read(|cx| {
            assert!(app.read(cx).plugins.details.is_empty());
            assert!(process.read(cx).jobs["missing"].error.is_some());
        });
        service.shutdown();
    }

    #[test]
    fn manager_renders_and_real_guest_results_require_explicit_fill_and_valid_revision() {
        let root = TestTempDir::new("nyaterm-plugin-manager-view");
        let app_root = TestTempDir::new("nyaterm-plugin-manager-app");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, app_root.path(), "draft-session");
        cx.update_entity(&app, |app, cx| {
            app.sync_component_theme(cx);
            app.apply_send_command_draft("existing user draft".into(), cx);
        });
        let runtime = AppRuntime::from_parts_for_test(
            RuntimeMode::Portable,
            root.path().into(),
            root.join("config"),
            root.join("logs"),
            root.join("cache"),
            None,
        );
        let process = cx.new(|cx| PluginProcess::new(runtime, cx));
        let service = cx.read(|cx| process.read(cx).service().unwrap());
        let source = root.join("source");
        std::fs::create_dir_all(&source).unwrap();
        let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
        std::fs::copy(
            crate_root.join("../../examples/plugins/diagnostic-command/plugin.toml"),
            source.join("plugin.toml"),
        )
        .unwrap();
        std::fs::copy(
            crate_root.join("../nyaterm-plugin-host/tests/fixtures/diagnostic-command.wasm"),
            source.join("plugin.wasm"),
        )
        .unwrap();
        block_on(
            service
                .submit(PluginOperation::Install {
                    source,
                    development: false,
                })
                .unwrap(),
        )
        .unwrap()
        .unwrap();
        let panel = cx.new(|cx| PluginPanel::new(app.downgrade(), process.clone(), cx));
        let panel_view = panel.clone();
        let (_, vcx) =
            cx.add_window_view(move |window, cx| nyaterm_ui::nya_root(panel_view, window, cx));
        vcx.update_entity(&app, |app, cx| {
            app.plugins.panel = panel.clone();
            app.open_panel(NavItem::Plugins, cx);
        });
        vcx.run_until_parked();
        // Sidebar rows and their actions must fit without horizontal scrolling,
        // including at the minimum practical panel width.
        for width in [240., 360., 500.] {
            vcx.simulate_resize(gpui::size(gpui::px(width), gpui::px(700.)));
            for discover in [true, false] {
                vcx.update_entity(&panel, |panel, cx| {
                    panel.discover = discover;
                    cx.notify();
                });
                for _ in 0..3 {
                    vcx.update(|window, cx| {
                        let _ = window.draw(cx);
                    });
                    vcx.run_until_parked();
                }
                let search_bounds = vcx.debug_bounds("extensions-search").unwrap();
                assert_eq!(search_bounds.size.height, gpui::px(32.));
                assert!(search_bounds.left() >= gpui::px(0.));
                assert!(search_bounds.right() <= gpui::px(width));
                if discover {
                    let bounds = vcx.debug_bounds("plugin-row-nyaterm-native-spice").unwrap();
                    assert!(bounds.left() >= gpui::px(0.));
                    assert!(bounds.right() <= gpui::px(width));
                    assert!(bounds.size.height <= gpui::px(88.));
                } else {
                    let bounds = vcx.debug_bounds("plugin-row-diagnostic-command").unwrap();
                    assert!(bounds.right() <= gpui::px(width));
                    assert!(bounds.size.height <= gpui::px(64.));
                    assert!(
                        vcx.debug_bounds("plugin-row-nyaterm-native-spice")
                            .is_none()
                    );
                }
            }
        }
        vcx.update_entity(&panel, |panel, cx| {
            panel.discover = true;
            panel
                .search
                .update(cx, |search, cx| search.set_content("VnC", cx));
        });
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(vcx.debug_bounds("plugin-row-nyaterm-native-vnc").is_some());
        assert!(
            vcx.debug_bounds("plugin-row-nyaterm-native-spice")
                .is_none()
        );
        assert!(vcx.debug_bounds("plugin-row-diagnostic-command").is_none());
        vcx.simulate_resize(gpui::size(gpui::px(240.), gpui::px(700.)));
        vcx.update_entity(&panel, |panel, cx| {
            panel.search.update(cx, |search, cx| search.clear(cx));
            panel.detail_key = Some(nyaterm_core::plugins::management::PluginKey::Wasm(
                "diagnostic-command".into(),
            ));
            panel.detail_tab = 3;
            cx.notify();
        });
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        let card_bounds = vcx.debug_bounds("plugin-card-diagnostic-command").unwrap();
        assert!(card_bounds.left() >= gpui::px(0.));
        assert!(card_bounds.right() <= gpui::px(240.));
        let contribution = service.snapshot().contributions[0].clone();
        vcx.update_entity(&panel, |panel, cx| {
            panel.select(contribution, cx);
            panel.invoke(cx);
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            vcx.run_until_parked();
            if vcx.read(|cx| !panel.read(cx).busy(cx)) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "manager invocation did not complete"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        for _ in 0..3 {
            vcx.update(|window, cx| {
                panel.update(cx, |_, cx| cx.notify());
                let _ = window.draw(cx);
            });
            vcx.run_until_parked();
        }
        let back_bounds = vcx.debug_bounds("plugins-back-to-list").unwrap();
        assert!(back_bounds.left() >= gpui::px(0.));
        assert!(back_bounds.right() <= gpui::px(240.));
        let generation = vcx.read(|cx| panel.read(cx).generation);
        vcx.update_entity(&app, |app, cx| {
            app.hide_activity_entry("plugins".into(), cx);
            app.show_activity_entry("plugins".into(), cx);
            app.move_activity_entry("plugins".into(), ActivityBarZone::RightBottom, None, cx);
            app.open_panel(NavItem::Plugins, cx);
            assert_eq!(app.plugins.panel.entity_id(), panel.entity_id());
        });
        vcx.update_entity(&panel, |panel, cx| {
            assert_eq!(panel.generation, generation);
            assert!(panel.selected.is_some());
            assert!(panel.preview.is_some());
            assert!(panel.error_notifications.is_empty());
            assert_eq!(
                panel.output.read(cx).value(cx),
                "nslookup example.org\nping example.org"
            );
            let app = panel.app.upgrade().unwrap();
            assert_eq!(
                app.read(cx).send_command.presentation(cx).draft,
                "existing user draft"
            );
            panel.fill(false, cx);
            assert_eq!(
                app.read(cx).send_command.presentation(cx).draft,
                "existing user draft\nnslookup example.org\nping example.org"
            );
            assert!(!app.read(cx).send_command.presentation(cx).sending);
        });
        block_on(
            service
                .submit(PluginOperation::Enable {
                    id: "diagnostic-command".into(),
                    enabled: false,
                })
                .unwrap(),
        )
        .unwrap()
        .unwrap();
        // Even before the catalog event is rendered, a revoked preview cannot fill.
        vcx.update_entity(&panel, |panel, cx| {
            let before = panel
                .app
                .upgrade()
                .unwrap()
                .read(cx)
                .send_command
                .presentation(cx)
                .draft;
            panel.fill(true, cx);
            assert_eq!(
                panel
                    .app
                    .upgrade()
                    .unwrap()
                    .read(cx)
                    .send_command
                    .presentation(cx)
                    .draft,
                before
            );
        });
        vcx.run_until_parked();
        assert!(vcx.read(|cx| panel.read(cx).preview.is_none()));
        service.shutdown();
    }
}
