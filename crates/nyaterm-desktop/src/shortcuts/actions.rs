//! Semantic GPUI actions dispatched by the resolved NyaTerm keymap.

use gpui::actions;

actions!(
    nyaterm_shortcuts,
    [
        TerminalCopy,
        TerminalPaste,
        TerminalPasteSelected,
        TerminalFind,
        TerminalClear,
        TerminalClearAll,
        TerminalCommandPrevious,
        TerminalCommandNext,
        TerminalCommandSelect,
        TerminalSelectAll,
        TerminalLocalMode,
        ManageSyncGroups,
        ShowCommandSuggestions,
        ToggleRecording,
        NewSession,
        OpenNewSessionMenu,
        TemporarySshLink,
        QuickSwitch,
        NewLocalTerminal,
        CloseTab,
        NextTab,
        PreviousTab,
        NextTerminalGroup,
        PreviousTerminalGroup,
        DuplicateSession,
        MultiplexSsh,
        DuplicateSessionWithCommand,
        MultiplexSshWithCommand,
        ToggleLeftSidebar,
        ToggleRightSidebar,
        TogglePaneFocus,
        ToggleNativeFullscreen,
        ToggleRdpFullscreen,
        ZoomIn,
        ZoomOut,
        ResetZoom,
        OpenSettings,
        OpenChat,
        ShowAllCommands,
        RenameFile,
        UploadFile,
        DownloadFiles,
        CopySelectedConnections,
        LockScreen
    ]
);

#[derive(Clone, Debug, PartialEq, Eq, gpui::Action)]
#[action(namespace = nyaterm_shortcuts, no_json)]
pub(crate) struct SwitchToTab {
    pub(crate) index: usize,
}
