use gpui::{Action, Context, SharedString};
use nyaterm_ui::NyaMenuItem;

use crate::features::NyaTermApp;
use crate::models::{HeaderStatusMode, SmartSplitMode};
use crate::shortcuts;

/// Commands contain identifiers, never captured window state or presentation callbacks.
#[derive(Clone, Debug, PartialEq, Eq, gpui::Action)]
#[action(namespace = nyaterm_title_menu, no_json)]
pub(crate) enum TitleMenuCommand {
    NewWindow,
    NewSession,
    DefaultSessionSettings,
    OpenFileManager,
    ImportConfig,
    ExportConfig,
    ZoomIn,
    ZoomOut,
    ResetZoom,
    QuickSwitch,
    ToggleActionLinks,
    ToggleTerminalZoom,
    ToggleBroadcast,
    TerminalClear,
    RefitTerminals,
    Unsplit,
    Documentation,
    CheckUpdates,
    ViewLogs,
    About,
    SetTheme(String),
    SetTerminalTheme(Option<String>),
    SetLanguage(String),
    HideHeaderStatus,
    SetHeaderStatus(HeaderStatusMode),
    TogglePanelFloating,
    TogglePanelMultiOpen,
    ResetPanelLayout,
    TogglePanelVisibility(String),
    TogglePadding,
    ToggleLineNumbers,
    ToggleTimestamps,
    SmartSplit(SmartSplitMode),
    ManageSyncGroups,
    OpenSettings,
    ToggleLeftSidebar,
    ToggleRightSidebar,
    TerminalCopy,
    TerminalPaste,
    TerminalFind,
    TerminalSelectAll,
}

/// Cocoa revalidates menu items through GPUI's action availability lookup. A
/// disabled command must have an unhandled action type, not just a disabled flag.
#[derive(Clone, Debug, PartialEq, Eq, gpui::Action)]
#[action(namespace = nyaterm_title_menu, no_json)]
pub(crate) struct UnavailableTitleMenuCommand(pub(crate) TitleMenuCommand);

impl TitleMenuCommand {
    /// Reuse semantic actions so native accelerators come from the resolved keymap.
    pub(crate) fn native_action(&self) -> Box<dyn Action> {
        match self {
            Self::NewWindow => Box::new(crate::app_shell::NativeNewWindow),
            Self::NewSession => Box::new(shortcuts::NewSession),
            Self::ZoomIn => Box::new(shortcuts::ZoomIn),
            Self::ZoomOut => Box::new(shortcuts::ZoomOut),
            Self::ResetZoom => Box::new(shortcuts::ResetZoom),
            Self::QuickSwitch => Box::new(shortcuts::QuickSwitch),
            Self::TerminalClear => Box::new(shortcuts::TerminalClear),
            Self::ManageSyncGroups => Box::new(shortcuts::ManageSyncGroups),
            Self::OpenSettings => Box::new(shortcuts::OpenSettings),
            Self::ToggleLeftSidebar => Box::new(shortcuts::ToggleLeftSidebar),
            Self::ToggleRightSidebar => Box::new(shortcuts::ToggleRightSidebar),
            Self::TerminalCopy => Box::new(shortcuts::TerminalCopy),
            Self::TerminalPaste => Box::new(shortcuts::TerminalPaste),
            Self::TerminalFind => Box::new(shortcuts::TerminalFind),
            Self::TerminalSelectAll => Box::new(shortcuts::TerminalSelectAll),
            _ => Box::new(self.clone()),
        }
    }

    pub(crate) fn needs_main_window(&self) -> bool {
        !matches!(
            self,
            Self::About
                | Self::Documentation
                | Self::CheckUpdates
                | Self::ViewLogs
                | Self::NewWindow
                | Self::NewSession
                | Self::DefaultSessionSettings
                | Self::OpenFileManager
                | Self::ImportConfig
                | Self::ExportConfig
                | Self::OpenSettings
                | Self::SetTheme(_)
                | Self::SetTerminalTheme(_)
                | Self::SetLanguage(_)
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TitleMenuKind {
    Action(Option<TitleMenuCommand>),
    Separator,
    Submenu(Vec<TitleMenuDefinition>),
}

/// An immutable projection shared by window and native menu adapters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TitleMenuDefinition {
    pub(crate) kind: TitleMenuKind,
    pub(crate) label: SharedString,
    pub(crate) disabled: bool,
    pub(crate) checked: bool,
    pub(crate) shortcut: Option<SharedString>,
    pub(crate) status: Option<(SharedString, u32)>,
    icon: Option<SharedString>,
    icon_color: Option<u32>,
    danger: bool,
}

impl TitleMenuDefinition {
    pub(crate) fn action(label: impl Into<SharedString>) -> Self {
        Self {
            kind: TitleMenuKind::Action(None),
            label: label.into(),
            disabled: false,
            checked: false,
            shortcut: None,
            status: None,
            icon: None,
            icon_color: None,
            danger: false,
        }
    }

    pub(crate) fn separator() -> Self {
        Self {
            kind: TitleMenuKind::Separator,
            ..Self::action("")
        }
    }

    pub(crate) fn submenu(label: impl Into<SharedString>, items: Vec<Self>) -> Self {
        Self {
            kind: TitleMenuKind::Submenu(items),
            ..Self::action(label)
        }
    }

    pub(crate) fn command(mut self, command: TitleMenuCommand) -> Self {
        self.kind = TitleMenuKind::Action(Some(command));
        self
    }

    pub(crate) fn icon(mut self, icon: impl Into<SharedString>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    pub(crate) fn icon_color(mut self, color: u32) -> Self {
        self.icon_color = Some(color);
        self
    }

    pub(crate) fn status(mut self, label: impl Into<SharedString>, color: u32) -> Self {
        self.status = Some((label.into(), color));
        self
    }

    pub(crate) fn shortcut(mut self, shortcut: impl Into<SharedString>) -> Self {
        self.shortcut = Some(shortcut.into());
        self
    }

    pub(crate) fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub(crate) fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }

    pub(crate) fn danger(mut self) -> Self {
        self.danger = true;
        self
    }

    pub(crate) fn into_window_item(self, cx: &mut Context<NyaTermApp>) -> NyaMenuItem {
        let mut item = match self.kind {
            TitleMenuKind::Separator => NyaMenuItem::separator(),
            TitleMenuKind::Submenu(children) => NyaMenuItem::submenu(
                self.label,
                children
                    .into_iter()
                    .map(|child| child.into_window_item(cx))
                    .collect(),
            ),
            TitleMenuKind::Action(command) => {
                let item = NyaMenuItem::action(self.label);
                if let Some(command) = command {
                    item.on_click(cx.listener(move |app, _, window, cx| {
                        app.execute_title_menu_command(&command, window, cx);
                    }))
                } else {
                    item
                }
            }
        }
        .disabled(self.disabled)
        .checked(self.checked);
        if let Some(icon) = self.icon {
            item = item.icon(icon);
        }
        if let Some(color) = self.icon_color {
            item = item.icon_color(color);
        }
        if let Some(shortcut) = self.shortcut {
            item = item.shortcut(shortcut);
        }
        if let Some((label, color)) = self.status {
            item = item.status(label, color);
        }
        if self.danger {
            item = item.danger();
        }
        item
    }
}
