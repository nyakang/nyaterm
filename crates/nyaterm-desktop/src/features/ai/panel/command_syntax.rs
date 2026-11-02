use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::Arc;

use gpui::{Context, HighlightStyle, IntoElement, StyledText};

use crate::features::ai::panel::{AiPanel, AiPanelSnapshot};
use crate::theme::ThemePalette;

type CommandHighlights = Arc<[(Range<usize>, HighlightStyle)]>;

#[derive(Default)]
pub(super) struct CommandSyntaxCache {
    palette: Option<ThemePalette>,
    highlights: HashMap<String, CommandHighlights>,
}

impl AiPanel {
    pub(super) fn refresh_command_syntax(
        &mut self,
        snapshot: &AiPanelSnapshot,
        cx: &mut Context<Self>,
    ) {
        let palette = snapshot.chrome.palette;
        if self.snapshot.as_ref().is_some_and(|previous| {
            previous.chrome.palette == palette
                && previous.messages.len() == snapshot.messages.len()
                && previous
                    .messages
                    .iter()
                    .zip(snapshot.messages.iter())
                    .all(|(a, b)| Arc::ptr_eq(a, b) || a.command_cards == b.command_cards)
                && previous.command_cards == snapshot.command_cards
                && previous.native_approvals == snapshot.native_approvals
                && previous.agent_steps.len() == snapshot.agent_steps.len()
                && previous
                    .agent_steps
                    .iter()
                    .zip(snapshot.agent_steps.iter())
                    .all(|(a, b)| a.step.command == b.step.command)
        }) {
            return;
        }
        let commands: HashSet<String> = snapshot
            .messages
            .iter()
            .flat_map(|message| &message.command_cards)
            .chain(snapshot.command_cards.iter())
            .map(|card| card.command.clone())
            .chain(
                snapshot
                    .native_approvals
                    .iter()
                    .filter_map(|view| view.command.clone()),
            )
            .chain(
                snapshot
                    .agent_steps
                    .iter()
                    .filter_map(|step| step.step.command.clone()),
            )
            .collect();
        if self.command_syntax.palette != Some(palette) {
            self.command_syntax.highlights.clear();
            self.command_syntax.palette = Some(palette);
        }
        self.command_syntax
            .highlights
            .retain(|command, _| commands.contains(command));
        let missing: Vec<_> = commands
            .into_iter()
            .filter(|command| !self.command_syntax.highlights.contains_key(command))
            .collect();
        if missing.is_empty() {
            return;
        }
        for command in &missing {
            // An empty entry also marks work in flight during streaming updates.
            self.command_syntax
                .highlights
                .insert(command.clone(), Arc::from([]));
        }
        let work = cx.background_executor().spawn(async move {
            missing
                .into_iter()
                .map(|command| {
                    let highlights =
                        nyaterm_ui::document_syntax::command_highlights(&command, palette);
                    (command, CommandHighlights::from(highlights))
                })
                .collect::<Vec<_>>()
        });
        cx.spawn(async move |panel, cx| {
            let highlights = work.await;
            let _ = panel.update(cx, |panel, cx| {
                if panel.command_syntax.palette != Some(palette) {
                    return;
                }
                for (command, highlights) in highlights {
                    if let Some(entry) = panel.command_syntax.highlights.get_mut(&command) {
                        *entry = highlights;
                    }
                }
                // Only color changes: text metrics and reading anchors stay valid.
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn highlighted_command(&self, command: &str) -> gpui::AnyElement {
        let highlights = self.command_syntax.highlights.get(command);
        StyledText::new(command.to_string())
            .with_highlights(
                highlights
                    .into_iter()
                    .flat_map(|styles| styles.iter().cloned()),
            )
            .into_any_element()
    }
}
