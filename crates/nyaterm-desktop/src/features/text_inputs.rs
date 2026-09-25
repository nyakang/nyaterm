//! A registry of real text inputs, keyed by an id the caller picks.
//!
//! Ordinary form, prompt and search inputs use [`NyaInputState`] entities
//! backed by `gpui-kit`. The paste review uses the shared document-editor
//! wrapper, while the terminal and remote file editor keep dedicated input
//! handlers because their selection and command semantics are different.
//!
//! The connection editor owns dedicated [`NyaInputState`] entities for fields
//! with custom focus choreography. For smaller panels, the fields live here
//! instead, keyed by a string id and created the first time a panel renders one.
//! A panel needs no state of its own beyond the value it already keeps, and
//! edits arrive as one event with the id attached.

use std::collections::HashMap;

use gpui::{
    AppContext, BoxShadow, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Rgba, SharedString, Styled as _, Subscription, Window, div,
    prelude::FluentBuilder as _, px, rgb,
};
use nyaterm_ui::{
    NYA_FORM_CONTROL_HEIGHT_PX, NyaInputEvent, NyaInputShell, NyaInputState, NyaNumberInput,
    NyaNumberInputEvent, NyaNumberInputOptions, NyaNumberInputState, NyaSearchInput,
};

use super::NyaTermApp;

pub(in crate::features) const ORDINARY_INPUT_SHELL_PADDING_X_PX: f32 = 4.;
pub(in crate::features) const ORDINARY_NUMBER_INPUT_WIDTH_PX: f32 = 128.;

pub(in crate::features) fn ordinary_input_shell_border_color(
    palette: crate::theme::ThemePalette,
    focused: bool,
) -> Rgba {
    rgb(if focused {
        palette.focus_ring
    } else {
        palette.border
    })
}

pub(in crate::features) fn ordinary_input_focus_ring(
    palette: crate::theme::ThemePalette,
) -> Vec<BoxShadow> {
    vec![
        BoxShadow::new(px(0.), px(0.), rgb(palette.focus_ring).alpha(0.32).into())
            .spread_radius(px(2.)),
    ]
}

/// How a field should behave, for the one call that creates it.
///
/// Only read when the field is first seen; later renders reuse the entity, so
/// changing this for an existing id has no effect until the id is forgotten.
#[derive(Default, Clone)]
pub(in crate::features) struct TextInputSetup {
    pub placeholder: SharedString,
    pub masked: bool,
    pub multi_line: bool,
    pub submit_on_enter: bool,
    /// A multi-line box holding source, rendered with a line-number gutter.
    pub code: bool,
}

impl TextInputSetup {
    pub fn placeholder(placeholder: impl Into<SharedString>) -> Self {
        Self {
            placeholder: placeholder.into(),
            ..Default::default()
        }
    }

    pub fn masked() -> Self {
        Self {
            masked: true,
            ..Default::default()
        }
    }

    pub fn multi_line(placeholder: impl Into<SharedString>) -> Self {
        Self {
            placeholder: placeholder.into(),
            masked: false,
            multi_line: true,
            submit_on_enter: false,
            code: false,
        }
    }

    /// A script box: multi-line, with the gutter Tauri hand-rolls for its command
    /// editor.
    pub fn code(placeholder: impl Into<SharedString>) -> Self {
        Self {
            placeholder: placeholder.into(),
            masked: false,
            multi_line: true,
            submit_on_enter: false,
            code: true,
        }
    }

    pub fn submit_on_enter(mut self) -> Self {
        self.submit_on_enter = true;
        self
    }
}

/// The setup for a settings field, masked when it holds a secret.
///
/// A stored secret is never read back into its box: the box holds the draft
/// that replaces it, and the panel badges whether one is stored at all.
pub(in crate::features) fn secret_input_setup(secret: bool) -> TextInputSetup {
    if secret {
        TextInputSetup::masked()
    } else {
        TextInputSetup::default()
    }
}

#[derive(Default)]
pub(in crate::features) struct TextInputRegistry {
    fields: HashMap<SharedString, Entity<NyaInputState>>,
    number_fields: HashMap<SharedString, Entity<NyaNumberInputState>>,
    /// Kept alive alongside its field, so edits keep arriving.
    subscriptions: HashMap<SharedString, Subscription>,
    number_subscriptions: HashMap<SharedString, Subscription>,
}

impl NyaTermApp {
    pub(in crate::features) fn focus_text_input_if_present(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(field) = self.text_inputs.fields.get(id).cloned() else {
            return false;
        };
        window.focus(&field.read(cx).focus_handle(), cx);
        true
    }

    /// An existing field, without creating one.
    ///
    /// `text_input` takes `&mut self` because it builds the entity on first use, so
    /// a render that calls it mutates authoritative state on the first frame that
    /// shows the field. Render paths use this instead: the boundary that reveals a
    /// field is the boundary that builds it.
    pub(in crate::features) fn existing_text_input(
        &self,
        id: impl AsRef<str>,
    ) -> Option<Entity<NyaInputState>> {
        self.text_inputs.fields.get(id.as_ref()).cloned()
    }

    pub(in crate::features) fn text_input_fields_snapshot(
        &self,
    ) -> HashMap<SharedString, Entity<NyaInputState>> {
        self.text_inputs.fields.clone()
    }

    pub(in crate::features) fn number_input_fields_snapshot(
        &self,
    ) -> HashMap<SharedString, Entity<NyaNumberInputState>> {
        self.text_inputs.number_fields.clone()
    }

    /// The input for `id`, created on first use and seeded with `seed`.
    ///
    /// After that the field owns its own text: `seed` is ignored, because the
    /// field is the source of truth for what is being typed. Use
    /// [`Self::reset_text_input`] to push a value back down, and
    /// [`Self::forget_text_inputs`] when the thing being edited goes away.
    pub(in crate::features) fn text_input(
        &mut self,
        id: impl Into<SharedString>,
        seed: &str,
        setup: TextInputSetup,
        cx: &mut Context<Self>,
    ) -> Entity<NyaInputState> {
        self.text_input_with_language(id, seed, setup, None, cx)
    }

    fn text_input_with_language(
        &mut self,
        id: impl Into<SharedString>,
        seed: &str,
        setup: TextInputSetup,
        language: Option<SharedString>,
        cx: &mut Context<Self>,
    ) -> Entity<NyaInputState> {
        let id = id.into();
        if let Some(field) = self.text_inputs.fields.get(&id) {
            return field.clone();
        }

        let entity = cx.new(|cx| {
            let input = NyaInputState::new(cx, seed.to_string()).placeholder(setup.placeholder);
            let input = if setup.code {
                input.code(Some(4))
            } else if setup.multi_line {
                input.multi_line(Some(4))
            } else {
                input
            };
            let input = if let Some(language) = language {
                input.language(language)
            } else {
                input
            };
            input
                .masked(setup.masked)
                .submit_on_enter(setup.submit_on_enter)
        });
        let subscription_id = id.clone();
        let subscription =
            cx.subscribe(
                &entity,
                move |app: &mut NyaTermApp, _, event, cx| match event {
                    NyaInputEvent::Changed(text) | NyaInputEvent::Submitted(text) => {
                        app.on_text_input_changed(subscription_id.clone(), text.clone(), cx);
                    }
                    NyaInputEvent::Blurred(_) => {}
                },
            );
        self.text_inputs.fields.insert(id.clone(), entity.clone());
        self.text_inputs.subscriptions.insert(id, subscription);
        entity
    }

    /// A bordered box hosting the input for `id`.
    ///
    /// The box is the hit target, so clicking anywhere in it takes the caret —
    /// the text itself is only one line tall inside it.
    pub(in crate::features) fn text_input_box<I: Into<SharedString>>(
        &mut self,
        id: I,
        seed: &str,
        setup: TextInputSetup,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<I> {
        let id = id.into();
        let multi_line = setup.multi_line;
        let field = self.text_input(id.clone(), seed, setup, cx);
        let shell = NyaInputShell::new(id, &field);
        if multi_line {
            shell.multi_line()
        } else {
            shell
        }
    }

    pub(in crate::features) fn text_input_box_with_language<I: Into<SharedString>>(
        &mut self,
        id: I,
        seed: &str,
        setup: TextInputSetup,
        language: SharedString,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<I> {
        let id = id.into();
        let multi_line = setup.multi_line;
        let field = self.text_input_with_language(id.clone(), seed, setup, Some(language), cx);
        let shell = NyaInputShell::new(id, &field);
        if multi_line {
            shell.multi_line()
        } else {
            shell
        }
    }

    pub(in crate::features) fn search_input_box<I: Into<SharedString>>(
        &mut self,
        id: I,
        seed: &str,
        setup: TextInputSetup,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<I> {
        let id = id.into();
        let field = self.text_input(id.clone(), seed, setup, cx);
        NyaSearchInput::new(id, &field)
    }

    /// A caption above the input for `id`.
    ///
    /// The caption goes above rather than inside the box, so the whole width is
    /// what was typed — the same shape the connection editor settled on.
    pub(in crate::features) fn text_input_field<I: Into<SharedString>, C: Into<SharedString>>(
        &mut self,
        id: I,
        caption: C,
        seed: &str,
        setup: TextInputSetup,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<I, C> {
        let palette = self.theme_palette();
        let caption = caption.into();
        let input = self.text_input_box(id, seed, setup, cx);
        div()
            .min_w_0()
            .flex()
            .flex_col()
            .gap_1()
            .when(!caption.is_empty(), |this| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(rgb(palette.text_muted))
                        .child(caption),
                )
            })
            .child(input)
    }

    pub(in crate::features) fn number_input(
        &mut self,
        id: impl Into<SharedString>,
        seed: &str,
        setup: NyaNumberInputOptions,
        cx: &mut Context<Self>,
    ) -> Entity<NyaNumberInputState> {
        let id = id.into();
        if let Some(field) = self.text_inputs.number_fields.get(&id) {
            return field.clone();
        }

        let entity = cx.new(|cx| NyaNumberInputState::new(cx, seed.to_string(), setup));
        let subscription_id = id.clone();
        let subscription =
            cx.subscribe(
                &entity,
                move |app: &mut NyaTermApp, _, event, cx| match event {
                    NyaNumberInputEvent::Changed(text) | NyaNumberInputEvent::Submitted(text) => {
                        app.on_text_input_changed(subscription_id.clone(), text.clone(), cx);
                    }
                    NyaNumberInputEvent::Stepped(_) => {}
                },
            );
        self.text_inputs
            .number_fields
            .insert(id.clone(), entity.clone());
        self.text_inputs
            .number_subscriptions
            .insert(id, subscription);
        entity
    }

    /// Build an input for `id` if it does not exist yet, discarding the handle.
    ///
    /// The counterpart to the `SettingsPanel::existing_*` lookups. Creating an input is
    /// a mutation -- it builds the entity and its change subscription -- so a render
    /// that calls `text_input` or `number_input` writes to authoritative state on the
    /// first frame that shows the field. These let the boundary that reveals a field
    /// build it, and leave render able only to look one up.
    ///
    /// Note that the options are only ever honoured here: `number_input` returns early
    /// for an id it already has, so a range or a disabled flag derived from state was
    /// already frozen at first creation. Moving creation to an activation boundary
    /// keeps that behaviour rather than changing it.
    pub(in crate::features) fn ensure_number_input(
        &mut self,
        id: impl Into<SharedString>,
        seed: &str,
        setup: NyaNumberInputOptions,
        cx: &mut Context<Self>,
    ) {
        let _ = self.number_input(id, seed, setup, cx);
    }

    pub(in crate::features) fn ensure_text_input(
        &mut self,
        id: impl Into<SharedString>,
        seed: &str,
        setup: TextInputSetup,
        cx: &mut Context<Self>,
    ) {
        let _ = self.text_input(id, seed, setup, cx);
    }

    pub(in crate::features) fn number_input_box<I: Into<SharedString>>(
        &mut self,
        id: I,
        seed: &str,
        setup: NyaNumberInputOptions,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<I> {
        let id = id.into();
        let palette = self.theme_palette();
        let field = self.number_input(id.clone(), seed, setup, cx);
        number_input_box_from_state(id, palette, field)
    }

    /// Push a value the runtime changed back into its input.
    pub(in crate::features) fn reset_text_input(
        &mut self,
        id: &str,
        text: &str,
        cx: &mut impl AppContext,
    ) {
        if let Some(field) = self.text_inputs.fields.get(id) {
            field.update(cx, |field, cx| field.set_content(text, cx));
        }
        if let Some(field) = self.text_inputs.number_fields.get(id) {
            field.update(cx, |field, cx| field.set_content(text, cx));
        }
    }

    pub(in crate::features) fn reset_number_input(
        &mut self,
        id: &str,
        text: &str,
        cx: &mut impl AppContext,
    ) {
        if let Some(field) = self.text_inputs.number_fields.get(id) {
            field.update(cx, |field, cx| field.set_content(text, cx));
        }
    }

    /// Route an edit to the panel that owns the id.
    ///
    /// Ids are dotted and start with the panel, so a panel claims a whole
    /// prefix: `settings.search-engine.<index>.name`. Anything unclaimed is
    /// ignored rather than panicking — a field can outlive one frame of the
    /// panel that made it.
    fn on_text_input_changed(&mut self, id: SharedString, text: String, cx: &mut Context<Self>) {
        // Remote panels render cached snapshots, so their input subscriptions need the
        // same deferred flush boundary as panel event handlers.
        let remote_panel_input = id.starts_with("remote.");
        if let Some(rest) = id.strip_prefix("settings.number.") {
            self.apply_settings_number_input(rest, &text, cx);
        } else if let Some(rest) = id.strip_prefix("ai.number.") {
            self.apply_ai_number_input(rest, &text, cx);
        } else if let Some(rest) = id.strip_prefix("cloud-sync.number.") {
            self.apply_cloud_sync_number_input(rest, &text, cx);
        } else if let Some(rest) = id.strip_prefix("appearance.number.") {
            self.apply_appearance_number_input(rest, &text, cx);
        } else if let Some(rest) = id.strip_prefix("session.number.") {
            self.apply_session_number_input(rest, &text, cx);
        } else if let Some(rest) = id.strip_prefix("settings.search-engine.") {
            self.apply_search_engine_input(rest, text, cx);
        } else if let Some(field) = id.strip_prefix("network.tunnel-editor.") {
            self.apply_network_tunnel_editor_input(field, text, cx);
        } else if let Some(field) = id.strip_prefix("network.proxy-editor.") {
            self.apply_network_proxy_editor_input(field, text, cx);
        } else if id.as_ref() == "network.group-editor.name" {
            self.apply_network_group_editor_name(text, cx);
        } else if id.as_ref() == "transfer.new-folder.name" {
            self.apply_transfer_new_folder_name(text, cx);
        } else if id.as_ref() == "transfer.new-file.name" {
            self.apply_transfer_new_file_name(text, cx);
        } else if id.as_ref() == "transfer.new-symlink.name" {
            self.apply_transfer_new_symlink_input(
                crate::models::TransferSymlinkField::Name,
                text,
                cx,
            );
        } else if id.as_ref() == "transfer.new-symlink.target" {
            self.apply_transfer_new_symlink_input(
                crate::models::TransferSymlinkField::Target,
                text,
                cx,
            );
        } else if id.as_ref() == "transfer.move.path" {
            self.apply_transfer_move_path(text, cx);
        } else if id.as_ref() == "transfer.browser.path" {
            self.apply_transfer_browser_path_input(text, cx);
        } else if id.as_ref() == "transfer.browser.search" {
            self.apply_transfer_browser_search_input(text, cx);
        } else if let Some(field) = id.strip_prefix("transfer.properties.") {
            self.apply_transfer_properties_input(field, text, cx);
        } else if id.starts_with("transfer.rename.") {
            self.apply_transfer_rename_input(text, cx);
        } else if let Some(field) = id.strip_prefix("quick-command.editor.") {
            if field == "new-category" {
                self.apply_quick_command_editor_new_category_input(text, cx);
            } else {
                self.apply_quick_command_editor_input(field, text, cx);
            }
        } else if let Some(index) = id
            .strip_prefix("quick-command.variable.")
            .and_then(|index| index.parse::<usize>().ok())
        {
            self.apply_quick_command_variable(index, text, cx);
        } else if id.as_ref() == "quick-command.category-rename" {
            self.apply_quick_command_category_rename(text, cx);
        } else if id.as_ref() == "quick-command.category-create" {
            self.apply_quick_command_category_create(text, cx);
        } else if id.as_ref() == "send-command.draft" {
            self.apply_send_command_draft(text, cx);
        } else if let Some(control) = id.strip_prefix("send-command.") {
            self.apply_send_command_control_input(control, text, cx);
        } else if let Some(field) = id.strip_prefix("security.editor.") {
            self.apply_security_editor_input(field, text, cx);
        } else if id.as_ref() == "ai.chat.prompt" {
            self.apply_ai_prompt(text, cx);
        } else if id.as_ref() == "ai.model-search" {
            self.apply_ai_model_search(text, cx);
        } else if id.as_ref() == "ai.history-search" {
            self.apply_ai_history_search(text, cx);
        } else if id.as_ref() == "ai.settings.model-search" {
            self.apply_ai_settings_model_search(text, cx);
        } else if id.as_ref() == "quick-command.search" {
            self.apply_quick_command_search(text, cx);
        } else if id.as_ref() == "quick-command.ai-prompt" {
            self.apply_quick_command_ai_prompt(text, cx);
        } else if id.as_ref() == "recording.search" {
            self.apply_recording_search(text, cx);
        } else if id.as_ref() == "settings.keybindings.search" {
            self.apply_keybinding_search(text, cx);
        } else if id.as_ref() == "sync.groups.search" {
            self.apply_sync_groups_search(text, cx);
        } else if let Some(group_id) = id.strip_prefix("sync.group-name.") {
            self.apply_sync_group_name(group_id, text, cx);
        } else if id.as_ref() == "temporary-ssh.link" {
            self.apply_temporary_ssh_link(text, cx);
        } else if id.as_ref() == "temporary-ssh.serial-port" {
            self.apply_temporary_serial_port_name(text, cx);
        } else if id.as_ref() == "temporary-ssh.baud-rate" {
            self.apply_temporary_serial_baud_rate(text, cx);
        } else if let Some(field) = id.strip_prefix("session.") {
            self.apply_session_text_input(field, text, cx);
        } else if id.as_ref() == "lock-screen.password" {
            self.apply_lock_password_input(text, cx);
        } else if id.as_ref() == "security.unlock.password" {
            self.apply_security_unlock_password_input(text, cx);
        } else if let Some(prompt_id) = id.strip_prefix("ssh.credential.") {
            self.apply_ssh_credential_input(prompt_id, text, cx);
        } else if let Some(field_id) = id.strip_prefix("ssh.keyboard-interactive.") {
            self.apply_keyboard_interactive_input(field_id, text, cx);
        } else if id.as_ref() == "snapshot-password.value" {
            self.apply_snapshot_password_input(text, cx);
        } else if id.as_ref() == "terminal.search.query" {
            self.apply_terminal_search_query(text, cx);
        } else if let Some(field_id) = id.strip_prefix("keyword.highlight.") {
            self.apply_keyword_highlight_input(field_id, text, cx);
        } else if let Some(field_id) = id.strip_prefix("ai.settings.action.") {
            self.apply_ai_action_input(field_id, text, cx);
        } else if let Some(group_key) = id.strip_prefix("ai.settings.manual-model.") {
            self.apply_ai_manual_model_input(group_key, text, cx);
        } else if let Some(rest) = id.strip_prefix("ai.credential.") {
            self.apply_ai_credential_input(rest, text, cx);
        } else if let Some(field) = id
            .strip_prefix("ai.input.")
            .and_then(crate::models::AiInputField::from_input_key)
        {
            self.apply_ai_input(field, text, cx);
        } else if let Some(field) = id
            .strip_prefix("translation.input.")
            .and_then(crate::models::TranslateInputField::from_input_key)
        {
            self.apply_translate_input(field, text, cx);
        } else if let Some(field) = id
            .strip_prefix("cloud-sync.input.")
            .and_then(crate::models::CloudSyncInputField::from_input_key)
        {
            self.apply_cloud_sync_input(field, text, cx);
        } else if id.as_ref() == "sessions.filter" {
            self.apply_active_sessions_search(text, cx);
        } else if id.as_ref() == "remote.docker.filter" {
            self.apply_docker_search(text, cx);
        } else if id.as_ref() == "remote.process.filter" {
            self.apply_process_search(text, cx);
        } else if id.as_ref() == "remote.gpu.filter" {
            self.apply_gpu_search(text, cx);
        } else if id.as_ref() == "remote.npu.filter" {
            self.apply_npu_search(text, cx);
        } else if id.starts_with("remote.process.") && id.ends_with(".nice") {
            self.apply_process_nice_input(text, cx);
        } else if id.as_ref() == "settings.interaction.word-separators" {
            self.apply_interaction_word_separators(text, cx);
        } else if id.as_ref() == "settings.terminal.x11-display" {
            self.apply_terminal_x11_display(text, cx);
        } else if id.as_ref() == "settings.terminal.timestamp-format" {
            self.apply_terminal_timestamp_format(text, cx);
        } else if id.as_ref() == "settings.security.master-password" {
            self.apply_settings_master_password(text, cx);
        } else if id.as_ref() == "settings.recording.path" {
            self.apply_recording_path(text, cx);
        } else if id.as_ref() == "settings.recording.path-template" {
            self.apply_recording_path_template(text, cx);
        } else if id.as_ref() == "settings.transfer.download-path" {
            self.apply_transfer_download_path(text, cx);
        } else if id.as_ref() == "settings.transfer.default-editor" {
            self.apply_transfer_default_editor(text, cx);
        } else if id.as_ref() == "settings.transfer.default-permissions" {
            self.apply_transfer_file_permissions(text, cx);
        }
        if remote_panel_input {
            self.defer_remote_panel_snapshot_flush(cx);
        }
    }

    fn apply_settings_number_input(&mut self, id: &str, text: &str, cx: &mut Context<Self>) {
        let Some(value) = parse_u32_input(text) else {
            return;
        };
        match id {
            "command-suggestion-min-chars" => self.set_command_suggestion_min_chars(value, cx),
            "command-suggestion-max-chars" => self.set_command_suggestion_max_chars(value, cx),
            "duplicate-session-command-delay" => {
                self.set_duplicate_session_command_delay(value, cx)
            }
            "idle-lock-minutes" => self.set_idle_lock_minutes(value, cx),
            "terminal-scrollback-lines" => self.set_terminal_scrollback_lines(value, cx),
            "terminal-keep-alive-interval" => self.set_terminal_keep_alive_interval(value, cx),
            "remote-stats-interval" => self.set_remote_stats_interval(value, cx),
            "gpu-monitor-interval" => self.set_gpu_monitor_interval(value, cx),
            "ascend-npu-monitor-interval" => self.set_ascend_npu_monitor_interval(value, cx),
            "process-manager-interval" => self.set_process_manager_interval(value, cx),
            "docker-manager-interval" => self.set_docker_manager_interval(value, cx),
            "recording-memory-limit" => self.set_recording_memory_limit(value as u64, cx),
            "recording-rotation-size" => self.set_recording_rotation_size_mib(value as u64, cx),
            "transfer-download-threads" => self.set_transfer_download_threads(value, cx),
            "transfer-upload-threads" => self.set_transfer_upload_threads(value, cx),
            "transfer-max-retries" => self.set_transfer_max_retries(value, cx),
            "transfer-buffer-size" => self.set_transfer_buffer_size(value, cx),
            _ => {}
        }
    }

    fn apply_ai_number_input(&mut self, id: &str, text: &str, cx: &mut Context<Self>) {
        let Some(value) = parse_u64_input(text) else {
            return;
        };
        match id {
            "context-line-limit" => self.set_ai_context_line_limit(value as u32, cx),
            "timeout-ms" => self.set_ai_timeout_ms(value, cx),
            "agent-steps" => self.set_ai_agent_steps(value as u16, cx),
            "agent-step-timeout-ms" => self.set_ai_agent_step_timeout_ms(value, cx),
            "terminal-output-lines" => self.set_ai_terminal_output_lines(value as u16, cx),
            "file-size-mb" => self.set_ai_file_size_mb(value, cx),
            _ => {}
        }
    }

    fn apply_cloud_sync_number_input(&mut self, id: &str, text: &str, cx: &mut Context<Self>) {
        let Some(value) = parse_u64_input(text) else {
            return;
        };
        if id == "debounce" {
            self.set_cloud_sync_debounce(value, cx);
        }
    }

    fn apply_appearance_number_input(&mut self, id: &str, text: &str, cx: &mut Context<Self>) {
        let Some(value) = parse_u16_input(text) else {
            return;
        };
        match id {
            "terminal-font-size" => self.set_terminal_font_size_from_input(value, cx),
            "ui-font-size" => self.set_ui_font_size_from_input(value, cx),
            _ => {}
        }
    }

    fn apply_session_number_input(&mut self, id: &str, text: &str, cx: &mut Context<Self>) {
        let Some(value) = parse_u64_input(text) else {
            return;
        };
        if id == "startup-delay" {
            self.set_startup_command_delay(value, cx);
        }
    }

    /// Drop every input whose id starts with `prefix`.
    ///
    /// Called when the thing being edited closes, so reopening it seeds fresh
    /// values rather than showing what was typed into the previous one.
    pub(in crate::features) fn forget_text_inputs(&mut self, prefix: &str) {
        self.text_inputs
            .fields
            .retain(|id, _| !id.starts_with(prefix));
        self.text_inputs
            .subscriptions
            .retain(|id, _| !id.starts_with(prefix));
        self.text_inputs
            .number_fields
            .retain(|id, _| !id.starts_with(prefix));
        self.text_inputs
            .number_subscriptions
            .retain(|id, _| !id.starts_with(prefix));
    }
}

fn parse_u16_input(text: &str) -> Option<u16> {
    text.trim().parse::<u16>().ok()
}

fn parse_u32_input(text: &str) -> Option<u32> {
    text.trim().parse::<u32>().ok()
}

fn parse_u64_input(text: &str) -> Option<u64> {
    text.trim().parse::<u64>().ok()
}

/// The number-input box, built from a state entity the caller already holds.
///
/// Split out of `number_input_box` so a panel rendering from a snapshot can build the box
/// without reaching for the app -- the app creates the entity at flush time and hands the
/// handle over.
pub(in crate::features) fn number_input_box_from_state(
    id: SharedString,
    palette: crate::theme::ThemePalette,
    field: Entity<NyaNumberInputState>,
) -> impl IntoElement {
    div()
        .id(id)
        .w(px(ORDINARY_NUMBER_INPUT_WIDTH_PX))
        .max_w_full()
        .h(px(NYA_FORM_CONTROL_HEIGHT_PX))
        .min_w_0()
        .flex()
        .items_center()
        .child(
            div()
                .min_w_0()
                .flex_1()
                .text_xs()
                .text_color(rgb(palette.text))
                .child(NyaNumberInput::new(&field)),
        )
}

#[cfg(test)]
mod tests {
    use super::ORDINARY_NUMBER_INPUT_WIDTH_PX;

    #[test]
    fn number_input_box_uses_stable_nonzero_width() {
        const { assert!(ORDINARY_NUMBER_INPUT_WIDTH_PX >= 120.) };
    }
}
