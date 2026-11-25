use crate::features::plugins::view::PluginPanel;
use gpui::{AppContext, Context, Entity};
use nyaterm_core::plugins::extension::{PanelElement, SessionHandle};
use nyaterm_core::plugins::{ErrorCode, PluginError, PluginResult, validate_id};
use nyaterm_ui::NyaInputState;
use std::collections::BTreeMap;

pub(super) struct ToolPanel {
    pub session: Option<SessionHandle>,
    pub revision: u64,
    pub title: String,
    pub elements: Vec<PanelElement>,
    pub inputs: BTreeMap<String, Entity<NyaInputState>>,
}

impl PluginPanel {
    pub(super) fn show_tool_panel(
        &mut self,
        owner: String,
        session: Option<SessionHandle>,
        id: String,
        title: String,
        elements: Vec<PanelElement>,
        cx: &mut Context<Self>,
    ) -> PluginResult<()> {
        validate_id(&id)?;
        let revision = self
            .catalog(cx)
            .plugins
            .iter()
            .find(|plugin| plugin.id == owner && plugin.enabled)
            .map(|plugin| plugin.revision)
            .ok_or_else(|| {
                PluginError::new(ErrorCode::Cancelled, "Extension is no longer active")
            })?;
        if elements.len() > 128
            || title.len() > 160
            || self.tool_panels.len() >= 32
                && !self.tool_panels.contains_key(&(owner.clone(), id.clone()))
        {
            return Err(PluginError::new(
                ErrorCode::PackageLimit,
                "Extension panel exceeds its limits",
            ));
        }
        let bytes: usize = elements
            .iter()
            .map(|element| match element {
                PanelElement::Text { text } | PanelElement::Log { text } => text.len(),
                PanelElement::Input { id, label, value } => id.len() + label.len() + value.len(),
                PanelElement::Button { action, label } => action.len() + label.len(),
                PanelElement::Progress { label, .. } => label.len(),
                PanelElement::Table { columns, rows } => {
                    columns.iter().map(String::len).sum::<usize>()
                        + rows.iter().flatten().map(String::len).sum::<usize>()
                }
            })
            .sum();
        if bytes > nyaterm_core::plugins::MAX_TEXT_BYTES {
            return Err(PluginError::new(
                ErrorCode::PackageLimit,
                "Extension panel text exceeds its limit",
            ));
        }
        let mut ids = std::collections::BTreeSet::new();
        for element in &elements {
            match element {
                PanelElement::Input { id, .. } => {
                    validate_id(id)?;
                    if !ids.insert(id) {
                        return Err(PluginError::new(
                            ErrorCode::Conflict,
                            "Duplicate panel input",
                        ));
                    }
                }
                PanelElement::Button { action, .. } => {
                    validate_id(action)?;
                }
                PanelElement::Table { columns, rows }
                    if columns.len() > 32
                        || rows.len() > 1000
                        || rows.iter().any(|row| row.len() != columns.len()) =>
                {
                    return Err(PluginError::new(
                        ErrorCode::PackageLimit,
                        "Extension table exceeds its limits",
                    ));
                }
                _ => {}
            }
        }
        let mut inputs = self
            .tool_panels
            .remove(&(owner.clone(), id.clone()))
            .map(|panel| panel.inputs)
            .unwrap_or_default();
        inputs.retain(|id, _| ids.contains(id));
        for element in &elements {
            if let PanelElement::Input { id, value, .. } = element {
                inputs.entry(id.clone()).or_insert_with(|| {
                    cx.new(|cx| NyaInputState::new(cx, value.clone()).max_chars(Some(32768)))
                });
            }
        }
        self.tool_panels.insert(
            (owner, id),
            ToolPanel {
                session,
                revision,
                title,
                elements,
                inputs,
            },
        );
        self.discover = false;
        cx.notify();
        Ok(())
    }
}
