use gpui::Pixels;
use nyaterm_core::{AiAction, AiContext, QuickCommand, QuickCommandCategory};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HeaderStatusMode {
    Session,
    Resources,
    Host,
    Gpu,
    Npu,
    DateTime,
}

impl HeaderStatusMode {
    pub(crate) const ALL: [Self; 6] = [
        Self::Session,
        Self::Resources,
        Self::Host,
        Self::DateTime,
        Self::Gpu,
        Self::Npu,
    ];

    pub(crate) fn from_setting(value: &str) -> Self {
        match value.trim() {
            "resources" => Self::Resources,
            "host" => Self::Host,
            "gpu" => Self::Gpu,
            "npu" => Self::Npu,
            "datetime" => Self::DateTime,
            _ => Self::Session,
        }
    }

    pub(crate) const fn persistence_id(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::Resources => "resources",
            Self::Host => "host",
            Self::Gpu => "gpu",
            Self::Npu => "npu",
            Self::DateTime => "datetime",
        }
    }

    pub(crate) const fn i18n_key(self) -> &'static str {
        match self {
            Self::Session => "headerStatus.session",
            Self::Resources => "headerStatus.resources",
            Self::Host => "headerStatus.host",
            Self::Gpu => "headerStatus.gpu",
            Self::Npu => "headerStatus.npu",
            Self::DateTime => "headerStatus.datetime",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HeaderStatusState {
    pub(crate) rendered_minute: i64,
    pub(crate) gpu_page: usize,
    pub(crate) npu_page: usize,
}

impl Default for HeaderStatusState {
    fn default() -> Self {
        Self {
            rendered_minute: -1,
            gpu_page: 0,
            npu_page: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RightFocus {
    Default,
    Recording,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BottomPanelMode {
    QuickCommands,
    CommandSend,
    Hidden,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QuickCommandSortMode {
    Usage,
    Name,
    Created,
    Custom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QuickCommandViewMode {
    List,
    Compact,
    Tile,
    CommandBar,
    Sidebar,
}

impl QuickCommandViewMode {
    pub(crate) const fn is_bottom_panel(self) -> bool {
        matches!(self, Self::List | Self::Compact | Self::Tile)
    }

    pub(crate) const fn is_command_bar(self) -> bool {
        matches!(self, Self::CommandBar)
    }

    pub(crate) const fn is_sidebar(self) -> bool {
        matches!(self, Self::Sidebar)
    }

    pub(crate) const fn icon_path(self) -> &'static str {
        match self {
            Self::List => "icons/view-list.svg",
            Self::Compact => "icons/view-compact.svg",
            Self::Tile => "icons/view-grid.svg",
            Self::CommandBar => "icons/menu/horizontal.svg",
            Self::Sidebar => "icons/menu/sidebar.svg",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QuickCommandEditorField {
    Label,
    Command,
    Category,
    Description,
    SegmentedInterval,
}

#[derive(Debug, Clone)]
pub(crate) struct QuickCommandEditorState {
    pub(crate) original: Option<QuickCommand>,
    pub(crate) focused_field: QuickCommandEditorField,
    pub(crate) label: String,
    pub(crate) command: String,
    pub(crate) category_id: Option<String>,
    pub(crate) category_draft: String,
    pub(crate) description: String,
    pub(crate) color_tag: Option<String>,
    pub(crate) icon_tag: Option<String>,
    pub(crate) pinned: bool,
    pub(crate) execution_mode: String,
    pub(crate) segmented_send_mode: nyaterm_core::QuickCommandSegmentedSendMode,
    pub(crate) segmented_send_interval_input: String,
    pub(crate) raw_send: bool,
    pub(crate) error: Option<String>,
    /// Which field the error belongs to. `None` is a general error, which the
    /// dialog shows in its own box instead of beside a caption.
    pub(crate) error_field: Option<QuickCommandEditorField>,
}

#[derive(Debug, Clone)]
pub(crate) struct QuickCommandDetailsState {
    pub(crate) command: QuickCommand,
    pub(crate) x: Pixels,
    pub(crate) y: Pixels,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AiDetectedErrorState {
    pub(crate) session_id: String,
    pub(crate) output: String,
}

#[derive(Debug, Clone)]
pub(crate) struct QuickCommandCategoryDeleteState {
    pub(crate) id: String,
    pub(crate) name: String,
}

#[derive(Debug, Clone)]
pub(crate) struct QuickCommandCategoryRenameState {
    pub(crate) id: String,
    pub(crate) original_name: String,
    pub(crate) draft: String,
    pub(crate) error: Option<String>,
}

/// Draft for the "add category" dialog. `parent_id` is `None` when the menu was
/// opened from a pseudo-row, which creates a root category.
#[derive(Debug, Clone)]
pub(crate) struct QuickCommandCategoryCreateState {
    pub(crate) parent_id: Option<String>,
    pub(crate) draft: String,
    pub(crate) error: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct QuickCommandVariableDef {
    pub(crate) raw: String,
    pub(crate) name: String,
    pub(crate) options: Vec<String>,
    pub(crate) value: String,
}

#[derive(Debug, Clone)]
pub(crate) struct QuickCommandVariablePromptState {
    pub(crate) command_id: String,
    pub(crate) label: String,
    pub(crate) command: String,
    pub(crate) execute: bool,
    pub(crate) send_to_all: bool,
    pub(crate) segmented_send: Option<nyaterm_core::QuickCommandSegmentedSend>,
    pub(crate) variables: Vec<QuickCommandVariableDef>,
}

impl QuickCommandEditorState {
    pub(crate) fn blank() -> Self {
        Self {
            original: None,
            focused_field: QuickCommandEditorField::Label,
            label: String::new(),
            command: String::new(),
            category_id: None,
            category_draft: String::new(),
            description: String::new(),
            color_tag: None,
            icon_tag: None,
            pinned: false,
            execution_mode: "execute".to_string(),
            segmented_send_mode: nyaterm_core::QuickCommandSegmentedSendMode::Line,
            segmented_send_interval_input: "1.00".to_string(),
            raw_send: false,
            error: None,
            error_field: None,
        }
    }

    /// `blank()` seeded with a category, for "add command" on a group row. An id
    /// that no longer resolves is dropped so the editor opens uncategorized rather
    /// than holding a dangling reference.
    pub(crate) fn blank_in_category(
        category_id: Option<String>,
        categories: &[QuickCommandCategory],
    ) -> Self {
        let category_id =
            category_id.filter(|id| categories.iter().any(|category| &category.id == id));
        Self {
            category_id,
            ..Self::blank()
        }
    }

    pub(crate) fn from_terminal_selection(command: String) -> Self {
        Self {
            focused_field: QuickCommandEditorField::Command,
            command,
            ..Self::blank()
        }
    }

    pub(crate) fn from_command(command: QuickCommand) -> Self {
        let segmented_send = command.segmented_send.as_ref();
        let segmented_send_mode = segmented_send.map(|send| send.mode).unwrap_or_default();
        let segmented_send_interval_input = segmented_send
            .map(|send| nyaterm_core::send_command::format_interval_ms(send.interval_ms))
            .unwrap_or_else(|| "1.00".to_string());
        let unsupported_error = segmented_send.and_then(|send| send.validate().err());
        let execution_mode = if segmented_send.is_some() {
            "segmented"
        } else if command.execution_mode.as_deref() == Some("append") {
            "append"
        } else {
            "execute"
        }
        .to_string();
        Self {
            focused_field: QuickCommandEditorField::Label,
            label: command.label.clone(),
            command: command.command.clone(),
            category_id: command.category_id.clone(),
            category_draft: String::new(),
            description: command.description.clone().unwrap_or_default(),
            color_tag: command.color_tag.clone(),
            icon_tag: command.icon_tag.clone(),
            pinned: command.pinned.unwrap_or_default(),
            raw_send: command.raw_send.unwrap_or_default(),
            execution_mode,
            segmented_send_mode,
            segmented_send_interval_input,
            error: unsupported_error,
            error_field: None,
            original: Some(command),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AiPreparedRequest {
    pub(crate) action: AiAction,
    pub(crate) context: AiContext,
    pub(crate) source_label: String,
}
