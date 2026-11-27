use crate::features::NyaTermApp;
use crate::send_command::SendCommandDataType;
use gpui::Context;
use nyaterm_core::command_draft::DraftOrigin;
use nyaterm_core::plugins::invocation::ordinary_text;
use nyaterm_core::plugins::{ErrorCode, MAX_TEXT_BYTES, PluginError, PluginResult};

pub(super) fn compose(existing: &str, incoming: &str, replace: bool) -> PluginResult<String> {
    if !ordinary_text(incoming) || incoming.len() > MAX_TEXT_BYTES {
        return Err(PluginError::new(
            ErrorCode::InvalidResult,
            "Draft contains controls or exceeds the size limit",
        ));
    }
    let draft = if replace || existing.is_empty() {
        incoming.to_string()
    } else {
        format!("{existing}\n{incoming}")
    };
    if draft.len() > MAX_TEXT_BYTES {
        return Err(PluginError::new(
            ErrorCode::InvalidResult,
            "Combined draft exceeds the size limit",
        ));
    }
    Ok(draft)
}

impl NyaTermApp {
    /// This adapter mutates only the composer. It has no terminal-write, target,
    /// send/Enter or history operation, even when a plugin returns many lines.
    pub(in crate::features) fn fill_plugin_draft(
        &mut self,
        tab_id: crate::features::panels::SendCommandTabId,
        incoming: &str,
        replace: bool,
        origin: DraftOrigin,
        cx: &mut Context<Self>,
    ) -> PluginResult<()> {
        if self.send_command.active_id() != tab_id || self.send_command.tab_name(tab_id).is_none() {
            return Err(PluginError::new(
                ErrorCode::InvalidResult,
                "The selected send tab is no longer active",
            ));
        }
        let state = self.send_command.view_presentation(cx);
        if state.sending || state.data_type != SendCommandDataType::Text {
            return Err(PluginError::new(
                ErrorCode::InvalidResult,
                "Stop sending and select text mode before filling a plugin draft",
            ));
        }
        let draft = compose(&state.draft, incoming, replace)?;
        self.send_command
            .fill_plugin_draft(draft.clone(), origin, replace);
        self.reset_text_input(&self.send_command.input_id("draft"), &draft, cx);
        self.set_bottom_panel_mode(crate::models::BottomPanelMode::CommandSend);
        self.shell
            .set_status(rust_i18n::t!("plugins.draftFilled").to_string());
        cx.notify();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::features::plugins::draft::compose;
    use crate::features::test_support::app_with_visible_local_session;
    use gpui::AppContext;
    use nyaterm_core::command_draft::DraftOrigin;
    use nyaterm_core::test_support::TestTempDir;

    fn origin() -> DraftOrigin {
        DraftOrigin::Plugin {
            plugin_id: "diagnostic".into(),
            action_id: "ping".into(),
            revision: 4,
        }
    }
    #[test]
    fn stale_plugin_fill_never_overwrites_another_send_tab() {
        let mut cx = gpui::TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let root = TestTempDir::new("nyaterm-plugin-send-tabs");
        let app = app_with_visible_local_session(&mut cx, root.path(), "plugin-send-tabs");
        cx.update_entity(&app, |app, cx| {
            let first = app.send_command.active_id();
            let second = app.send_command.add_tab(cx);
            app.send_command.apply_draft("user".into());
            assert!(
                app.fill_plugin_draft(first, "plugin", true, origin(), cx)
                    .is_err()
            );
            assert_eq!(app.send_command.presentation(cx).draft, "user");
            assert!(app.send_command.close_tabs(&[first]));
            assert!(
                app.fill_plugin_draft(first, "plugin", true, origin(), cx)
                    .is_err()
            );
            app.fill_plugin_draft(second, "plugin", false, origin(), cx)
                .unwrap();
            assert_eq!(app.send_command.presentation(cx).draft, "user\nplugin");
        });
    }

    #[test]
    fn explicit_fill_preserves_existing_draft_and_replacement_requires_separate_choice() {
        assert_eq!(
            compose("echo user", "ping host", false).unwrap(),
            "echo user\nping host"
        );
        assert_eq!(
            compose("echo user", "ping host", true).unwrap(),
            "ping host"
        );
        assert_eq!(compose("", "ping host", false).unwrap(), "ping host");
        assert!(compose("", "\x1b[31m", false).is_err());
    }

    #[test]
    fn application_adapter_only_fills_draft_and_preserves_session_history_and_user_templates() {
        let mut cx = gpui::TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let root = TestTempDir::new("nyaterm-plugin-draft-app");
        let app = app_with_visible_local_session(&mut cx, root.path(), "plugin-draft-session");
        cx.update_entity(&app, |app, cx| {
            app.apply_send_command_draft("echo user".into(), cx);
            let session = app.session.active_id().map(str::to_owned);
            let commands = serde_json::to_string(app.commands.quick_commands()).unwrap();
            let history = serde_json::to_string(app.commands.command_history()).unwrap();
            let target = app.send_command.presentation(cx).target;
            app.fill_plugin_draft(
                app.send_command.active_id(),
                "ping host",
                false,
                origin(),
                cx,
            )
            .unwrap();
            let state = app.send_command.presentation(cx);
            assert_eq!(state.draft, "echo user\nping host");
            assert_eq!(state.provenance.origins, vec![DraftOrigin::User, origin()]);
            app.apply_send_command_draft("echo user\nping other-host".into(), cx);
            assert!(app.send_command.presentation(cx).provenance.edited);
            assert!(!state.sending);
            assert_eq!(state.target, target);
            assert_eq!(app.session.active_id(), session.as_deref());
            assert_eq!(
                serde_json::to_string(app.commands.quick_commands()).unwrap(),
                commands
            );
            assert_eq!(
                serde_json::to_string(app.commands.command_history()).unwrap(),
                history
            );
            app.fill_plugin_draft(
                app.send_command.active_id(),
                "replacement",
                true,
                origin(),
                cx,
            )
            .unwrap();
            assert_eq!(app.send_command.presentation(cx).draft, "replacement");
            assert_eq!(
                app.send_command.presentation(cx).provenance.origins,
                vec![origin()]
            );
            assert!(!app.send_command.presentation(cx).provenance.edited);
            app.send_command.clear_draft(cx);
            assert!(
                app.send_command
                    .presentation(cx)
                    .provenance
                    .origins
                    .is_empty()
            );
        });
    }
}
