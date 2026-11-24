use super::helpers::{available_translation_providers, search_engine_url};
use super::quick_highlight::{
    CUSTOM_HIGHLIGHT_SWATCH_INDEX, VISIBLE_QUICK_COLOR_COUNT, quick_color, quick_color_tooltip,
};
use crate::features::terminal::state::TerminalContextWord;
use crate::features::{NyaTermApp, session::SerialUploadProtocol};
use crate::models::{AiPreparedRequest, NavItem, SettingsTab};
use gpui::Context;
use nyaterm_core::{AiAction, AiContext};
use nyaterm_recording::recording::{RecordingMode, RecordingStatus};
use nyaterm_transport::SessionKind;
use nyaterm_ui::NyaMenuItem;
use nyaterm_ui::icons::known_search_engine_icon;
use rust_i18n::t;

/// Parse a `#rrggbb` swatch into the packed RGB value the menu renderer uses.
fn swatch_rgb(hex: &str) -> u32 {
    let digits = hex.trim().trim_start_matches('#');
    u32::from_str_radix(digits, 16).unwrap_or(0xffffff)
}

impl NyaTermApp {
    /// Drop the transient word resolved from the previous right-click. Called
    /// before every terminal right-click so a mouse-report or non-menu action
    /// cannot leave stale highlight text behind.
    pub(in crate::features) fn clear_terminal_context_word(&mut self) {
        self.terminal.menus.context_word = None;
    }

    pub(in crate::features) fn prepare_terminal_context_menu(
        &mut self,
        session_id: &str,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.clear_terminal_context_word();
        if !self.terminal_has_selection_for_session(session_id)
            && let Some(text) =
                self.terminal_word_text_at_pointer_for_session(session_id, position, cx)
        {
            self.terminal.menus.context_word = Some(TerminalContextWord {
                session_id: session_id.to_string(),
                text,
            });
        }
        self.terminal.menus.action_link_menu = None;
        self.terminal.menus.action_link_tooltip = None;
        self.terminal.assist.commands.popup = None;
        self.terminal.assist.credential_suggestions = None;
        self.shell
            .set_status("terminal context menu opened".to_string());
        cx.notify();
    }

    pub(in crate::features) fn terminal_context_menu_items_for_session(
        &mut self,
        session_id: String,
        cx: &mut Context<Self>,
    ) -> Vec<NyaMenuItem> {
        if self.terminal_has_selection_for_session(&session_id) {
            self.ensure_terminal_selection_text(cx);
        }
        let selection_session_id = self
            .terminal
            .selection
            .session_id
            .as_deref()
            .or(self.session.active_id());
        let selected = (selection_session_id == Some(session_id.as_str()))
            .then(|| self.selected_terminal_text())
            .flatten()
            .unwrap_or_default();
        // The right-click word is a highlight-only fallback. A real selection
        // always wins, including a pending one whose text has not arrived yet.
        let highlight_text =
            if self.terminal_has_selection_for_session(&session_id) || !selected.is_empty() {
                selected.clone()
            } else {
                self.terminal
                    .menus
                    .context_word
                    .as_ref()
                    .filter(|word| word.session_id == session_id)
                    .map(|word| word.text.clone())
                    .unwrap_or_default()
            };
        self.terminal_context_menu_items_with_highlight_text(
            session_id,
            selected,
            highlight_text,
            cx,
        )
    }

    #[cfg(test)]
    pub(in crate::features) fn terminal_context_menu_items(
        &mut self,
        session_id: String,
        selected: String,
        cx: &mut Context<Self>,
    ) -> Vec<NyaMenuItem> {
        self.terminal_context_menu_items_with_highlight_text(
            session_id,
            selected.clone(),
            selected,
            cx,
        )
    }

    pub(in crate::features) fn terminal_context_menu_items_with_highlight_text(
        &mut self,
        session_id: String,
        selected: String,
        highlight_text: String,
        cx: &mut Context<Self>,
    ) -> Vec<NyaMenuItem> {
        let owns_selection = self.terminal_has_selection_for_session(&session_id);
        let has_selection = !selected.is_empty() || owns_selection;
        let text_unavailable = selected.is_empty();
        let selection_pending = owns_selection
            && matches!(
                self.terminal.selection.text.status,
                crate::features::terminal::state::TerminalSelectionTextStatus::Pending
            );
        let copy_request = selection_pending.then(|| {
            (
                self.terminal
                    .selection
                    .text
                    .key
                    .clone()
                    .expect("pending selection key"),
                self.terminal.selection.text.generation,
            )
        });
        let shortcut = |id: &str, fallback: &str| self.display_shortcut_for(id, fallback);
        let copy_sc = shortcut("terminal.copy", "Ctrl+Shift+C");
        let paste_sc = shortcut("terminal.paste", "Ctrl+Shift+V");
        let paste_sel_sc = shortcut("terminal.pasteSelected", "Ctrl+Shift+X");
        let find_sc = shortcut("terminal.find", "Ctrl+Shift+F");
        let clear_sc = shortcut("terminal.clear", "Ctrl+L");
        let select_all_sc = shortcut("terminal.selectAll", "Ctrl+Shift+A");
        let local_mode_sc = shortcut("terminal.localMode", "Ctrl+Enter");
        let recording_sc = shortcut("terminal.recording.toggle", "Ctrl+Shift+R");
        let mut items = Vec::new();

        if has_selection {
            let selected_for_copy = selected.clone();
            items.push(
                NyaMenuItem::action(t!("terminalCtx.copy"))
                    .icon("icons/copy.svg")
                    .shortcut(copy_sc)
                    .disabled(text_unavailable && !selection_pending)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some((key, generation)) = copy_request.as_ref() {
                            this.copy_terminal_selection_request(key, *generation, cx);
                            return;
                        }
                        this.copy_text_with_feedback(
                            "copy-terminal-selection",
                            selected_for_copy.clone(),
                            cx,
                        );
                        this.shell
                            .set_status("copied terminal selection".to_string());
                        cx.notify();
                    })),
            );
            let find_session_id = session_id.clone();
            items.push(
                NyaMenuItem::action(t!("terminalCtx.find"))
                    .icon("icons/fe/search.svg")
                    .shortcut(find_sc)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.activate_workspace_pane(find_session_id.clone(), cx);
                        this.terminal.search.active_index = 0;
                        this.open_terminal_search(window, cx);
                    })),
            );
            let selected_for_quick_command = selected.clone();
            items.push(
                NyaMenuItem::action(t!("terminalCtx.saveAsQuickCommand"))
                    .icon("icons/save.svg")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_quick_command_editor_with_command(
                            selected_for_quick_command.clone(),
                            window,
                            cx,
                        );
                    })),
            );

            let search_items = self.terminal_online_search_menu_items(&selected, cx);
            items.push(
                NyaMenuItem::submenu(t!("terminalCtx.searchOnline"), search_items)
                    .icon("icons/menu/travel-explore.svg"),
            );

            let ai_items = self.terminal_ai_context_menu_items(&session_id, &selected, cx);
            if !ai_items.is_empty() {
                items.push(NyaMenuItem::submenu(t!("ai.title"), ai_items).icon("icons/ai.svg"));
            }

            let translation_items = self.terminal_translation_menu_items(&selected, cx);
            if !translation_items.is_empty() {
                items.push(
                    NyaMenuItem::submenu(t!("terminalCtx.translate"), translation_items)
                        .icon("icons/translation.svg"),
                );
            }

            if text_unavailable {
                // Keep Copy enabled while pending; it binds to this request.
                for item in items.iter_mut().skip(1) {
                    *item = item.clone().disabled(true);
                }
            }

            let selected_for_paste = selected.clone();
            let paste_session_id = session_id.clone();
            let paste_selected_session_id = session_id.clone();
            items.extend([
                NyaMenuItem::separator(),
                NyaMenuItem::action(t!("terminalCtx.paste"))
                    .icon("icons/menu/paste.svg")
                    .shortcut(paste_sc)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.activate_workspace_pane(paste_session_id.clone(), cx);
                        this.paste_from_clipboard(window, cx);
                    })),
                NyaMenuItem::action(t!("terminalCtx.pasteSelectedText"))
                    .icon("icons/menu/paste-go.svg")
                    .shortcut(paste_sel_sc)
                    .disabled(text_unavailable)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.activate_workspace_pane(paste_selected_session_id.clone(), cx);
                        this.paste_terminal_text(selected_for_paste.clone(), window, cx);
                    })),
            ]);
        } else {
            let paste_session_id = session_id.clone();
            let find_session_id = session_id.clone();
            items.extend([
                NyaMenuItem::action(t!("terminalCtx.paste"))
                    .icon("icons/menu/paste.svg")
                    .shortcut(paste_sc)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.activate_workspace_pane(paste_session_id.clone(), cx);
                        this.paste_from_clipboard(window, cx);
                    })),
                NyaMenuItem::action(t!("terminalCtx.find"))
                    .icon("icons/fe/search.svg")
                    .shortcut(find_sc)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.activate_workspace_pane(find_session_id.clone(), cx);
                        this.open_terminal_search(window, cx);
                    })),
            ]);
        }

        let recording_items = self.terminal_recording_menu_items(&session_id, recording_sc, cx);
        let serial_upload = self
            .session
            .ordered_sessions()
            .into_iter()
            .find(|session| session.id == session_id)
            .is_some_and(|session| session.kind == SessionKind::Serial);
        if serial_upload {
            let x_session = session_id.clone();
            let y_session = session_id.clone();
            let z_session = session_id.clone();
            items.extend([
                NyaMenuItem::separator(),
                NyaMenuItem::submenu(
                    t!("terminalCtx.serialUpload"),
                    vec![
                        NyaMenuItem::action("XMODEM").on_click(cx.listener(
                            move |this, _, _, cx| {
                                this.prompt_serial_upload_files(
                                    x_session.clone(),
                                    SerialUploadProtocol::Xmodem,
                                    cx,
                                );
                            },
                        )),
                        NyaMenuItem::action("YMODEM").on_click(cx.listener(
                            move |this, _, _, cx| {
                                this.prompt_serial_upload_files(
                                    y_session.clone(),
                                    SerialUploadProtocol::Ymodem,
                                    cx,
                                );
                            },
                        )),
                        NyaMenuItem::action("ZMODEM").on_click(cx.listener(
                            move |this, _, _, cx| {
                                this.prompt_serial_upload_files(
                                    z_session.clone(),
                                    SerialUploadProtocol::Zmodem,
                                    cx,
                                );
                            },
                        )),
                    ],
                )
                .icon("icons/fe/upload.svg"),
            ]);
        }
        let clear_screen_session_id = session_id.clone();
        let clear_all_session_id = session_id.clone();
        let select_all_session_id = session_id.clone();
        items.extend([
            NyaMenuItem::separator(),
            NyaMenuItem::action(t!("terminalCtx.clearScreen"))
                .icon("icons/menu/clear-all.svg")
                .shortcut(clear_sc)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.activate_workspace_pane(clear_screen_session_id.clone(), cx);
                    this.send_terminal_clear_screen(cx);
                })),
            NyaMenuItem::action(t!("terminalCtx.clearAll"))
                .icon("icons/menu/delete-sweep.svg")
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.activate_workspace_pane(clear_all_session_id.clone(), cx);
                    this.clear_terminal(cx);
                })),
            NyaMenuItem::separator(),
            NyaMenuItem::submenu(t!("terminalCtx.recordingLogs"), recording_items)
                .icon("icons/session/record.svg"),
            NyaMenuItem::separator(),
        ]);

        // Quick session colors work on the highlight text: a real selection,
        // or the word resolved from the right-click position when no selection
        // exists. The bar sits directly above the full highlight editor action.
        if has_selection || !highlight_text.is_empty() {
            let checked = self.selected_quick_highlight_color(&session_id, &highlight_text);
            let highlight_unavailable = highlight_text.is_empty();
            let mut swatches = Vec::new();

            // A custom-colored rule gets a checked gray indicator in the first
            // slot. It is not a paintable quick color: clicking it removes the
            // rule instead of applying gray to the selection.
            if self
                .selected_custom_highlight_rule(&session_id, &highlight_text)
                .is_some()
            {
                let color = quick_color(CUSTOM_HIGHLIGHT_SWATCH_INDEX)
                    .expect("custom color slot stays in the preset table");
                let swatch_session = session_id.clone();
                let swatch_text = highlight_text.clone();
                swatches.push(
                    NyaMenuItem::action(quick_color_tooltip(CUSTOM_HIGHLIGHT_SWATCH_INDEX, true))
                        .icon_color(swatch_rgb(color.background))
                        .swatch_text_color(swatch_rgb(color.light))
                        .checked(true)
                        .disabled(highlight_unavailable)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.remove_quick_highlight_color(&swatch_session, &swatch_text, cx);
                        })),
                );
            }

            for index in 0..VISIBLE_QUICK_COLOR_COUNT {
                let color = quick_color(index).expect("index from visible quick color range");
                let swatch_session = session_id.clone();
                let swatch_text = highlight_text.clone();
                let swatch_checked = checked == Some(index);
                swatches.push(
                    NyaMenuItem::action(quick_color_tooltip(index, swatch_checked))
                        .icon_color(swatch_rgb(color.background))
                        .swatch_text_color(swatch_rgb(color.light))
                        .checked(swatch_checked)
                        .disabled(highlight_unavailable)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            // Clicking the applied color toggles it off;
                            // any other color recolors the same rule.
                            if swatch_checked {
                                this.remove_quick_highlight_color(
                                    &swatch_session,
                                    &swatch_text,
                                    cx,
                                );
                            } else {
                                this.apply_quick_highlight_color(
                                    &swatch_session,
                                    &swatch_text,
                                    index,
                                    cx,
                                );
                            }
                        })),
                );
            }
            items.push(NyaMenuItem::color_swatch_bar(swatches));
        }

        items.extend([
            NyaMenuItem::action(t!("terminalCtx.keywordHighlight"))
                .icon("icons/menu/palette.svg")
                .on_click(cx.listener({
                    let popup_session_id = select_all_session_id.clone();
                    let popup_selected = highlight_text.clone();
                    move |this, _, _, cx| {
                        this.open_keyword_highlight_popup(
                            popup_session_id.clone(),
                            popup_selected.clone(),
                            cx,
                        );
                    }
                })),
            NyaMenuItem::action(t!("settings.shortcutLabels.localMode"))
                .icon("icons/mouse.svg")
                .shortcut(local_mode_sc)
                .on_click(cx.listener({
                    let local_mode_session_id = select_all_session_id.clone();
                    move |this, _, _, cx| {
                        this.activate_workspace_pane(local_mode_session_id.clone(), cx);
                        this.toggle_terminal_local_mode(cx);
                    }
                })),
            NyaMenuItem::separator(),
            NyaMenuItem::action(t!("terminalCtx.selectAll"))
                .icon("icons/menu/select-all.svg")
                .shortcut(select_all_sc)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.activate_workspace_pane(select_all_session_id.clone(), cx);
                    this.select_all_terminal(cx);
                })),
        ]);
        items
    }

    fn terminal_recording_menu_items(
        &self,
        session_id: &str,
        shortcut: String,
        cx: &mut Context<Self>,
    ) -> Vec<NyaMenuItem> {
        let recording_status = self.recording.status(session_id);
        self.terminal_recording_menu_items_for_status(session_id, shortcut, recording_status, cx)
    }

    fn terminal_recording_menu_items_for_status(
        &self,
        session_id: &str,
        shortcut: String,
        recording_status: Option<RecordingStatus>,
        cx: &mut Context<Self>,
    ) -> Vec<NyaMenuItem> {
        let mut items = if let Some(status) = recording_status {
            let stop_session_id = session_id.to_string();
            let open_path = status.file_path.clone();
            let reveal_path = status.file_path;
            vec![
                NyaMenuItem::action(t!("recording.stop"))
                    .icon("icons/session/stop.svg")
                    .shortcut(shortcut)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.stop_recording_for_session(&stop_session_id, cx);
                    })),
                NyaMenuItem::action(t!("recording.openLog"))
                    .icon("icons/file/description.svg")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let Some(path) = open_path.as_ref() else {
                            return;
                        };
                        cx.open_with_system(path);
                        this.shell.set_status(t!("recording.opened").to_string());
                    })),
                NyaMenuItem::action(t!("recording.showInFolder"))
                    .icon("icons/session/folder-open.svg")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let Some(path) = reveal_path.as_ref() else {
                            return;
                        };
                        cx.reveal_path(path);
                        this.shell
                            .set_status(t!("recording.folderRevealed").to_string());
                    })),
            ]
        } else {
            let transcript_session_id = session_id.to_string();
            let raw_session_id = session_id.to_string();
            vec![
                NyaMenuItem::action(t!("recording.startTranscriptLog"))
                    .icon("icons/file/description.svg")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.start_recording_for_session(
                            &transcript_session_id,
                            RecordingMode::Transcript,
                            cx,
                        );
                    })),
                NyaMenuItem::action(t!("recording.startRawLog"))
                    .icon("icons/session/record.svg")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.start_recording_for_session(&raw_session_id, RecordingMode::Raw, cx);
                    })),
            ]
        };
        let save_session_id = session_id.to_string();
        items.extend([
            NyaMenuItem::separator(),
            NyaMenuItem::action(t!("recording.saveTranscript"))
                .icon("icons/file/description.svg")
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.save_session_transcript_for_session(&save_session_id, cx);
                })),
            NyaMenuItem::action(t!("terminalCtx.recordingSettings"))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.shell
                        .set_settings_active_tab(SettingsTab::TerminalGeneral);
                    this.open_page(NavItem::Settings, cx);
                }))
                .icon("icons/settings.svg"),
        ]);
        items
    }

    fn terminal_online_search_menu_items(
        &mut self,
        selected: &str,
        cx: &mut Context<Self>,
    ) -> Vec<NyaMenuItem> {
        self.settings
            .summary()
            .search_custom_engines
            .iter()
            .filter(|engine| engine.show_in_menu)
            .cloned()
            .map(|engine| {
                let query = selected.to_string();
                let name = engine.name.clone();
                let status_name = name.clone();
                let template = engine.url_template;
                let icon = known_search_engine_icon(engine.icon.as_deref());
                let item =
                    NyaMenuItem::action(name).on_click(cx.listener(move |this, _, _, cx| {
                        let url = search_engine_url(&template, &query);
                        this.open_external_url_for_ui(&url, cx);
                        this.shell
                            .set_status(format!("opened online search: {status_name}"));
                    }));
                match icon {
                    Some((path, Some(color))) => item.icon(path).icon_color(color),
                    Some((path, None)) => item.icon(path),
                    None => item,
                }
            })
            .collect()
    }

    fn terminal_ai_context_menu_items(
        &mut self,
        session_id: &str,
        selected: &str,
        cx: &mut Context<Self>,
    ) -> Vec<NyaMenuItem> {
        if !self.ai.settings_config().enabled {
            return Vec::new();
        }
        self.ai
            .settings_config()
            .terminal_ai_actions
            .iter()
            .filter(|action| action.enabled && !action.name.trim().is_empty())
            .cloned()
            .map(|action| {
                let selected = selected.to_string();
                let session_id = session_id.to_string();
                let name = action.name.clone();
                let status_name = name.clone();
                let prompt = action.prompt;
                NyaMenuItem::action(name).on_click(cx.listener(move |this, _, window, cx| {
                    this.activate_workspace_pane(session_id.clone(), cx);
                    let context = this.ai_terminal_context_for_session(Some(&session_id));
                    let request = terminal_ai_prepared_request(
                        context,
                        selected.clone(),
                        status_name.clone(),
                    );
                    this.set_ai_prompt_draft(prompt.clone(), cx);
                    this.ai.prepare_external_request(
                        request,
                        format!("Starting AI action: {status_name}"),
                        format!("AI action started: {status_name}"),
                        false,
                    );
                    this.ensure_panel_open(NavItem::AiAssistant, cx);
                    window.focus(this.ai.chat_focus(), cx);
                    this.start_ai_ask(cx);
                }))
            })
            .collect()
    }

    fn terminal_translation_menu_items(
        &mut self,
        selected: &str,
        cx: &mut Context<Self>,
    ) -> Vec<NyaMenuItem> {
        available_translation_providers(self.translation.settings())
            .into_iter()
            .map(|(id, _)| {
                let label = t!(match id.as_str() {
                    "google" => "translation.google",
                    "microsoft" => "translation.microsoft",
                    "deepl" => "translation.deepl",
                    "baidu" => "translation.baidu",
                    "ali" => "translation.ali",
                    "youdao" => "translation.youdao",
                    _ => "translation.provider",
                })
                .to_string();
                let selected = selected.to_string();
                let provider_id = id.clone();
                let provider_label = label.clone();
                NyaMenuItem::action(label).on_click(cx.listener(move |this, _, window, cx| {
                    this.open_translation_dialog(
                        selected.clone(),
                        provider_id.clone(),
                        provider_label.clone(),
                        window,
                        cx,
                    );
                }))
            })
            .collect()
    }
}

fn terminal_ai_prepared_request(
    mut context: AiContext,
    selected_text: String,
    source_label: String,
) -> AiPreparedRequest {
    context.selected_text = selected_text;
    AiPreparedRequest {
        action: AiAction::CustomTerminalAction,
        context,
        source_label,
    }
}

#[cfg(test)]
mod tests {
    use super::terminal_ai_prepared_request;
    use crate::entities::{OverlayStore, StartupRestoreStore, UiStoreHandles};
    use crate::features::NyaTermApp;
    use crate::features::test_support::app_with_visible_local_session;
    use crate::features::translation::TranslationFeatureState;
    use gpui::{AppContext as _, TestAppContext, px};
    use nyaterm_core::test_support::TestTempDir;
    use nyaterm_core::{
        AiAction, AiContext, AiCustomActionConfig, AiSettings, AppRuntime, RuntimeMode,
        SearchEngineConfig, TranslationSettings, uuid,
    };
    use nyaterm_recording::recording::{RecordingMode, RecordingStatus, RecordingStatusState};
    use nyaterm_ui::NyaMenuItem;
    use std::path::PathBuf;

    fn unique_test_dir(label: &str) -> PathBuf {
        // A uuid rather than a clock reading: these tests run in parallel and
        // Windows' ~15ms clock granularity lets a nanosecond timestamp repeat,
        // which would share one config dir and so one settings database.
        std::env::temp_dir().join(format!(
            "nyaterm-terminal-menu-{label}-{}-{}",
            std::process::id(),
            uuid()
        ))
    }

    fn menu_app(cx: &mut TestAppContext) -> gpui::Entity<NyaTermApp> {
        let root = unique_test_dir("app");
        let runtime = AppRuntime::from_parts_for_test(
            RuntimeMode::Portable,
            root.clone(),
            root.join("config"),
            root.join("logs"),
            root.join("cache"),
            None,
        );
        let stores = UiStoreHandles {
            startup_restore: cx.new(|_| StartupRestoreStore::default()),
            overlays: cx.new(|_| OverlayStore::default()),
        };
        cx.new(|cx| NyaTermApp::new(runtime, stores, cx))
    }

    fn labels(items: &[NyaMenuItem]) -> Vec<&str> {
        items.iter().map(NyaMenuItem::test_label).collect()
    }

    fn item<'a>(items: &'a [NyaMenuItem], label: &str) -> &'a NyaMenuItem {
        items
            .iter()
            .find(|item| item.test_label() == label)
            .unwrap_or_else(|| panic!("missing menu item {label}"))
    }

    #[test]
    fn menu_tree_matches_tauri_for_selection_and_empty_selection() {
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = menu_app(&mut cx);
        let (selected, empty) = cx.update_entity(&app, |app, cx| {
            let ai = AiSettings {
                enabled: false,
                ..AiSettings::default()
            };
            app.ai.replace_settings_config(ai, true);
            let mut summary = app.settings.summary().clone();
            summary.search_custom_engines = vec![
                SearchEngineConfig {
                    name: "Visible".to_string(),
                    url_template: String::new(),
                    icon: Some("google".to_string()),
                    show_in_menu: true,
                },
                SearchEngineConfig {
                    name: "Hidden".to_string(),
                    url_template: "https://hidden.test/%s".to_string(),
                    icon: Some("github".to_string()),
                    show_in_menu: false,
                },
            ];
            app.settings.replace_summary(summary);
            (
                app.terminal_context_menu_items(
                    "clicked-pane".to_string(),
                    "selected text".to_string(),
                    cx,
                ),
                app.terminal_context_menu_items("clicked-pane".to_string(), String::new(), cx),
            )
        });

        assert_eq!(
            labels(&selected),
            [
                "Copy",
                "Find...",
                "Save as Quick Command",
                "Search Web",
                "Translate",
                "",
                "Paste",
                "Paste Selected Text",
                "",
                "Clear",
                "Clear All",
                "",
                "Recording Logs",
                "",
                "",
                "Keyword Highlight...",
                "Toggle local mode",
                "",
                "Select All",
            ]
        );
        assert_eq!(
            labels(&empty),
            [
                "Paste",
                "Find...",
                "",
                "Clear",
                "Clear All",
                "",
                "Recording Logs",
                "",
                "Keyword Highlight...",
                "Toggle local mode",
                "",
                "Select All",
            ]
        );
        assert!(!labels(&selected).contains(&"Open Link"));
        assert!(!labels(&selected).contains(&"More Actions..."));

        // The selection menu carries the nine paintable quick colors; the
        // reserved custom indicator stays hidden until a custom rule matches.
        let bar = selected
            .iter()
            .find_map(NyaMenuItem::test_color_swatch_items)
            .expect("quick color swatch bar");
        assert_eq!(bar.len(), 9);
        assert_eq!(bar[0].test_label(), "Add Red marker");
        assert_eq!(bar[8].test_label(), "Add Magenta marker");
        assert!(bar.iter().all(|swatch| swatch.test_icon_color().is_some()));
        // Each dot is filled with its background color and reveals the
        // matching text color as the "+" on hover.
        assert_eq!(bar[0].test_icon_color(), Some(0xb3_26_1e));
        assert_eq!(bar[0].test_swatch_text_color(), Some(0xff_f0_ee));
        assert_eq!(bar[8].test_icon_color(), Some(0xa3_22_74));
        assert_eq!(bar[8].test_swatch_text_color(), Some(0xff_e6_f5));
        assert!(
            bar.iter()
                .all(|swatch| swatch.test_swatch_text_color().is_some())
        );
        assert!(
            empty
                .iter()
                .all(|item| item.test_color_swatch_items().is_none())
        );

        let search = item(&selected, "Search Web")
            .children()
            .expect("search submenu");
        assert_eq!(labels(search), ["Visible"]);
        assert_eq!(search[0].test_icon_color(), Some(0x4285f4));
        assert_eq!(
            search[0].test_presentation().1.as_deref(),
            Some("icons/brand/google.svg")
        );

        let recording = item(&selected, "Recording Logs")
            .children()
            .expect("recording submenu");
        assert_eq!(
            labels(recording),
            [
                "Start Transcript Log",
                "Start Raw Log",
                "",
                "Save Text",
                "Settings...",
            ]
        );
        assert_eq!(
            recording
                .iter()
                .map(NyaMenuItem::test_presentation)
                .map(|(_, icon, shortcut, _, _, _)| (icon, shortcut))
                .collect::<Vec<_>>(),
            vec![
                (Some("icons/file/description.svg".to_string()), None),
                (Some("icons/session/record.svg".to_string()), None),
                (None, None),
                (Some("icons/file/description.svg".to_string()), None),
                (Some("icons/settings.svg".to_string()), None),
            ]
        );
    }

    #[test]
    fn clicking_one_swatch_adds_exactly_one_rule() {
        use crate::features::test_support::session_focus_test_window;
        use gpui::ClickEvent;

        let root = TestTempDir::new("nyaterm-swatch-click");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, root.path(), "local");

        // Build a menu with a real selection and grab the first swatch handler.
        let handler = cx.update_entity(&app, |app, cx| {
            let items = app.terminal_context_menu_items("local".into(), "ERROR".into(), cx);
            let bar = items
                .iter()
                .find_map(NyaMenuItem::test_color_swatch_items)
                .expect("swatch bar");
            bar[0].test_on_click().cloned().expect("swatch click")
        });

        // Invoke it twice: the swatch renderer registers both an inner
        // Button on_click and an outer on_action(Confirm) with the same
        // handler, so a single physical click may dispatch it more than once.
        let vcx = session_focus_test_window(&mut cx, &app);
        vcx.update(|window, cx| {
            handler(&ClickEvent::default(), window, cx);
            handler(&ClickEvent::default(), window, cx);
        });

        app.read_with(&cx, |app, _| {
            let rules = app.session_keyword_rules("local").unwrap();
            assert_eq!(
                rules.len(),
                1,
                "clicking one swatch must never stack two rules, got {rules:?}"
            );
        });
    }

    #[test]
    fn clicking_checked_swatch_removes_its_rule() {
        use crate::features::test_support::session_focus_test_window;
        use gpui::ClickEvent;

        let root = TestTempDir::new("nyaterm-swatch-remove");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, root.path(), "local");

        // Apply color 0 to the selection first, then rebuild the menu so the
        // first swatch is in its checked ("−") state.
        cx.update_entity(&app, |app, cx| {
            app.apply_quick_highlight_color("local", "ERROR", 0, cx);
        });
        let (handler, is_checked) = cx.update_entity(&app, |app, cx| {
            let items = app.terminal_context_menu_items("local".into(), "ERROR".into(), cx);
            let bar = items
                .iter()
                .find_map(NyaMenuItem::test_color_swatch_items)
                .expect("swatch bar");
            let first = &bar[0];
            (
                first.test_on_click().cloned().expect("swatch click"),
                first.test_checked(),
            )
        });
        assert!(
            is_checked,
            "first swatch should be checked after applying it"
        );

        // Invoke twice to prove the remove path is idempotent under the
        // renderer's double dispatch.
        let vcx = session_focus_test_window(&mut cx, &app);
        vcx.update(|window, cx| {
            handler(&ClickEvent::default(), window, cx);
            handler(&ClickEvent::default(), window, cx);
        });

        app.read_with(&cx, |app, _| {
            assert!(
                app.session_keyword_rules("local").unwrap().is_empty(),
                "clicking an applied swatch should remove its rule"
            );
        });
    }

    #[test]
    fn custom_color_rule_shows_checked_indicator_first_and_removes_it() {
        use crate::features::test_support::session_focus_test_window;
        use gpui::ClickEvent;
        use nyaterm_core::KeywordHighlightRule;

        let root = TestTempDir::new("nyaterm-swatch-custom");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, root.path(), "local");

        cx.update_entity(&app, |app, cx| {
            app.set_session_keyword_rules(
                "local",
                vec![KeywordHighlightRule {
                    id: "kh-custom".into(),
                    name: "ERROR".into(),
                    patterns: vec![regex::escape("ERROR")],
                    color_dark: "#123456".into(),
                    color_light: "#654321".into(),
                    enabled: true,
                }],
                cx,
            );
        });

        let (label, checked, color, handler) = cx.update_entity(&app, |app, cx| {
            let items = app.terminal_context_menu_items("local".into(), "ERROR".into(), cx);
            let bar = items
                .iter()
                .find_map(NyaMenuItem::test_color_swatch_items)
                .expect("swatch bar");
            assert_eq!(bar.len(), 10, "custom indicator plus nine quick colors");
            let first = &bar[0];
            (
                first.test_label().to_string(),
                first.test_checked(),
                first.test_icon_color(),
                first.test_on_click().cloned().expect("swatch click"),
            )
        });
        assert_eq!(label, "Remove Custom color marker");
        assert!(checked, "custom indicator must show as checked");
        assert_eq!(color, Some(0x4c_55_60));

        let vcx = session_focus_test_window(&mut cx, &app);
        vcx.update(|window, cx| {
            handler(&ClickEvent::default(), window, cx);
            handler(&ClickEvent::default(), window, cx);
        });
        app.read_with(&cx, |app, _| {
            assert!(
                app.session_keyword_rules("local").unwrap().is_empty(),
                "clicking the custom indicator removes its rule"
            );
        });

        cx.update_entity(&app, |app, cx| {
            let items = app.terminal_context_menu_items("local".into(), "other".into(), cx);
            let bar = items
                .iter()
                .find_map(NyaMenuItem::test_color_swatch_items)
                .expect("swatch bar");
            assert_eq!(bar.len(), 9, "unrelated text has no custom indicator");
        });
    }

    #[test]
    fn right_click_word_only_feeds_highlight_entries() {
        use crate::features::terminal::state::TerminalContextWord;
        use crate::features::test_support::session_focus_test_window;
        use gpui::ClickEvent;

        let root = TestTempDir::new("nyaterm-right-click-word");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, root.path(), "local");

        let word_position = cx.update_entity(&app, |app, cx| {
            app.terminal.layout.surface_bounds = Some(gpui::Bounds {
                origin: gpui::Point::default(),
                size: gpui::Size {
                    width: px(800.),
                    height: px(600.),
                },
            });
            app.terminal
                .seed_session_view("local".into(), "target word".into(), "UTF-8");
            let geometry = app
                .terminal_hit_test_geometry_for_session(Some("local"), cx)
                .expect("terminal geometry");
            gpui::Point {
                x: px(f32::from(geometry.bounds.origin.x)
                    + geometry.padding_left
                    + geometry.gutter
                    + geometry.cell_w * 0.5),
                y: px(f32::from(geometry.bounds.origin.y)
                    + geometry.padding_top
                    + geometry.visual_y_offset
                    + geometry.cell_h * 0.5),
            }
        });

        cx.update_entity(&app, |app, cx| {
            assert_eq!(
                app.terminal_word_text_at_pointer_for_session("local", word_position, cx)
                    .as_deref(),
                Some("target")
            );
            app.prepare_terminal_context_menu("local", word_position, cx);
            assert_eq!(
                app.terminal
                    .menus
                    .context_word
                    .as_ref()
                    .map(|word| word.text.as_str()),
                Some("target")
            );
        });

        let (items, handler) = cx.update_entity(&app, |app, cx| {
            let items = app.terminal_context_menu_items_for_session("local".into(), cx);
            let bar = items
                .iter()
                .find_map(NyaMenuItem::test_color_swatch_items)
                .expect("right-click word should reveal the swatch bar");
            let handler = bar[0].test_on_click().cloned().expect("swatch click");
            (items, handler)
        });

        // The word is highlight-only: no real selection was created.
        assert!(!labels(&items).contains(&"Copy"));
        assert!(!labels(&items).contains(&"Save as Quick Command"));
        app.read_with(&cx, |app, _| {
            assert!(app.terminal.selection.selection.is_none());
        });

        let vcx = session_focus_test_window(&mut cx, &app);
        vcx.update(|window, cx| {
            handler(&ClickEvent::default(), window, cx);
        });
        app.read_with(&cx, |app, _| {
            let rules = app.session_keyword_rules("local").unwrap();
            assert_eq!(rules.len(), 1);
            assert_eq!(rules[0].patterns, vec![regex::escape("target")]);
        });

        cx.update_entity(&app, |app, cx| {
            app.clear_terminal_context_word();
            let cleared = app.terminal_context_menu_items_for_session("local".into(), cx);
            assert!(
                cleared
                    .iter()
                    .all(|item| item.test_color_swatch_items().is_none())
            );
            // A word resolved for another session must not leak into this menu.
            app.terminal.menus.context_word = Some(TerminalContextWord {
                session_id: "other".into(),
                text: "foreign".into(),
            });
            let other = app.terminal_context_menu_items_for_session("local".into(), cx);
            assert!(
                other
                    .iter()
                    .all(|item| item.test_color_swatch_items().is_none())
            );
        });
    }

    #[test]
    fn right_click_inside_word_returns_the_full_word_like_double_click() {
        let root = TestTempDir::new("nyaterm-right-click-word-middle");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, root.path(), "local");

        // "100 deny ip any any any any": click the `e` in `deny`. Double-click
        // selects `deny`; the right-click fallback must not return only `eny`.
        let inside_deny = cx.update_entity(&app, |app, cx| {
            app.terminal.layout.surface_bounds = Some(gpui::Bounds {
                origin: gpui::Point::default(),
                size: gpui::Size {
                    width: px(800.),
                    height: px(600.),
                },
            });
            app.terminal.seed_session_view(
                "local".into(),
                "100 deny ip any any any any".into(),
                "UTF-8",
            );
            let geometry = app
                .terminal_hit_test_geometry_for_session(Some("local"), cx)
                .expect("terminal geometry");
            gpui::Point {
                x: px(f32::from(geometry.bounds.origin.x)
                    + geometry.padding_left
                    + geometry.gutter
                    + geometry.cell_w * 5.5),
                y: px(f32::from(geometry.bounds.origin.y)
                    + geometry.padding_top
                    + geometry.visual_y_offset
                    + geometry.cell_h * 0.5),
            }
        });

        cx.update_entity(&app, |app, cx| {
            assert_eq!(
                app.terminal_word_text_at_pointer_for_session("local", inside_deny, cx)
                    .as_deref(),
                Some("deny")
            );
            app.prepare_terminal_context_menu("local", inside_deny, cx);
            assert_eq!(
                app.terminal
                    .menus
                    .context_word
                    .as_ref()
                    .map(|word| word.text.as_str()),
                Some("deny")
            );
        });
    }

    #[test]
    fn right_click_on_blank_output_does_not_create_highlight_text() {
        let root = TestTempDir::new("nyaterm-right-click-blank");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, root.path(), "local");

        let blank_position = cx.update_entity(&app, |app, cx| {
            app.terminal.layout.surface_bounds = Some(gpui::Bounds {
                origin: gpui::Point::default(),
                size: gpui::Size {
                    width: px(800.),
                    height: px(600.),
                },
            });
            app.terminal
                .seed_session_view("local".into(), "word\r\n            ".into(), "UTF-8");
            let geometry = app
                .terminal_hit_test_geometry_for_session(Some("local"), cx)
                .expect("terminal geometry");
            gpui::Point {
                x: px(f32::from(geometry.bounds.origin.x)
                    + geometry.padding_left
                    + geometry.gutter
                    + geometry.cell_w * 0.5),
                y: px(f32::from(geometry.bounds.origin.y)
                    + geometry.padding_top
                    + geometry.visual_y_offset
                    + geometry.cell_h * 1.5),
            }
        });

        cx.update_entity(&app, |app, cx| {
            assert_eq!(
                app.terminal_word_text_at_pointer_for_session("local", blank_position, cx),
                None
            );
            app.prepare_terminal_context_menu("local", blank_position, cx);
            assert!(app.terminal.menus.context_word.is_none());
            let items = app.terminal_context_menu_items_for_session("local".into(), cx);
            assert!(
                items
                    .iter()
                    .all(|item| item.test_color_swatch_items().is_none())
            );
        });
    }

    #[test]
    fn select_all_uses_worker_scrollback_and_reopens_the_selection_menu() {
        let root = TestTempDir::new("nyaterm-select-all-menu");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, root.path(), "selected-pane");
        cx.update_entity(&app, |app, cx| {
            let empty = app.terminal_context_menu_items_for_session("selected-pane".into(), cx);
            assert!(!labels(&empty).contains(&"Copy"));
            let pipeline = &app.terminal.view.frame_pipeline;
            pipeline.seed_session("selected-pane", "", "UTF-8", 1000);
            pipeline.resize_session("selected-pane", 5, 2);
            pipeline.submit_decoded_output("selected-pane".into(), "abcdef\r\nghij".into());
            // The UI-side parser stays blank, as it does for real worker frames.
            assert_eq!(
                app.terminal.view.views["selected-pane"].screen.all_text(),
                None
            );
            app.select_all_terminal(cx);
            let pending = app.terminal_context_menu_items_for_session("selected-pane".into(), cx);
            assert!(labels(&pending).contains(&"Copy"));
            assert!(!item(&pending, "Copy").test_presentation().3);
            app.terminal.view.frame_pipeline.flush_for_test();
        });
        cx.run_until_parked();
        cx.update_entity(&app, |app, cx| {
            assert_eq!(
                app.selected_terminal_text().as_deref(),
                Some("abcdef\nghij")
            );
            let selected = app.terminal_context_menu_items_for_session("selected-pane".into(), cx);
            assert!(labels(&selected).contains(&"Copy"));
            assert!(labels(&selected).contains(&"Save as Quick Command"));
            assert!(!item(&selected, "Copy").test_presentation().3);
            let other = app.terminal_context_menu_items_for_session("other-pane".into(), cx);
            assert!(!labels(&other).contains(&"Copy"));
            app.clear_terminal_selection(cx);
            let cleared = app.terminal_context_menu_items_for_session("selected-pane".into(), cx);
            assert!(!labels(&cleared).contains(&"Copy"));
        });
    }

    #[test]
    fn clearing_select_all_discards_an_in_flight_worker_reply() {
        let root = TestTempDir::new("nyaterm-select-all-cancel");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, root.path(), "selected-pane");
        cx.update_entity(&app, |app, cx| {
            app.terminal
                .view
                .frame_pipeline
                .seed_session("selected-pane", "text", "UTF-8", 1000);
            app.select_all_terminal(cx);
            app.clear_terminal_selection(cx);
            app.terminal.view.frame_pipeline.flush_for_test();
        });
        cx.run_until_parked();
        cx.update_entity(&app, |app, cx| {
            assert!(app.selected_terminal_text().is_none());
            let items = app.terminal_context_menu_items_for_session("selected-pane".into(), cx);
            assert!(!labels(&items).contains(&"Copy"));
        });
    }

    #[test]
    fn optional_ai_and_credential_translation_items_match_tauri() {
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = menu_app(&mut cx);
        let items = cx.update_entity(&app, |app, cx| {
            let ai = AiSettings {
                enabled: true,
                terminal_ai_actions: vec![
                    AiCustomActionConfig {
                        id: "enabled".to_string(),
                        name: "Explain".to_string(),
                        prompt: "Explain this".to_string(),
                        enabled: true,
                    },
                    AiCustomActionConfig {
                        id: "hidden".to_string(),
                        name: "Hidden".to_string(),
                        prompt: "Hidden".to_string(),
                        enabled: false,
                    },
                ],
                ..AiSettings::default()
            };
            app.ai.replace_settings_config(ai, true);
            app.translation = TranslationFeatureState::new(TranslationSettings {
                deepl_api_key: "configured".to_string().into(),
                baidu_app_id: "id".to_string(),
                baidu_app_key: "key".to_string().into(),
                ali_app_id: "id".to_string(),
                ali_app_key: "key".to_string().into(),
                youdao_app_id: "id".to_string(),
                youdao_app_key: "key".to_string().into(),
                ..TranslationSettings::default()
            });
            app.terminal_context_menu_items("clicked-pane".to_string(), "selection".to_string(), cx)
        });

        let ai = item(&items, "AI").children().expect("AI submenu");
        assert_eq!(labels(ai), ["Explain"]);
        assert!(ai[0].test_presentation().1.is_none());

        let translation = item(&items, "Translate")
            .children()
            .expect("translation submenu");
        assert_eq!(
            labels(translation),
            ["Google", "Microsoft", "DeepL", "Baidu", "Alibaba", "Youdao"]
        );
        assert!(
            translation
                .iter()
                .all(|item| item.test_presentation().1.is_none())
        );
    }

    #[test]
    fn ai_request_keeps_full_selection_and_custom_action_metadata() {
        let selected = "x".repeat(8_192);
        let request = terminal_ai_prepared_request(
            AiContext::default(),
            selected.clone(),
            "Explain".to_string(),
        );

        assert_eq!(request.action, AiAction::CustomTerminalAction);
        assert_eq!(request.context.selected_text, selected);
        assert_eq!(request.source_label, "Explain");
    }

    #[test]
    fn recording_submenu_uses_clicked_session_status_and_active_icons() {
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = menu_app(&mut cx);
        let recording_path = unique_test_dir("active").join("recording.log");
        let (clicked_items, other_items) = cx.update_entity(&app, |app, cx| {
            app.recording.apply_status(RecordingStatus {
                session_id: "clicked-pane".to_string(),
                state: RecordingStatusState::Recording,
                mode: RecordingMode::Transcript,
                file_path: Some(recording_path.clone()),
                input_file_path: None,
                started_at: None,
                written_bytes: 0,
                queued_bytes: 0,
                dropped_bytes: 0,
                last_error: None,
            });
            (
                app.terminal_recording_menu_items("clicked-pane", "Ctrl+Shift+R".to_string(), cx),
                app.terminal_recording_menu_items("other-pane", "Ctrl+Shift+R".to_string(), cx),
            )
        });

        assert_eq!(
            labels(&clicked_items),
            [
                "Stop",
                "Open Recording",
                "Show in Folder",
                "",
                "Save Text",
                "Settings...",
            ]
        );
        assert_eq!(
            labels(&other_items),
            [
                "Start Transcript Log",
                "Start Raw Log",
                "",
                "Save Text",
                "Settings...",
            ]
        );
        assert_eq!(
            clicked_items[2].test_presentation().1.as_deref(),
            Some("icons/session/folder-open.svg")
        );

        let stopped_items = cx.update_entity(&app, |app, cx| {
            app.recording.remove_status("clicked-pane");
            app.terminal_recording_menu_items("clicked-pane", "Ctrl+Shift+R".to_string(), cx)
        });
        assert_eq!(labels(&stopped_items)[0], "Start Transcript Log");
        let _ = std::fs::remove_file(recording_path);
    }

    #[test]
    fn active_recording_without_a_path_keeps_open_and_reveal_actions_visible() {
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = menu_app(&mut cx);
        let items = cx.update_entity(&app, |app, cx| {
            app.terminal_recording_menu_items_for_status(
                "clicked-pane",
                "Ctrl+Shift+R".to_string(),
                Some(RecordingStatus {
                    session_id: "clicked-pane".to_string(),
                    state: RecordingStatusState::Starting,
                    mode: RecordingMode::Transcript,
                    file_path: None,
                    input_file_path: None,
                    started_at: None,
                    written_bytes: 0,
                    queued_bytes: 0,
                    dropped_bytes: 0,
                    last_error: None,
                }),
                cx,
            )
        });

        assert_eq!(
            labels(&items),
            [
                "Stop",
                "Open Recording",
                "Show in Folder",
                "",
                "Save Text",
                "Settings...",
            ]
        );
    }
}
