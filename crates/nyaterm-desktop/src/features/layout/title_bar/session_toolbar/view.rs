use gpui::{Context, IntoElement, MouseButton, div, prelude::*, px};
use nyaterm_ui::NyaIconButton;
use rust_i18n::t;

use super::policy::SessionToolbarAction;
use crate::features::NyaTermApp;
use crate::features::layout::title_bar::definition::{TitleMenuCommand, TitleMenuDefinition};
use crate::models::HeaderStatusMode;

impl NyaTermApp {
    pub(in crate::features::layout::title_bar) fn session_toolbar_menu_definitions(
        &self,
    ) -> Vec<TitleMenuDefinition> {
        let context = self.session_toolbar_context();
        let mut items = HeaderStatusMode::ALL
            .into_iter()
            .filter(|mode| context.supports_mode(*mode))
            .map(|mode| {
                let enabled = context.mode_enabled(mode);
                let mut item = TitleMenuDefinition::action(t!(mode.i18n_key()))
                    .checked(context.visible && context.effective_mode() == mode)
                    .disabled(!enabled)
                    .command(TitleMenuCommand::SetHeaderStatus(mode));
                if !enabled {
                    item = item.status(
                        t!("headerStatus.temporarilyUnavailable"),
                        self.theme_palette().text_muted,
                    );
                }
                item
            })
            .collect::<Vec<_>>();
        if let Some(id) = context.session_id.as_deref() {
            let mut actions = Vec::new();
            for action in [
                SessionToolbarAction::Fullscreen,
                SessionToolbarAction::Reconnect,
            ] {
                let Some(enabled) = context.action_enabled(action) else {
                    continue;
                };
                let label = match action {
                    SessionToolbarAction::Fullscreen if context.fullscreen_active => {
                        t!("remoteDesktop.exitFullscreen")
                    }
                    SessionToolbarAction::Fullscreen => t!("remoteDesktop.fullscreen"),
                    SessionToolbarAction::Reconnect => t!("tabCtx.reconnect"),
                };
                let mut item = TitleMenuDefinition::action(label)
                    .icon(action.icon())
                    .disabled(!enabled)
                    .command(TitleMenuCommand::SessionToolbarAction {
                        session_id: id.to_string(),
                        action,
                    });
                if action == SessionToolbarAction::Fullscreen {
                    item = item.shortcut(
                        self.display_shortcut_for("view.toggleRdpFullscreen", "Ctrl+Shift+Alt+F11"),
                    );
                }
                if !enabled {
                    let reason = if action == SessionToolbarAction::Reconnect
                        && context.capabilities.reconnect_requires_disconnect
                        && !context.capabilities.disconnected
                    {
                        "headerStatus.retryAfterDisconnect"
                    } else {
                        "headerStatus.temporarilyUnavailable"
                    };
                    item = item.status(t!(reason), self.theme_palette().text_muted);
                }
                actions.push(item);
            }
            if !actions.is_empty() {
                items.push(TitleMenuDefinition::separator());
                items.extend(actions);
            }
        }
        items
    }

    pub(in crate::features::layout::title_bar) fn session_toolbar_shortcuts(
        &self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let context = self.session_toolbar_context();
        let mut buttons = div()
            .flex()
            .flex_none()
            .items_center()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
        if let Some(id) = context.session_id.as_deref() {
            for action in context.shortcut_actions(self.shell.viewport_size().0) {
                let label = match action {
                    SessionToolbarAction::Fullscreen => format!(
                        "{} ({})",
                        if context.fullscreen_active {
                            t!("remoteDesktop.exitFullscreen")
                        } else {
                            t!("remoteDesktop.fullscreen")
                        },
                        self.display_shortcut_for("view.toggleRdpFullscreen", "Ctrl+Shift+Alt+F11")
                    ),
                    SessionToolbarAction::Reconnect => t!("tabCtx.reconnect").to_string(),
                };
                let enabled = context.action_enabled(action) == Some(true);
                let tooltip = if enabled {
                    label
                } else {
                    format!("{label} · {}", t!("headerStatus.temporarilyUnavailable"))
                };
                let target = id.to_string();
                buttons = buttons.child(
                    NyaIconButton::new(format!("session-toolbar-{action:?}"), action.icon())
                        .size(px(26.))
                        .icon_size(px(14.))
                        .disabled(!enabled)
                        .tooltip(tooltip)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.execute_session_toolbar_action(&target, action, window, cx);
                        })),
                );
            }
        }
        buttons
    }
}

impl SessionToolbarAction {
    fn icon(self) -> &'static str {
        match self {
            Self::Fullscreen => "icons/window/maximize.svg",
            Self::Reconnect => "icons/fe/refresh.svg",
        }
    }
}
