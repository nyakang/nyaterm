use serde::{Deserialize, Serialize};

use crate::ai::RiskLevel;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QuickCommandCategory {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub sort_order: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QuickCommand {
    pub id: String,
    pub label: String,
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_tag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_tag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_mode: Option<String>,
    /// Missing, null and false retain the legacy script interpretation.
    #[serde(default, skip_serializing_if = "raw_send_disabled")]
    pub raw_send: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub risk_level: Option<RiskLevel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub use_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort_order: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct QuickCommandsConfig {
    #[serde(default)]
    pub commands: Vec<QuickCommand>,
    #[serde(default)]
    pub categories: Vec<QuickCommandCategory>,
}

fn raw_send_disabled(value: &Option<bool>) -> bool {
    !value.unwrap_or_default()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuickCommandRelativePosition {
    Before,
    After,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuickCommandCategoryPosition {
    Before,
    After,
    Inside,
}

/// One parent's children in the order the sidebar renders them: `sort_order`,
/// then name, then id.
///
/// This is the single authority for that order, so "move up" in the group menu
/// always means "the row above". A `parent_id` that names no existing category is
/// treated as a root, matching how the sidebar recovers from orphaned rows.
pub fn quick_command_category_sibling_order<'a>(
    categories: &'a [QuickCommandCategory],
    parent_id: Option<&str>,
) -> Vec<&'a QuickCommandCategory> {
    let ids = categories
        .iter()
        .map(|item| item.id.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let mut children = categories
        .iter()
        .filter(|category| {
            let effective = category
                .parent_id
                .as_deref()
                .filter(|parent| ids.contains(parent));
            effective == parent_id
        })
        .collect::<Vec<_>>();
    children.sort_by(|left, right| {
        left.sort_order
            .cmp(&right.sort_order)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
            .then_with(|| left.id.cmp(&right.id))
    });
    children
}

/// The sibling a category would swap with when moved up or down, or `None` when it
/// already sits at that end of its run. Drives both the move action and whether the
/// menu item is enabled.
pub fn quick_command_category_move_neighbor(
    categories: &[QuickCommandCategory],
    category_id: &str,
    up: bool,
) -> Option<String> {
    let ids = categories
        .iter()
        .map(|item| item.id.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let category = categories.iter().find(|item| item.id == category_id)?;
    let parent_id = category
        .parent_id
        .as_deref()
        .filter(|parent| ids.contains(parent));
    let siblings = quick_command_category_sibling_order(categories, parent_id);
    let index = siblings.iter().position(|item| item.id == category_id)?;
    let neighbor = if up {
        index.checked_sub(1)?
    } else {
        index.checked_add(1).filter(|next| *next < siblings.len())?
    };
    Some(siblings[neighbor].id.clone())
}

impl QuickCommandsConfig {
    /// Moves a command relative to another command. Pinned and unpinned commands
    /// remain separate visual partitions; dropping across the boundary adopts the
    /// target partition before orders are normalized.
    pub fn reorder_command_relative(
        &mut self,
        source_id: &str,
        target_id: &str,
        position: QuickCommandRelativePosition,
    ) -> bool {
        if source_id == target_id {
            return false;
        }
        let Some(source_index) = self.commands.iter().position(|item| item.id == source_id) else {
            return false;
        };
        let Some(target) = self
            .commands
            .iter()
            .find(|item| item.id == target_id)
            .cloned()
        else {
            return false;
        };
        let mut source = self.commands.remove(source_index);
        source.category_id = target.category_id.clone();
        source.pinned = target.pinned;

        let mut partition = self
            .commands
            .iter()
            .filter(|item| {
                item.category_id == target.category_id
                    && item.pinned.unwrap_or_default() == target.pinned.unwrap_or_default()
            })
            .map(|item| item.id.clone())
            .collect::<Vec<_>>();
        partition.sort_by(|left, right| {
            let left = self
                .commands
                .iter()
                .find(|item| item.id == *left)
                .expect("id");
            let right = self
                .commands
                .iter()
                .find(|item| item.id == *right)
                .expect("id");
            left.sort_order
                .unwrap_or(i32::MAX)
                .cmp(&right.sort_order.unwrap_or(i32::MAX))
                .then_with(|| left.id.cmp(&right.id))
        });
        let Some(target_index) = partition.iter().position(|id| id == target_id) else {
            self.commands.push(source);
            return false;
        };
        let insert_index =
            target_index + usize::from(position == QuickCommandRelativePosition::After);
        partition.insert(insert_index, source.id.clone());
        self.commands.push(source);
        self.normalize_command_partition(
            &target.category_id,
            target.pinned.unwrap_or_default(),
            &partition,
        );
        true
    }

    pub fn move_command_to_category(
        &mut self,
        source_id: &str,
        category_id: Option<String>,
    ) -> bool {
        let Some(source) = self.commands.iter_mut().find(|item| item.id == source_id) else {
            return false;
        };
        if source.category_id == category_id {
            return false;
        }
        source.category_id = category_id.clone();
        let pinned = source.pinned.unwrap_or_default();
        let next = self
            .commands
            .iter()
            .filter(|item| {
                item.id != source_id
                    && item.category_id == category_id
                    && item.pinned.unwrap_or_default() == pinned
            })
            .filter_map(|item| item.sort_order)
            .max()
            .unwrap_or(-1)
            .saturating_add(1);
        if let Some(source) = self.commands.iter_mut().find(|item| item.id == source_id) {
            source.sort_order = Some(next);
        }
        true
    }

    pub fn move_category(
        &mut self,
        source_id: &str,
        target_id: &str,
        position: QuickCommandCategoryPosition,
    ) -> bool {
        if !self.can_move_category(source_id, target_id) {
            return false;
        }
        let target_parent = self
            .categories
            .iter()
            .find(|item| item.id == target_id)
            .and_then(|item| item.parent_id.as_deref())
            .filter(|parent| self.categories.iter().any(|item| item.id == *parent))
            .map(ToString::to_string);
        let new_parent = match position {
            QuickCommandCategoryPosition::Inside => Some(target_id.to_string()),
            QuickCommandCategoryPosition::Before | QuickCommandCategoryPosition::After => {
                target_parent
            }
        };
        let siblings =
            quick_command_category_sibling_order(&self.categories, new_parent.as_deref())
                .into_iter()
                .filter(|item| item.id != source_id)
                .map(|item| item.id.clone())
                .collect::<Vec<_>>();
        let insert_index = match position {
            QuickCommandCategoryPosition::Inside => siblings.len(),
            QuickCommandCategoryPosition::Before | QuickCommandCategoryPosition::After => {
                let Some(index) = siblings.iter().position(|id| id == target_id) else {
                    return false;
                };
                index + usize::from(position == QuickCommandCategoryPosition::After)
            }
        };
        self.move_category_to_parent(source_id, new_parent.as_deref(), insert_index)
    }

    pub fn can_move_category(&self, source_id: &str, target_id: &str) -> bool {
        !(source_id == target_id
            || !self.categories.iter().any(|item| item.id == source_id)
            || !self.categories.iter().any(|item| item.id == target_id)
            || self.category_is_descendant(target_id, source_id))
    }

    /// `insertion_index` counts siblings after excluding the source; an index
    /// beyond the end appends. Validate the whole move before changing the tree.
    pub fn move_category_to_parent(
        &mut self,
        source_id: &str,
        new_parent_id: Option<&str>,
        insertion_index: usize,
    ) -> bool {
        let Some(source) = self.categories.iter().find(|item| item.id == source_id) else {
            return false;
        };
        if new_parent_id.is_some_and(|parent_id| {
            !self.categories.iter().any(|item| item.id == parent_id)
                || self.category_is_descendant(parent_id, source_id)
        }) {
            return false;
        }
        let old_parent = source.parent_id.clone();
        let new_parent = new_parent_id.map(ToString::to_string);
        let original_siblings =
            quick_command_category_sibling_order(&self.categories, new_parent_id)
                .into_iter()
                .map(|item| item.id.clone())
                .collect::<Vec<_>>();
        let mut siblings = original_siblings
            .iter()
            .filter(|id| id.as_str() != source_id)
            .cloned()
            .collect::<Vec<_>>();
        siblings.insert(insertion_index.min(siblings.len()), source_id.to_string());
        if old_parent == new_parent && siblings == original_siblings {
            return false;
        }
        self.categories
            .iter_mut()
            .find(|item| item.id == source_id)
            .expect("validated source")
            .parent_id = new_parent.clone();
        if old_parent != new_parent {
            let old_siblings =
                quick_command_category_sibling_order(&self.categories, old_parent.as_deref())
                    .into_iter()
                    .map(|item| item.id.clone())
                    .collect::<Vec<_>>();
            self.normalize_category_siblings(old_siblings);
        }
        self.normalize_category_siblings(siblings);
        true
    }

    /// Place the whole subtree immediately after its former parent.
    pub fn outdent_category(&mut self, source_id: &str) -> bool {
        let Some(parent_id) = self
            .categories
            .iter()
            .find(|item| item.id == source_id)
            .and_then(|item| item.parent_id.clone())
        else {
            return false;
        };
        self.move_category(source_id, &parent_id, QuickCommandCategoryPosition::After)
    }

    fn normalize_category_siblings(&mut self, siblings: Vec<String>) {
        for (order, id) in siblings.into_iter().enumerate() {
            if let Some(category) = self.categories.iter_mut().find(|item| item.id == id) {
                category.sort_order = i32::try_from(order).unwrap_or(i32::MAX);
            }
        }
    }

    fn category_is_descendant(&self, candidate_id: &str, ancestor_id: &str) -> bool {
        let mut current = Some(candidate_id);
        let mut visited = std::collections::BTreeSet::new();
        while let Some(id) = current {
            if id == ancestor_id {
                return true;
            }
            if !visited.insert(id.to_string()) {
                return false;
            }
            current = self
                .categories
                .iter()
                .find(|item| item.id == id)
                .and_then(|item| item.parent_id.as_deref());
        }
        false
    }

    fn normalize_command_partition(
        &mut self,
        category_id: &Option<String>,
        pinned: bool,
        ordered_ids: &[String],
    ) {
        for (order, id) in ordered_ids.iter().enumerate() {
            if let Some(command) = self.commands.iter_mut().find(|item| {
                item.id == *id
                    && item.category_id == *category_id
                    && item.pinned.unwrap_or_default() == pinned
            }) {
                command.sort_order = Some(i32::try_from(order).unwrap_or(i32::MAX));
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct QuickCommandsExportConfig {
    pub categories: Vec<QuickCommandCategoryExport>,
    pub commands: Vec<QuickCommandExport>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct QuickCommandCategoryExport {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    pub sort_order: i32,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct QuickCommandExport {
    pub id: String,
    pub label: String,
    pub command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color_tag: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon_tag: Option<String>,
    pub pinned: bool,
    pub execution_mode: String,
    #[serde(skip_serializing_if = "raw_send_disabled")]
    pub raw_send: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub risk_level: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort_order: Option<i32>,
}

impl From<QuickCommandsConfig> for QuickCommandsExportConfig {
    fn from(config: QuickCommandsConfig) -> Self {
        Self {
            categories: config
                .categories
                .into_iter()
                .map(|category| QuickCommandCategoryExport {
                    id: category.id,
                    name: category.name,
                    parent_id: category.parent_id,
                    sort_order: category.sort_order,
                })
                .collect(),
            commands: config
                .commands
                .into_iter()
                .map(|command| QuickCommandExport {
                    id: command.id,
                    label: command.label,
                    command: command.command,
                    category_id: command.category_id,
                    description: command.description,
                    color_tag: command.color_tag,
                    icon_tag: command.icon_tag,
                    pinned: command.pinned.unwrap_or_default(),
                    execution_mode: command
                        .execution_mode
                        .unwrap_or_else(|| "execute".to_string()),
                    raw_send: command.raw_send,
                    source: command.source,
                    risk_level: command
                        .risk_level
                        .map(|risk| quick_command_export_risk_label(&risk).to_string()),
                    sort_order: command.sort_order,
                })
                .collect(),
        }
    }
}

pub fn export_quick_commands_json(
    config: QuickCommandsConfig,
) -> Result<String, serde_json::Error> {
    serde_json::to_string_pretty(&QuickCommandsExportConfig::from(config))
}

fn quick_command_export_risk_label(risk: &RiskLevel) -> &'static str {
    match risk {
        RiskLevel::Low => "low",
        RiskLevel::Medium => "medium",
        RiskLevel::High => "high",
        RiskLevel::Critical => "critical",
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommandHistoryEntry {
    pub command: String,
    pub last_used_at_ms: u64,
    pub use_count: u32,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FuzzyResult {
    pub command: String,
    pub score: u32,
    pub indices: Vec<u32>,
    pub source: String,
    pub display: String,
}

#[cfg(test)]
mod tests {
    use crate::models::quick_commands::{
        QuickCommand, QuickCommandsConfig, export_quick_commands_json,
    };

    #[test]
    fn legacy_null_and_disabled_literal_modes_keep_the_old_serialized_shape() {
        for extra in [
            serde_json::json!({}),
            serde_json::json!({"raw_send": null}),
            serde_json::json!({"raw_send": false}),
        ] {
            let mut value = serde_json::json!({"id": "old", "label": "Legacy", "command": "pwd"});
            value
                .as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            let command: QuickCommand = serde_json::from_value(value).unwrap();
            assert!(!command.raw_send.unwrap_or_default());
            assert!(
                serde_json::to_value(&command)
                    .unwrap()
                    .get("raw_send")
                    .is_none()
            );
            let exported = export_quick_commands_json(QuickCommandsConfig {
                commands: vec![command],
                categories: Vec::new(),
            })
            .unwrap();
            let value: serde_json::Value = serde_json::from_str(&exported).unwrap();
            assert!(value["commands"][0].get("raw_send").is_none());
        }
    }

    #[test]
    fn literal_mode_survives_serialization_and_export_without_changing_text() {
        let command: QuickCommand = serde_json::from_value(serde_json::json!({
            "id": "literal", "label": "Literal", "command": "  {{变量}} C:\\temp\\p\n下一行  ", "raw_send": true,
        })).unwrap();
        let serialized = serde_json::to_string(&command).unwrap();
        assert_eq!(
            serde_json::from_str::<QuickCommand>(&serialized).unwrap(),
            command
        );
        let exported = export_quick_commands_json(QuickCommandsConfig {
            commands: vec![command.clone()],
            categories: Vec::new(),
        })
        .unwrap();
        let loaded: QuickCommandsConfig = serde_json::from_str(&exported).unwrap();
        assert_eq!(loaded.commands[0].raw_send, Some(true));
        assert_eq!(loaded.commands[0].command, command.command);
    }
}
