use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui::Context;
use nyaterm_core::ai::AiMessage;

use super::texts::TextOwner;
use super::transcript::AiTranscriptRow;
use super::{AiPanel, AiPanelSnapshot};
use crate::features::formatting::extract_think_content;

pub(super) struct PreparedMessage {
    pub display: String,
    pub reasoning: Option<String>,
}

impl PreparedMessage {
    fn parse(message: &AiMessage) -> Self {
        let (display, embedded) = if message.role == nyaterm_core::AiMessageRole::User {
            (message.content.clone(), None)
        } else {
            extract_think_content(&message.content)
        };
        let reasoning = message
            .reasoning_content
            .as_deref()
            .filter(|text| !text.trim().is_empty())
            .map(str::to_string)
            .or(embedded);
        Self { display, reasoning }
    }
}

struct CachedMessage {
    source: Arc<AiMessage>,
    prepared: Arc<PreparedMessage>,
}

#[derive(Default)]
pub(super) struct ContentCache {
    context: Option<(String, Option<String>, Option<String>)>,
    generation: u64,
    in_flight: bool,
    messages: HashMap<String, CachedMessage>,
    #[cfg(test)]
    pub parses: usize,
}

fn same_source(a: &Arc<AiMessage>, b: &Arc<AiMessage>) -> bool {
    Arc::ptr_eq(a, b)
        || (a.content == b.content
            && a.reasoning_content == b.reasoning_content
            && a.role == b.role)
}

impl AiPanel {
    pub(super) fn refresh_content(&mut self, snapshot: &AiPanelSnapshot, cx: &mut Context<Self>) {
        let context = (
            snapshot.current_ai_session_id.clone(),
            snapshot.owner_terminal_id.clone(),
            snapshot.owner_connection_id.clone(),
        );
        let cache = &mut self.content;
        if cache.context.as_ref() != Some(&context) {
            cache.context = Some(context);
            cache.generation = cache.generation.wrapping_add(1);
            cache.in_flight = false;
            cache.messages.clear();
        }
        let ids: HashSet<_> = snapshot
            .messages
            .iter()
            .map(|message| message.id.as_str())
            .collect();
        cache.messages.retain(|id, _| ids.contains(id.as_str()));
        for message in snapshot.messages.iter() {
            if let Some(ready) = cache.messages.get(&message.id) {
                let owner = TextOwner::Message(message.id.clone());
                let answer_replaced = !message.content.starts_with(&ready.source.content)
                    || message.role != ready.source.role;
                let reasoning_replaced = !message
                    .reasoning_content
                    .as_deref()
                    .unwrap_or_default()
                    .starts_with(
                        ready
                            .source
                            .reasoning_content
                            .as_deref()
                            .unwrap_or_default(),
                    )
                    || answer_replaced;
                for (purpose, replaced) in
                    [("answer", answer_replaced), ("thought", reasoning_replaced)]
                {
                    if replaced
                        && let Some(entry) =
                            self.texts.entries.get(&(owner.clone(), purpose.into()))
                    {
                        entry.state.invalidate_selection(cx);
                    }
                }
            }
        }
        if cache.in_flight {
            return;
        }
        let missing: Vec<_> = snapshot
            .messages
            .iter()
            .filter(|message| {
                !cache
                    .messages
                    .get(&message.id)
                    .is_some_and(|ready| same_source(&ready.source, message))
            })
            .cloned()
            .collect();
        if missing.is_empty() {
            return;
        }
        let generation = cache.generation;
        cache.in_flight = true;
        let work = cx.background_executor().spawn(async move {
            missing
                .into_iter()
                .map(|source| {
                    let prepared = Arc::new(PreparedMessage::parse(&source));
                    (source, prepared)
                })
                .collect::<Vec<_>>()
        });
        cx.spawn(async move |panel, cx| {
            let messages = work.await;
            let _ = panel.update(cx, |panel, cx| {
                if panel.content.generation != generation { return; }
                panel.content.in_flight = false;
                let Some(snapshot) = panel.snapshot.clone() else { return; };
                let desired: HashMap<_, _> = snapshot.messages.iter().map(|message| (message.id.as_str(), message)).collect();
                let mut changed = HashSet::new();
                #[cfg(test)] { panel.content.parses += messages.len(); }
                for (source, prepared) in messages {
                    if desired.get(source.id.as_str()).is_some_and(|latest| same_source(&source, latest)) {
                        changed.insert(source.id.clone());
                        panel.content.messages.insert(source.id.clone(), CachedMessage { source, prepared });
                    }
                }
                // Message preprocessing may add or remove a reasoning disclosure;
                // the renderer's later parse observer measures only its own rows.
                panel.transcript_scroll.update(cx, |scroll, cx| {
                    for (index, row) in panel.transcript_rows.iter().enumerate() {
                        if matches!(row, AiTranscriptRow::Message { id, .. } | AiTranscriptRow::ActivityMessage { id, .. } | AiTranscriptRow::FinalMessage { id, .. } if changed.contains(id)) { scroll.remeasure_items(index..index + 1, cx); }
                    }
                });
                panel.refresh_texts(&snapshot, cx);
                panel.refresh_content(&snapshot, cx);
                if !changed.is_empty() { cx.notify(); }
            });
        }).detach();
    }

    pub(super) fn prepared_message(&self, id: &str) -> Option<&PreparedMessage> {
        self.content
            .messages
            .get(id)
            .map(|entry| entry.prepared.as_ref())
    }
}
