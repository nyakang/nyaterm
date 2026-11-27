use super::{
    SendCommandComposerState, SendCommandFeatureState, SendCommandOptionsState,
    SendCommandProgressResult, SendCommandProgressState, SendCommandTab, SendCommandTabId,
};
use crate::send_command::{
    SendCommandControlFocus, SendCommandDataType, SendCommandLineEnding, SendCommandMode,
    SendCommandTarget,
};
use gpui::App;
use gpui::AppContext as _;
use nyaterm_core::command_draft::DraftProvenance;
use nyaterm_core::hex_document::{HexCopyFormat, format_hex};
use nyaterm_core::send_command::TextSendStatistics;
use nyaterm_ui::hex_editor::NyaHexEditorState;

impl SendCommandTab {
    pub(super) fn new(id: SendCommandTabId, cx: &mut App) -> Self {
        Self {
            id,
            progress: SendCommandProgressState::default(),
            name: id.0.to_string(),
            composer: SendCommandComposerState {
                text_draft: String::new(),
                text_statistics: TextSendStatistics::default(),
                provenance: DraftProvenance::default(),
                hex: cx.new(NyaHexEditorState::new),
            },
            options: SendCommandOptionsState {
                data_type: SendCommandDataType::Text,
                text_mode: SendCommandMode::Line,
                hex_mode: SendCommandMode::Byte,
                line_ending: SendCommandLineEnding::Crlf,
                target: SendCommandTarget::Current,
                count: Some(1),
                count_input: "1".to_string(),
                interval_seconds: 1.0,
                interval_input: "1.00".to_string(),
            },
        }
    }
}

impl SendCommandFeatureState {
    pub(super) fn active(&self) -> &SendCommandTab {
        self.tabs
            .iter()
            .find(|tab| tab.id == self.active_tab)
            .expect("active send tab")
    }
    pub(super) fn active_mut(&mut self) -> &mut SendCommandTab {
        self.tabs
            .iter_mut()
            .find(|tab| tab.id == self.active_tab)
            .expect("active send tab")
    }
    pub(in crate::features) fn active_id(&self) -> SendCommandTabId {
        self.active_tab
    }
    pub(in crate::features) fn input_id(&self, control: &str) -> String {
        self.active_tab.input_id(control)
    }
    pub(in crate::features) fn active_is_sending(&self) -> bool {
        self.active().progress.sending
    }
    pub(in crate::features) fn tab_is_sending(&self, id: SendCommandTabId) -> bool {
        self.tabs
            .iter()
            .any(|tab| tab.id == id && tab.progress.sending)
    }
    pub(in crate::features) fn matches_run(&self, tab: SendCommandTabId, run: u64) -> bool {
        self.tabs.iter().any(|entry| {
            entry.id == tab && entry.progress.sending && entry.progress.run_id == Some(run)
        })
    }
    pub(in crate::features) fn tab_labels(&self) -> Vec<(SendCommandTabId, String)> {
        self.tabs
            .iter()
            .map(|tab| (tab.id, tab.name.clone()))
            .collect()
    }
    pub(in crate::features) fn tab_name(&self, id: SendCommandTabId) -> Option<&str> {
        self.tabs
            .iter()
            .find(|tab| tab.id == id)
            .map(|tab| tab.name.as_str())
    }
    pub(in crate::features) fn tab_text_draft(&self, id: SendCommandTabId) -> Option<&str> {
        self.tabs
            .iter()
            .find(|tab| tab.id == id)
            .map(|tab| tab.composer.text_draft.as_str())
    }
    pub(in crate::features) fn switch_tab(&mut self, id: SendCommandTabId) -> bool {
        if self.tab_name(id).is_none() {
            return false;
        }
        self.target_picker_open = false;
        self.active_tab = id;
        self.reveal_active_tab = true;
        true
    }
    pub(in crate::features) fn add_tab(&mut self, cx: &mut App) -> SendCommandTabId {
        let id = SendCommandTabId(self.next_tab_number);
        self.next_tab_number = self
            .next_tab_number
            .checked_add(1)
            .expect("send tab IDs exhausted");
        self.tabs.push(SendCommandTab::new(id, cx));
        self.switch_tab(id);
        id
    }
    pub(in crate::features) fn rename_tab(&mut self, id: SendCommandTabId, name: &str) {
        if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id) {
            tab.name = if name.trim().is_empty() {
                id.0.to_string()
            } else {
                name.trim().to_owned()
            };
            self.reveal_active_tab = true;
        }
    }
    pub(in crate::features) fn duplicate_tab(
        &mut self,
        id: SendCommandTabId,
        cx: &mut App,
    ) -> Option<SendCommandTabId> {
        let source = self.tabs.iter().find(|tab| tab.id == id)?;
        let text = source.composer.text_draft.clone();
        let provenance = source.composer.provenance.clone();
        let document = source.composer.hex.read(cx).document().clone();
        let mut options = source.options.clone();
        options.apply_count_input(false);
        options.count_input = options.count_label();
        if nyaterm_core::send_command::parse_interval_ms(&options.interval_input).is_none() {
            options.interval_seconds = 1.0;
        }
        options.sync_interval_input();
        let new_id = self.add_tab(cx);
        let tab = self.active_mut();
        tab.options = options;
        tab.composer.text_statistics = TextSendStatistics::from_text(&text);
        tab.composer.text_draft = text;
        tab.composer.provenance = provenance;
        tab.composer
            .hex
            .update(cx, |hex, cx| hex.replace_document(document, cx));
        Some(new_id)
    }
    pub(in crate::features) fn tab_has_draft(&self, id: SendCommandTabId, cx: &App) -> bool {
        self.tabs
            .iter()
            .find(|tab| tab.id == id)
            .is_some_and(|tab| {
                let hex = tab.composer.hex.read(cx).document();
                !tab.composer.text_draft.is_empty()
                    || !hex.bytes().is_empty()
                    || hex.pending_nibble().is_some()
            })
    }
    pub(in crate::features) fn can_close_tabs(&self, ids: &[SendCommandTabId]) -> bool {
        !ids.is_empty()
            && self.tabs.iter().any(|tab| !ids.contains(&tab.id))
            && !self
                .tabs
                .iter()
                .any(|tab| tab.progress.sending && ids.contains(&tab.id))
    }
    pub(in crate::features) fn close_tabs(&mut self, ids: &[SendCommandTabId]) -> bool {
        if !self.can_close_tabs(ids) {
            return false;
        }
        if ids.contains(&self.active_tab) {
            let index = self
                .tabs
                .iter()
                .position(|tab| tab.id == self.active_tab)
                .unwrap();
            self.active_tab = self.tabs[index + 1..]
                .iter()
                .chain(self.tabs[..index].iter().rev())
                .find(|tab| !ids.contains(&tab.id))
                .expect("one send tab remains")
                .id;
        }
        self.tabs.retain(|tab| !ids.contains(&tab.id));
        self.target_picker_open = false;
        self.reveal_active_tab = true;
        true
    }
    pub(in crate::features) fn apply_tab_draft(
        &mut self,
        id: SendCommandTabId,
        text: String,
    ) -> bool {
        let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id) else {
            return false;
        };
        if tab.composer.text_draft != text {
            tab.composer.provenance.edit(text.is_empty());
        }
        tab.composer.text_statistics = TextSendStatistics::from_text(&text);
        tab.composer.text_draft = text;
        true
    }
    pub(in crate::features) fn apply_tab_control(
        &mut self,
        id: SendCommandTabId,
        control: SendCommandControlFocus,
        text: String,
    ) -> Option<String> {
        let running = self.tab_is_sending(id);
        let tab = self.tabs.iter_mut().find(|tab| tab.id == id)?;
        match control {
            SendCommandControlFocus::Count => {
                if !running {
                    tab.options.count_input = text;
                    tab.options.apply_count_input(true);
                }
                Some(tab.options.count_input.clone())
            }
            SendCommandControlFocus::Interval => {
                if !running {
                    tab.options.interval_input = text;
                    tab.options.apply_interval_input(true);
                }
                Some(tab.options.interval_input.clone())
            }
        }
    }
    pub(in crate::features) fn clear_sent_tab(
        &mut self,
        id: SendCommandTabId,
        run: u64,
        enabled: bool,
        successful: bool,
        sent: &str,
        cx: &mut App,
    ) -> bool {
        if !enabled || !successful || !self.matches_run(id, run) {
            return false;
        }
        let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id) else {
            return false;
        };
        match tab.options.data_type {
            SendCommandDataType::Text => {
                if tab.composer.text_draft != sent {
                    return false;
                }
                tab.composer.text_draft.clear();
                tab.composer.text_statistics = TextSendStatistics::default();
                tab.composer.provenance = DraftProvenance::default();
            }
            SendCommandDataType::Hex => {
                let document = tab.composer.hex.read(cx).document();
                if document.pending_nibble().is_some()
                    || format_hex(document.bytes(), HexCopyFormat::Hex) != sent
                {
                    return false;
                }
                tab.composer.hex.update(cx, |hex, cx| hex.clear(cx));
            }
        }
        true
    }
    pub(in crate::features) fn progress_result(
        &self,
        tab: SendCommandTabId,
        run: u64,
    ) -> Option<SendCommandProgressResult> {
        let progress = &self
            .tabs
            .iter()
            .find(|entry| {
                entry.id == tab && entry.progress.sending && entry.progress.run_id == Some(run)
            })?
            .progress;
        Some(SendCommandProgressResult {
            completed: progress.completed,
            total: progress.total,
            round: progress.round,
        })
    }
}
