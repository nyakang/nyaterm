//! Typed shortcut definitions, persistence parsing, and the rebuildable GPUI keymap.

use std::collections::{HashMap, HashSet};

use gpui::{App, Global, KeyBinding, KeyContext, KeyDownEvent, Keystroke, Modifiers};

mod actions;
pub(crate) use actions::*;

pub(crate) const WORKSPACE_KEY_CONTEXT: &str = "NyaWorkspace";
pub(crate) const FILE_EXPLORER_KEY_CONTEXT: &str = "FileExplorer";
pub(crate) const SAVED_CONNECTIONS_KEY_CONTEXT: &str = "SavedConnections";
pub(crate) const MODAL_KEY_CONTEXT: &str = "NyaModal";
pub(crate) const SCREEN_LOCKED_KEY_CONTEXT: &str = "ScreenLocked";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ShortcutId {
    TerminalCopy,
    TerminalPaste,
    TerminalPasteSelected,
    TerminalFind,
    TerminalClear,
    TerminalSelectAll,
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
    SwitchToTab,
    DuplicateSession,
    MultiplexSsh,
    DuplicateSessionWithCommand,
    MultiplexSshWithCommand,
    ToggleLeftSidebar,
    ToggleRightSidebar,
    TogglePaneFocus,
    ToggleNativeFullscreen,
    ZoomIn,
    ZoomOut,
    ResetZoom,
    OpenSettings,
    OpenChat,
    ShowAllCommands,
    RenameFile,
    CopySelectedConnections,
    LockScreen,
}

impl ShortcutId {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::TerminalCopy => "terminal.copy",
            Self::TerminalPaste => "terminal.paste",
            Self::TerminalPasteSelected => "terminal.pasteSelected",
            Self::TerminalFind => "terminal.find",
            Self::TerminalClear => "terminal.clear",
            Self::TerminalSelectAll => "terminal.selectAll",
            Self::ManageSyncGroups => "terminal.manageSyncGroups",
            Self::ShowCommandSuggestions => "terminal.showCommandSuggestions",
            Self::ToggleRecording => "terminal.recording.toggle",
            Self::NewSession => "tab.newSession",
            Self::OpenNewSessionMenu => "tab.openNewSessionMenu",
            Self::TemporarySshLink => "tab.temporarySshLink",
            Self::QuickSwitch => "tab.quickSwitch",
            Self::NewLocalTerminal => "tab.newLocalTerminal",
            Self::CloseTab => "tab.close",
            Self::NextTab => "tab.next",
            Self::PreviousTab => "tab.prev",
            Self::SwitchToTab => "tab.switchTo",
            Self::DuplicateSession => "tab.duplicateSession",
            Self::MultiplexSsh => "tab.multiplexSsh",
            Self::DuplicateSessionWithCommand => "tab.duplicateSessionWithCommand",
            Self::MultiplexSshWithCommand => "tab.multiplexSshWithCommand",
            Self::ToggleLeftSidebar => "view.toggleLeftSidebar",
            Self::ToggleRightSidebar => "view.toggleRightSidebar",
            Self::TogglePaneFocus => "view.togglePaneFocus",
            Self::ToggleNativeFullscreen => "view.toggleNativeFullscreen",
            Self::ZoomIn => "view.zoomIn",
            Self::ZoomOut => "view.zoomOut",
            Self::ResetZoom => "view.resetZoom",
            Self::OpenSettings => "view.openSettings",
            Self::OpenChat => "view.openChat",
            Self::ShowAllCommands => "view.showAllCommands",
            Self::RenameFile => "fileExplorer.rename",
            Self::CopySelectedConnections => "savedConnections.copySelected",
            Self::LockScreen => "special.lockScreen",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        SHORTCUT_REGISTRY
            .iter()
            .find(|definition| definition.id.as_str() == value)
            .map(|definition| definition.id)
    }
}

/// Stable semantic action identity stored by every registry definition.
pub(crate) type ShortcutSemanticAction = ShortcutId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShortcutCategory {
    Terminal,
    Tab,
    View,
    FileExplorer,
    SavedConnections,
    Special,
}

impl ShortcutCategory {
    pub(crate) fn label_key(self) -> &'static str {
        match self {
            Self::Terminal => "settings.shortcutCategories.terminal",
            Self::Tab => "settings.shortcutCategories.tab",
            Self::View => "settings.shortcutCategories.view",
            Self::FileExplorer => "settings.shortcutCategories.fileExplorer",
            Self::SavedConnections => "settings.shortcutCategories.savedConnections",
            Self::Special => "settings.shortcutCategories.special",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShortcutNativeStatus {
    Supported,
    Partial,
    Contextual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[allow(dead_code)] // Reserved scopes are part of the shortcut compatibility model.
pub(crate) enum ShortcutScope {
    Global,
    Workspace,
    Terminal,
    TextInput,
    FileExplorer,
    SavedConnections,
    Modal,
}

impl ShortcutScope {
    fn context(self) -> Option<&'static str> {
        match self {
            Self::Global => None,
            Self::Workspace => Some("NyaWorkspace && !Dialog && !NyaModal && !ScreenLocked"),
            Self::Terminal => Some("Terminal"),
            Self::TextInput => Some("Input"),
            Self::FileExplorer => Some(FILE_EXPLORER_KEY_CONTEXT),
            Self::SavedConnections => Some(SAVED_CONNECTIONS_KEY_CONTEXT),
            Self::Modal => Some(MODAL_KEY_CONTEXT),
        }
    }

    pub(crate) fn overlaps(self, other: Self) -> bool {
        use ShortcutScope::*;
        match (self, other) {
            (Global, _) | (_, Global) => true,
            (Workspace, Workspace | Terminal | TextInput | FileExplorer | SavedConnections)
            | (Terminal | TextInput | FileExplorer | SavedConnections, Workspace) => true,
            _ => self == other,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShortcutKind {
    Direct,
    IndexedTab { first: u8, last: u8 },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct ShortcutChord(Keystroke);

impl ShortcutChord {
    pub(crate) fn new(keystroke: Keystroke) -> Result<Self, String> {
        if is_modifier_key(&keystroke.key) {
            return Err("a shortcut must include a non-modifier key".to_string());
        }
        if keystroke.key.trim().is_empty() {
            return Err("a shortcut key cannot be empty".to_string());
        }
        Ok(Self(keystroke))
    }

    pub(crate) fn keystroke(&self) -> &Keystroke {
        &self.0
    }

    fn with_key(&self, key: impl Into<String>) -> Self {
        let mut keystroke = self.0.clone();
        keystroke.key = key.into();
        keystroke.key_char = None;
        Self(keystroke)
    }

    fn gpui_source(&self) -> String {
        let mut parts = Vec::new();
        if self.0.modifiers.control {
            parts.push("ctrl".to_string());
        }
        if self.0.modifiers.platform {
            parts.push("cmd".to_string());
        }
        if self.0.modifiers.alt {
            parts.push("alt".to_string());
        }
        if self.0.modifiers.shift {
            parts.push("shift".to_string());
        }
        if self.0.modifiers.function {
            parts.push("fn".to_string());
        }
        parts.push(self.0.key.clone());
        parts.join("-")
    }

    fn canonical(&self) -> String {
        let mut parts = Vec::new();
        if self.0.modifiers.control {
            parts.push("ctrl".to_string());
        }
        if self.0.modifiers.platform {
            parts.push("meta".to_string());
        }
        if self.0.modifiers.alt {
            parts.push("alt".to_string());
        }
        if self.0.modifiers.shift {
            parts.push("shift".to_string());
        }
        if self.0.modifiers.function {
            parts.push("fn".to_string());
        }
        parts.push(canonical_key_name(&self.0.key));
        parts.join("+")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ShortcutBinding {
    chords: Vec<ShortcutChord>,
}

impl ShortcutBinding {
    pub(crate) fn parse(source: &str) -> Result<Self, String> {
        let alternatives = split_alternatives(source);
        if alternatives.is_empty() {
            return Err("shortcut binding is empty".to_string());
        }
        let mut chords = Vec::with_capacity(alternatives.len());
        let mut seen = HashSet::new();
        for alternative in alternatives {
            let chord = parse_chord(alternative)?;
            if seen.insert(chord.clone()) {
                chords.push(chord);
            }
        }
        Ok(Self { chords })
    }

    pub(crate) fn from_keystroke(keystroke: Keystroke) -> Result<Self, String> {
        Ok(Self {
            chords: vec![ShortcutChord::new(keystroke)?],
        })
    }

    pub(crate) fn chords(&self) -> &[ShortcutChord] {
        &self.chords
    }

    pub(crate) fn canonical(&self) -> String {
        self.chords
            .iter()
            .map(ShortcutChord::canonical)
            .collect::<Vec<_>>()
            .join(",")
    }

    pub(crate) fn contains(&self, chord: &ShortcutChord) -> bool {
        self.chords.contains(chord)
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ShortcutDefinition {
    pub(crate) id: ShortcutId,
    pub(crate) action: ShortcutSemanticAction,
    pub(crate) category: ShortcutCategory,
    pub(crate) label_key: &'static str,
    pub(crate) scope: ShortcutScope,
    pub(crate) kind: ShortcutKind,
    non_macos_defaults: &'static str,
    macos_defaults: &'static str,
    pub(crate) native_status: ShortcutNativeStatus,
    pub(crate) note: &'static str,
}

impl ShortcutDefinition {
    pub(crate) fn default_binding(&self) -> ShortcutBinding {
        self.default_binding_for(if cfg!(target_os = "macos") {
            ShortcutPlatform::MacOs
        } else if cfg!(target_os = "windows") {
            ShortcutPlatform::Windows
        } else {
            ShortcutPlatform::Linux
        })
    }

    pub(crate) fn default_binding_for(&self, platform: ShortcutPlatform) -> ShortcutBinding {
        let source = if platform == ShortcutPlatform::MacOs {
            self.macos_defaults
        } else {
            self.non_macos_defaults
        };
        ShortcutBinding::parse(source).expect("registry defaults must be valid")
    }

    pub(crate) fn default_keys(&self) -> String {
        self.default_binding().canonical()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShortcutPlatform {
    Windows,
    Linux,
    MacOs,
}

pub(crate) const SHORTCUT_CATEGORIES: [ShortcutCategory; 6] = [
    ShortcutCategory::Terminal,
    ShortcutCategory::Tab,
    ShortcutCategory::View,
    ShortcutCategory::FileExplorer,
    ShortcutCategory::SavedConnections,
    ShortcutCategory::Special,
];

macro_rules! shortcut {
    ($id:ident, $category:ident, $label:literal, $scope:ident, $kind:expr, $ctrl:literal, $mac:literal, $status:ident, $note:literal) => {
        ShortcutDefinition {
            id: ShortcutId::$id,
            action: ShortcutId::$id,
            category: ShortcutCategory::$category,
            label_key: $label,
            scope: ShortcutScope::$scope,
            kind: $kind,
            non_macos_defaults: $ctrl,
            macos_defaults: $mac,
            native_status: ShortcutNativeStatus::$status,
            note: $note,
        }
    };
}

pub(crate) const SHORTCUT_REGISTRY: [ShortcutDefinition; 35] = [
    shortcut!(
        TerminalCopy,
        Terminal,
        "settings.shortcutLabels.copy",
        Terminal,
        ShortcutKind::Direct,
        "ctrl+shift+c",
        "meta+shift+c",
        Supported,
        "Copies the current selection, otherwise the visible terminal text."
    ),
    shortcut!(
        TerminalPaste,
        Terminal,
        "settings.shortcutLabels.paste",
        Terminal,
        ShortcutKind::Direct,
        "ctrl+shift+v,shift+insert",
        "meta+shift+v,shift+insert",
        Partial,
        "Pastes clipboard text into the active terminal."
    ),
    shortcut!(
        TerminalPasteSelected,
        Terminal,
        "settings.shortcutLabels.pasteSelectedText",
        Terminal,
        ShortcutKind::Direct,
        "ctrl+shift+x",
        "meta+shift+x",
        Supported,
        "Pastes the current terminal selection."
    ),
    shortcut!(
        TerminalFind,
        Terminal,
        "settings.shortcutLabels.find",
        Terminal,
        ShortcutKind::Direct,
        "ctrl+shift+f",
        "meta+shift+f",
        Supported,
        "Opens terminal buffer search."
    ),
    shortcut!(
        TerminalClear,
        Terminal,
        "settings.shortcutLabels.clearScreen",
        Terminal,
        ShortcutKind::Direct,
        "ctrl+l",
        "meta+l",
        Supported,
        "Clears the active terminal screen."
    ),
    shortcut!(
        TerminalSelectAll,
        Terminal,
        "settings.shortcutLabels.selectAll",
        Terminal,
        ShortcutKind::Direct,
        "ctrl+shift+a",
        "meta+shift+a",
        Supported,
        "Selects the visible terminal grid."
    ),
    shortcut!(
        ManageSyncGroups,
        Terminal,
        "settings.shortcutLabels.manageSyncGroups",
        Workspace,
        ShortcutKind::Direct,
        "ctrl+shift+g",
        "meta+shift+g",
        Supported,
        "Opens synchronized input groups."
    ),
    shortcut!(
        ShowCommandSuggestions,
        Terminal,
        "settings.shortcutLabels.showCommandSuggestions",
        Terminal,
        ShortcutKind::Direct,
        "alt+r",
        "alt+r",
        Supported,
        "Shows recent history and pinned quick commands."
    ),
    shortcut!(
        ToggleRecording,
        Terminal,
        "settings.shortcutLabels.toggleSessionRecording",
        Workspace,
        ShortcutKind::Direct,
        "ctrl+alt+r",
        "meta+alt+r",
        Supported,
        "Starts or stops transcript recording."
    ),
    shortcut!(
        NewSession,
        Tab,
        "settings.shortcutLabels.newSession",
        Workspace,
        ShortcutKind::Direct,
        "ctrl+shift+t",
        "meta+shift+t",
        Supported,
        "Opens saved connections."
    ),
    shortcut!(
        OpenNewSessionMenu,
        Tab,
        "settings.shortcutLabels.openNewSessionMenu",
        Workspace,
        ShortcutKind::Direct,
        "ctrl+shift+o",
        "meta+shift+o",
        Supported,
        "Opens the new session menu."
    ),
    shortcut!(
        TemporarySshLink,
        Tab,
        "settings.shortcutLabels.temporarySshLink",
        Workspace,
        ShortcutKind::Direct,
        "ctrl+alt+n",
        "meta+alt+n",
        Supported,
        "Opens a temporary SSH link dialog."
    ),
    shortcut!(
        QuickSwitch,
        Tab,
        "settings.shortcutLabels.quickSwitch",
        Workspace,
        ShortcutKind::Direct,
        "ctrl+shift+s",
        "meta+shift+s",
        Supported,
        "Opens the session switcher."
    ),
    shortcut!(
        NewLocalTerminal,
        Tab,
        "settings.shortcutLabels.newLocalTerminal",
        Workspace,
        ShortcutKind::Direct,
        "ctrl+backquote",
        "meta+backquote",
        Supported,
        "Starts a local PTY session."
    ),
    shortcut!(
        CloseTab,
        Tab,
        "settings.shortcutLabels.closeTab",
        Workspace,
        ShortcutKind::Direct,
        "ctrl+shift+w",
        "meta+w,meta+shift+w",
        Supported,
        "Closes the active session."
    ),
    shortcut!(
        NextTab,
        Tab,
        "settings.shortcutLabels.nextTab",
        Workspace,
        ShortcutKind::Direct,
        "ctrl+tab",
        "meta+tab",
        Supported,
        "Cycles forward through sessions."
    ),
    shortcut!(
        PreviousTab,
        Tab,
        "settings.shortcutLabels.prevTab",
        Workspace,
        ShortcutKind::Direct,
        "ctrl+shift+tab",
        "meta+shift+tab",
        Supported,
        "Cycles backward through sessions."
    ),
    shortcut!(
        SwitchToTab,
        Tab,
        "settings.shortcutLabels.switchTab",
        Workspace,
        ShortcutKind::IndexedTab { first: 1, last: 9 },
        "ctrl+1",
        "meta+1",
        Supported,
        "Switches to tabs 1-8, or the last tab with 9."
    ),
    shortcut!(
        DuplicateSession,
        Tab,
        "settings.shortcutLabels.duplicateSession",
        Workspace,
        ShortcutKind::Direct,
        "ctrl+shift+d",
        "meta+shift+d",
        Supported,
        "Duplicates the active session."
    ),
    shortcut!(
        MultiplexSsh,
        Tab,
        "settings.shortcutLabels.multiplexSsh",
        Workspace,
        ShortcutKind::Direct,
        "ctrl+shift+m",
        "meta+shift+m",
        Supported,
        "Creates a multiplexed SSH session."
    ),
    shortcut!(
        DuplicateSessionWithCommand,
        Tab,
        "settings.shortcutLabels.duplicateSessionWithCommand",
        Workspace,
        ShortcutKind::Direct,
        "ctrl+alt+d",
        "meta+alt+d",
        Supported,
        "Opens the duplicate-and-run dialog."
    ),
    shortcut!(
        MultiplexSshWithCommand,
        Tab,
        "settings.shortcutLabels.multiplexSshWithCommand",
        Workspace,
        ShortcutKind::Direct,
        "ctrl+alt+m",
        "meta+alt+m",
        Supported,
        "Opens the multiplex-and-run dialog."
    ),
    shortcut!(
        ToggleLeftSidebar,
        View,
        "settings.shortcutLabels.toggleLeftSidebar",
        Workspace,
        ShortcutKind::Direct,
        "ctrl+shift+e",
        "meta+shift+e",
        Supported,
        "Toggles the Explorer panel."
    ),
    shortcut!(
        ToggleRightSidebar,
        View,
        "settings.shortcutLabels.toggleRightSidebar",
        Workspace,
        ShortcutKind::Direct,
        "ctrl+shift+b",
        "meta+shift+b",
        Supported,
        "Toggles the Inspector panel."
    ),
    shortcut!(
        TogglePaneFocus,
        View,
        "settings.shortcutLabels.togglePaneFocus",
        Workspace,
        ShortcutKind::Direct,
        "ctrl+shift+enter",
        "meta+shift+enter",
        Supported,
        "Focuses the active pane."
    ),
    shortcut!(
        ToggleNativeFullscreen,
        View,
        "settings.shortcutLabels.toggleNativeFullscreen",
        Workspace,
        ShortcutKind::Direct,
        "f11",
        "ctrl+meta+f",
        Supported,
        "Toggles native fullscreen."
    ),
    shortcut!(
        ZoomIn,
        View,
        "settings.shortcutLabels.zoomIn",
        Workspace,
        ShortcutKind::Direct,
        "ctrl+equal,ctrl+shift+equal",
        "meta+equal,meta+shift+equal",
        Supported,
        "Increases terminal font size."
    ),
    shortcut!(
        ZoomOut,
        View,
        "settings.shortcutLabels.zoomOut",
        Workspace,
        ShortcutKind::Direct,
        "ctrl+-",
        "meta+-",
        Supported,
        "Decreases terminal font size."
    ),
    shortcut!(
        ResetZoom,
        View,
        "settings.shortcutLabels.resetZoom",
        Workspace,
        ShortcutKind::Direct,
        "ctrl+0",
        "meta+0",
        Supported,
        "Resets terminal font size."
    ),
    shortcut!(
        OpenSettings,
        View,
        "settings.shortcutLabels.openSettings",
        Workspace,
        ShortcutKind::Direct,
        "ctrl+comma",
        "meta+comma",
        Supported,
        "Opens Settings."
    ),
    shortcut!(
        OpenChat,
        View,
        "settings.shortcutLabels.openChat",
        Workspace,
        ShortcutKind::Direct,
        "ctrl+alt+i",
        "meta+alt+i",
        Supported,
        "Focuses the AI panel."
    ),
    shortcut!(
        ShowAllCommands,
        View,
        "settings.shortcutLabels.showAllCommands",
        Workspace,
        ShortcutKind::Direct,
        "ctrl+shift+p",
        "meta+shift+p",
        Supported,
        "Opens quick commands."
    ),
    shortcut!(
        RenameFile,
        FileExplorer,
        "settings.shortcutLabels.renameFile",
        FileExplorer,
        ShortcutKind::Direct,
        "f2",
        "f2",
        Contextual,
        "Renames the selected SFTP entry."
    ),
    shortcut!(
        CopySelectedConnections,
        SavedConnections,
        "settings.shortcutLabels.copySelectedSavedConnections",
        SavedConnections,
        ShortcutKind::Direct,
        "ctrl+alt+c",
        "meta+alt+c",
        Supported,
        "Duplicates selected saved connections without secrets."
    ),
    shortcut!(
        LockScreen,
        Special,
        "settings.shortcutLabels.lockScreen",
        Workspace,
        ShortcutKind::Direct,
        "ctrl+shift+l",
        "meta+shift+l",
        Supported,
        "Locks the workspace."
    ),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ShortcutDiagnosticKind {
    Invalid(String),
    Conflict(ShortcutId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ShortcutDiagnostic {
    pub(crate) id: ShortcutId,
    pub(crate) kind: ShortcutDiagnosticKind,
}

#[derive(Debug, Clone)]
pub(crate) struct ResolvedShortcut {
    pub(crate) id: ShortcutId,
    pub(crate) binding: ShortcutBinding,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ShortcutInvocation {
    pub(crate) id: ShortcutId,
    pub(crate) tab_index: Option<usize>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ResolvedKeymap {
    pub(crate) shortcuts: Vec<ResolvedShortcut>,
    pub(crate) diagnostics: Vec<ShortcutDiagnostic>,
}

impl ResolvedKeymap {
    pub(crate) fn resolve(overrides: &HashMap<String, String>) -> Self {
        let mut resolved = Self::default();
        let mut accepted: Vec<(ShortcutId, ShortcutScope, ShortcutChord)> = Vec::new();
        for definition in SHORTCUT_REGISTRY {
            let binding = match overrides.get(definition.id.as_str()) {
                Some(raw) => match ShortcutBinding::parse(raw) {
                    Ok(binding) => binding,
                    Err(error) => {
                        resolved.diagnostics.push(ShortcutDiagnostic {
                            id: definition.id,
                            kind: ShortcutDiagnosticKind::Invalid(error),
                        });
                        continue;
                    }
                },
                None => definition.default_binding(),
            };
            let candidate_chords = expanded_chords(&definition, &binding);
            let conflict = candidate_chords.iter().find_map(|chord| {
                accepted
                    .iter()
                    .find(|(_, scope, accepted_chord)| {
                        definition.scope.overlaps(*scope) && chord == accepted_chord
                    })
                    .map(|(id, _, _)| *id)
            });
            if let Some(first) = conflict {
                resolved.diagnostics.push(ShortcutDiagnostic {
                    id: definition.id,
                    kind: ShortcutDiagnosticKind::Conflict(first),
                });
                continue;
            }
            accepted.extend(
                candidate_chords
                    .into_iter()
                    .map(|chord| (definition.id, definition.scope, chord)),
            );
            resolved.shortcuts.push(ResolvedShortcut {
                id: definition.id,
                binding,
            });
        }
        resolved
    }

    pub(crate) fn diagnostic(&self, id: ShortcutId) -> Option<&ShortcutDiagnosticKind> {
        self.diagnostics
            .iter()
            .find(|diagnostic| diagnostic.id == id)
            .map(|diagnostic| &diagnostic.kind)
    }
}

struct ShortcutKeymapGlobal {
    baseline: Vec<KeyBinding>,
    #[allow(dead_code)] // Kept as the authoritative runtime resolution snapshot.
    resolved: ResolvedKeymap,
}

impl Global for ShortcutKeymapGlobal {}

pub(crate) fn init(cx: &mut App) {
    ensure_global(cx);
}

fn ensure_global(cx: &mut App) {
    if cx.has_global::<ShortcutKeymapGlobal>() {
        return;
    }
    let baseline = cx.key_bindings().borrow().bindings().cloned().collect();
    cx.set_global(ShortcutKeymapGlobal {
        baseline,
        resolved: ResolvedKeymap::default(),
    });
}

pub(crate) fn rebuild_keymap(overrides: &HashMap<String, String>, cx: &mut App) {
    ensure_global(cx);
    let resolved = ResolvedKeymap::resolve(overrides);
    let baseline = cx.global::<ShortcutKeymapGlobal>().baseline.clone();
    let bindings = gpui_bindings(&resolved);
    cx.clear_key_bindings();
    cx.bind_keys(baseline.clone());
    cx.bind_keys(bindings);
    crate::features::init_protection_key_bindings(cx);
    cx.set_global(ShortcutKeymapGlobal { baseline, resolved });
}

pub(crate) fn resolve_shortcut_invocation(
    keystroke: &Keystroke,
    context_stack: &[KeyContext],
    cx: &App,
) -> Option<ShortcutInvocation> {
    let keymap = cx.try_global::<ShortcutKeymapGlobal>()?;
    resolve_invocation(&keymap.resolved, keystroke, context_stack)
}

fn resolve_invocation(
    resolved: &ResolvedKeymap,
    keystroke: &Keystroke,
    context_stack: &[KeyContext],
) -> Option<ShortcutInvocation> {
    for shortcut in &resolved.shortcuts {
        let definition = SHORTCUT_REGISTRY
            .iter()
            .find(|definition| definition.id == shortcut.id)
            .expect("resolved shortcut must have a definition");
        if !scope_allows(definition.scope, context_stack) {
            continue;
        }
        match definition.kind {
            ShortcutKind::Direct => {
                if shortcut
                    .binding
                    .chords()
                    .iter()
                    .any(|chord| keystroke_matches(keystroke, chord.keystroke()))
                {
                    return Some(ShortcutInvocation {
                        id: shortcut.id,
                        tab_index: None,
                    });
                }
            }
            ShortcutKind::IndexedTab { first, last } => {
                for template in shortcut.binding.chords() {
                    for index in first..=last {
                        let chord = template.with_key(index.to_string());
                        if keystroke_matches(keystroke, chord.keystroke()) {
                            return Some(ShortcutInvocation {
                                id: shortcut.id,
                                tab_index: Some(usize::from(index)),
                            });
                        }
                    }
                }
            }
        }
    }
    None
}

fn scope_allows(scope: ShortcutScope, context_stack: &[KeyContext]) -> bool {
    let has_context = |name: &str| context_stack.iter().any(|context| context.contains(name));
    if has_context("Dialog")
        || has_context(MODAL_KEY_CONTEXT)
        || has_context(SCREEN_LOCKED_KEY_CONTEXT)
    {
        return false;
    }
    match scope {
        ShortcutScope::Global | ShortcutScope::Workspace => true,
        ShortcutScope::Terminal => has_context("Terminal"),
        ShortcutScope::TextInput => has_context("Input"),
        ShortcutScope::FileExplorer => has_context(FILE_EXPLORER_KEY_CONTEXT),
        ShortcutScope::SavedConnections => has_context(SAVED_CONNECTIONS_KEY_CONTEXT),
        ShortcutScope::Modal => has_context(MODAL_KEY_CONTEXT),
    }
}

fn keystroke_matches(event: &Keystroke, binding: &Keystroke) -> bool {
    event.modifiers == binding.modifiers && normalize_key_name(&event.key) == binding.key
}

fn gpui_bindings(resolved: &ResolvedKeymap) -> Vec<KeyBinding> {
    let mut bindings = Vec::new();
    for shortcut in &resolved.shortcuts {
        let definition = SHORTCUT_REGISTRY
            .iter()
            .find(|definition| definition.id == shortcut.id)
            .expect("resolved shortcut must have a definition");
        match definition.kind {
            ShortcutKind::Direct => {
                for chord in shortcut.binding.chords() {
                    bindings.push(binding_for_action(
                        definition.action,
                        chord,
                        definition.scope.context(),
                    ));
                }
            }
            ShortcutKind::IndexedTab { first, last } => {
                for template in shortcut.binding.chords() {
                    for index in first..=last {
                        let chord = template.with_key(index.to_string());
                        bindings.push(KeyBinding::new(
                            &chord.gpui_source(),
                            SwitchToTab {
                                index: usize::from(index),
                            },
                            definition.scope.context(),
                        ));
                    }
                }
            }
        }
    }
    bindings
}

fn binding_for_action(id: ShortcutId, chord: &ShortcutChord, context: Option<&str>) -> KeyBinding {
    let source = chord.gpui_source();
    macro_rules! binding {
        ($action:ident) => {
            KeyBinding::new(&source, $action, context)
        };
    }
    match id {
        ShortcutId::TerminalCopy => binding!(TerminalCopy),
        ShortcutId::TerminalPaste => binding!(TerminalPaste),
        ShortcutId::TerminalPasteSelected => binding!(TerminalPasteSelected),
        ShortcutId::TerminalFind => binding!(TerminalFind),
        ShortcutId::TerminalClear => binding!(TerminalClear),
        ShortcutId::TerminalSelectAll => binding!(TerminalSelectAll),
        ShortcutId::ManageSyncGroups => binding!(ManageSyncGroups),
        ShortcutId::ShowCommandSuggestions => binding!(ShowCommandSuggestions),
        ShortcutId::ToggleRecording => binding!(ToggleRecording),
        ShortcutId::NewSession => binding!(NewSession),
        ShortcutId::OpenNewSessionMenu => binding!(OpenNewSessionMenu),
        ShortcutId::TemporarySshLink => binding!(TemporarySshLink),
        ShortcutId::QuickSwitch => binding!(QuickSwitch),
        ShortcutId::NewLocalTerminal => binding!(NewLocalTerminal),
        ShortcutId::CloseTab => binding!(CloseTab),
        ShortcutId::NextTab => binding!(NextTab),
        ShortcutId::PreviousTab => binding!(PreviousTab),
        ShortcutId::SwitchToTab => unreachable!("indexed actions are expanded separately"),
        ShortcutId::DuplicateSession => binding!(DuplicateSession),
        ShortcutId::MultiplexSsh => binding!(MultiplexSsh),
        ShortcutId::DuplicateSessionWithCommand => binding!(DuplicateSessionWithCommand),
        ShortcutId::MultiplexSshWithCommand => binding!(MultiplexSshWithCommand),
        ShortcutId::ToggleLeftSidebar => binding!(ToggleLeftSidebar),
        ShortcutId::ToggleRightSidebar => binding!(ToggleRightSidebar),
        ShortcutId::TogglePaneFocus => binding!(TogglePaneFocus),
        ShortcutId::ToggleNativeFullscreen => binding!(ToggleNativeFullscreen),
        ShortcutId::ZoomIn => binding!(ZoomIn),
        ShortcutId::ZoomOut => binding!(ZoomOut),
        ShortcutId::ResetZoom => binding!(ResetZoom),
        ShortcutId::OpenSettings => binding!(OpenSettings),
        ShortcutId::OpenChat => binding!(OpenChat),
        ShortcutId::ShowAllCommands => binding!(ShowAllCommands),
        ShortcutId::RenameFile => binding!(RenameFile),
        ShortcutId::CopySelectedConnections => binding!(CopySelectedConnections),
        ShortcutId::LockScreen => binding!(LockScreen),
    }
}

pub(crate) fn shortcut_keys_for(id: &str, overrides: &HashMap<String, String>) -> Option<String> {
    let id = ShortcutId::parse(id)?;
    if let Some(raw) = overrides.get(id.as_str()) {
        return Some(raw.clone());
    }
    SHORTCUT_REGISTRY
        .iter()
        .find(|definition| definition.id == id)
        .map(ShortcutDefinition::default_keys)
}

pub(crate) fn is_default_binding(id: ShortcutId, binding: &ShortcutBinding) -> bool {
    let Some(definition) = SHORTCUT_REGISTRY.iter().find(|item| item.id == id) else {
        return false;
    };
    let defaults = definition.default_binding();
    binding
        .chords()
        .iter()
        .any(|chord| defaults.contains(chord))
}

pub(crate) fn reset_known_overrides(overrides: &mut HashMap<String, String>) {
    overrides.retain(|id, _| ShortcutId::parse(id).is_none());
}

pub(crate) fn conflicting_shortcut(
    pending: &ShortcutBinding,
    exclude: ShortcutId,
    overrides: &HashMap<String, String>,
) -> Option<ShortcutId> {
    let pending_definition = SHORTCUT_REGISTRY.iter().find(|item| item.id == exclude)?;
    let pending_scope = pending_definition.scope;
    let pending_chords = expanded_chords(pending_definition, pending);
    for definition in SHORTCUT_REGISTRY {
        if definition.id == exclude || !pending_scope.overlaps(definition.scope) {
            continue;
        }
        let existing = match overrides.get(definition.id.as_str()) {
            Some(raw) => ShortcutBinding::parse(raw).ok(),
            None => Some(definition.default_binding()),
        };
        let Some(existing) = existing else {
            continue;
        };
        let existing_chords = expanded_chords(&definition, &existing);
        if pending_chords
            .iter()
            .any(|chord| existing_chords.contains(chord))
        {
            return Some(definition.id);
        }
    }
    None
}

fn expanded_chords(
    definition: &ShortcutDefinition,
    binding: &ShortcutBinding,
) -> Vec<ShortcutChord> {
    match definition.kind {
        ShortcutKind::Direct => binding.chords().to_vec(),
        ShortcutKind::IndexedTab { first, last } => binding
            .chords()
            .iter()
            .flat_map(|template| {
                (first..=last).map(move |index| template.with_key(index.to_string()))
            })
            .collect(),
    }
}

pub(crate) fn event_to_hotkey_string(event: &KeyDownEvent) -> Option<String> {
    ShortcutBinding::from_keystroke(event.keystroke.clone())
        .ok()
        .map(|binding| binding.canonical())
}

pub(crate) fn format_hotkey_for_display(keys: &str) -> String {
    ShortcutBinding::parse(keys)
        .map(|binding| {
            binding
                .chords()
                .iter()
                .map(|chord| {
                    chord
                        .canonical()
                        .split('+')
                        .map(format_hotkey_part)
                        .collect::<Vec<_>>()
                        .join("+")
                })
                .collect::<Vec<_>>()
                .join(" / ")
        })
        .unwrap_or_else(|_| keys.to_string())
}

pub(crate) fn hotkey_keystrokes_for_display(keys: &str) -> Option<Vec<Keystroke>> {
    ShortcutBinding::parse(keys)
        .ok()
        .map(|binding| binding.chords.into_iter().map(|chord| chord.0).collect())
}

pub(crate) fn compact_indexed_hotkey_keystrokes_for_display(
    mut keystrokes: Vec<Keystroke>,
) -> Vec<Keystroke> {
    if keystrokes.len() == 1 && keystrokes[0].key == "1" {
        keystrokes[0].key = "1–9".to_string();
        keystrokes[0].key_char = None;
    }
    keystrokes
}

pub(crate) fn is_indexed_shortcut_template(keys: &str) -> bool {
    ShortcutBinding::parse(keys).is_ok_and(|binding| {
        !binding.chords().is_empty()
            && binding
                .chords()
                .iter()
                .all(|chord| chord.keystroke().key == "1")
    })
}

fn parse_chord(source: &str) -> Result<ShortcutChord, String> {
    let mut modifiers = Modifiers::default();
    let parts = source
        .trim()
        .split('+')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.is_empty() {
        return Err("shortcut chord is empty".to_string());
    }
    let mut key = None;
    for part in parts {
        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => modifiers.control = true,
            "cmd" | "command" | "meta" | "super" | "win" | "windows" => modifiers.platform = true,
            "alt" | "option" => modifiers.alt = true,
            "shift" => modifiers.shift = true,
            "fn" | "function" => modifiers.function = true,
            value if key.is_none() => key = Some(normalize_key_name(value)),
            _ => return Err("shortcut chord contains more than one key".to_string()),
        }
    }
    let key = key.ok_or_else(|| "shortcut chord has only modifiers".to_string())?;
    ShortcutChord::new(Keystroke {
        modifiers,
        key,
        key_char: None,
    })
}

fn split_alternatives(source: &str) -> Vec<&str> {
    let trimmed = source.trim();
    if trimmed.ends_with("+,") && trimmed.matches(',').count() == 1 {
        vec![trimmed]
    } else {
        trimmed
            .split(',')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .collect()
    }
}

fn normalize_key_name(key: &str) -> String {
    match key.trim().to_ascii_lowercase().as_str() {
        "comma" | "," => ",".to_string(),
        "backquote" | "grave" | "backtick" | "`" => "`".to_string(),
        "equal" | "equals" | "=" | "plus" => "=".to_string(),
        "return" => "enter".to_string(),
        "esc" => "escape".to_string(),
        "spacebar" => "space".to_string(),
        other => other.to_string(),
    }
}

fn canonical_key_name(key: &str) -> String {
    match normalize_key_name(key).as_str() {
        "," => "comma".to_string(),
        "`" => "backquote".to_string(),
        "=" => "equal".to_string(),
        other => other.to_string(),
    }
}

fn is_modifier_key(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().as_str(),
        "ctrl"
            | "control"
            | "shift"
            | "alt"
            | "option"
            | "meta"
            | "cmd"
            | "command"
            | "super"
            | "win"
            | "windows"
            | "fn"
            | "function"
    )
}

fn format_hotkey_part(part: &str) -> String {
    match part.to_ascii_lowercase().as_str() {
        "ctrl" => "Ctrl".to_string(),
        "meta" => {
            if cfg!(target_os = "macos") {
                "Cmd".to_string()
            } else if cfg!(target_os = "windows") {
                "Win".to_string()
            } else {
                "Super".to_string()
            }
        }
        "alt" => "Alt".to_string(),
        "shift" => "Shift".to_string(),
        "fn" => "Fn".to_string(),
        "comma" => ",".to_string(),
        "backquote" => "`".to_string(),
        "equal" => "=".to_string(),
        key if key.starts_with('f') && key[1..].chars().all(|char| char.is_ascii_digit()) => {
            key.to_ascii_uppercase()
        }
        key if key.len() == 1 => key.to_ascii_uppercase(),
        key => {
            let mut chars = key.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
                .unwrap_or_default()
        }
    }
}

#[cfg(test)]
mod tests {
    use gpui::{KeyContext, Keymap};

    use super::{
        FILE_EXPLORER_KEY_CONTEXT, MODAL_KEY_CONTEXT, OpenSettings, RenameFile, ResolvedKeymap,
        SCREEN_LOCKED_KEY_CONTEXT, SHORTCUT_REGISTRY, ShortcutBinding, ShortcutDiagnosticKind,
        ShortcutId, ShortcutInvocation, ShortcutPlatform, ShortcutScope, SwitchToTab, TerminalCopy,
        WORKSPACE_KEY_CONTEXT, gpui_bindings, is_default_binding, reset_known_overrides,
        resolve_invocation,
    };
    use std::collections::{HashMap, HashSet};

    #[test]
    fn registry_is_complete_unique_and_has_valid_platform_defaults() {
        assert_eq!(SHORTCUT_REGISTRY.len(), 35);
        let mut ids = HashSet::new();
        for definition in SHORTCUT_REGISTRY {
            assert!(ids.insert(definition.id));
            assert_eq!(definition.action, definition.id);
            assert!(!definition.default_binding().chords().is_empty());
            assert_eq!(
                ShortcutId::parse(definition.id.as_str()),
                Some(definition.id)
            );
        }
    }

    #[test]
    fn aliases_and_punctuation_round_trip_to_canonical_format() {
        let cases = [
            ("Control+Shift+C", "ctrl+shift+c"),
            ("Cmd+Shift+C", "meta+shift+c"),
            ("Win+R", "meta+r"),
            ("Super+R", "meta+r"),
            ("Shift+Insert", "shift+insert"),
            ("Ctrl+comma", "ctrl+comma"),
            ("Ctrl+backquote", "ctrl+backquote"),
            ("Ctrl+=", "ctrl+equal"),
            ("F2", "f2"),
            ("Option+R", "alt+r"),
            ("Ctrl+Return", "ctrl+enter"),
        ];
        for (source, canonical) in cases {
            let parsed = ShortcutBinding::parse(source).unwrap();
            assert_eq!(parsed.canonical(), canonical, "{source}");
            assert_eq!(
                ShortcutBinding::parse(canonical).unwrap().canonical(),
                canonical
            );
        }
    }

    #[test]
    fn platform_defaults_do_not_mix_ctrl_and_meta() {
        for definition in SHORTCUT_REGISTRY {
            for platform in [ShortcutPlatform::Windows, ShortcutPlatform::Linux] {
                let binding = definition.default_binding_for(platform);
                assert!(
                    binding
                        .chords()
                        .iter()
                        .all(|chord| !chord.keystroke().modifiers.platform),
                    "{}",
                    definition.id.as_str()
                );
            }
            let mac = definition.default_binding_for(ShortcutPlatform::MacOs);
            assert!(
                definition.id == ShortcutId::ToggleNativeFullscreen
                    || mac
                        .chords()
                        .iter()
                        .all(|chord| !chord.keystroke().modifiers.control),
                "{}",
                definition.id.as_str()
            );
        }
    }

    #[test]
    fn tauri_aligned_defaults_are_platform_specific() {
        let recording = SHORTCUT_REGISTRY
            .iter()
            .find(|item| item.id == ShortcutId::ToggleRecording)
            .unwrap();
        assert_eq!(
            recording
                .default_binding_for(ShortcutPlatform::Windows)
                .canonical(),
            "ctrl+alt+r"
        );
        let close = SHORTCUT_REGISTRY
            .iter()
            .find(|item| item.id == ShortcutId::CloseTab)
            .unwrap();
        assert_eq!(
            close
                .default_binding_for(ShortcutPlatform::MacOs)
                .canonical(),
            "meta+w,meta+shift+w"
        );
    }

    #[test]
    fn invalid_override_disables_and_conflicts_keep_registry_first() {
        let new_session_binding = SHORTCUT_REGISTRY
            .iter()
            .find(|definition| definition.id == ShortcutId::NewSession)
            .expect("new session shortcut must be registered")
            .default_keys();
        let mut overrides = HashMap::new();
        overrides.insert("terminal.copy".to_string(), "ctrl".to_string());
        overrides.insert("tab.close".to_string(), new_session_binding);
        let resolved = ResolvedKeymap::resolve(&overrides);
        assert!(matches!(
            resolved.diagnostic(ShortcutId::TerminalCopy),
            Some(ShortcutDiagnosticKind::Invalid(_))
        ));
        assert!(matches!(
            resolved.diagnostic(ShortcutId::CloseTab),
            Some(ShortcutDiagnosticKind::Conflict(ShortcutId::NewSession))
        ));
        assert!(
            !resolved
                .shortcuts
                .iter()
                .any(|item| { matches!(item.id, ShortcutId::TerminalCopy | ShortcutId::CloseTab) })
        );
    }

    #[test]
    fn scope_overlap_keeps_mutually_exclusive_contexts_separate() {
        assert!(ShortcutScope::Global.overlaps(ShortcutScope::Modal));
        assert!(ShortcutScope::Workspace.overlaps(ShortcutScope::Terminal));
        assert!(ShortcutScope::Workspace.overlaps(ShortcutScope::TextInput));
        assert!(!ShortcutScope::Terminal.overlaps(ShortcutScope::FileExplorer));
        assert!(!ShortcutScope::Modal.overlaps(ShortcutScope::Workspace));
    }

    #[test]
    fn indexed_tab_binding_expands_one_template_to_nine_actions() {
        let resolved = ResolvedKeymap::resolve(&HashMap::new());
        let bindings = gpui_bindings(&resolved);
        let indexed = bindings
            .iter()
            .filter(|binding| binding.action().as_any().is::<SwitchToTab>())
            .collect::<Vec<_>>();
        assert_eq!(indexed.len(), 9);
        assert_eq!(
            indexed[8]
                .action()
                .as_any()
                .downcast_ref::<SwitchToTab>()
                .unwrap()
                .index,
            9
        );
    }

    #[test]
    fn default_detection_accepts_any_default_alternative() {
        let binding = ShortcutBinding::parse(if cfg!(target_os = "macos") {
            "meta+shift+w"
        } else {
            "ctrl+shift+w"
        })
        .unwrap();
        assert!(is_default_binding(ShortcutId::CloseTab, &binding));
    }

    #[test]
    fn unknown_overrides_survive_resolution_and_reset_all() {
        let mut overrides = HashMap::from([
            ("terminal.copy".to_string(), "ctrl".to_string()),
            ("plugin.futureAction".to_string(), " win+Q ".to_string()),
        ]);
        let resolved = ResolvedKeymap::resolve(&overrides);
        assert!(matches!(
            resolved.diagnostic(ShortcutId::TerminalCopy),
            Some(ShortcutDiagnosticKind::Invalid(_))
        ));
        assert_eq!(overrides["terminal.copy"], "ctrl");
        reset_known_overrides(&mut overrides);
        assert_eq!(overrides.len(), 1);
        assert_eq!(overrides["plugin.futureAction"], " win+Q ");
    }

    #[test]
    fn gpui_contexts_dispatch_local_actions_and_allow_workspace_in_inputs() {
        let resolved = ResolvedKeymap::resolve(&HashMap::new());
        let mut keymap = Keymap::new(gpui_bindings(&resolved));

        let terminal_copy = SHORTCUT_REGISTRY
            .iter()
            .find(|item| item.id == ShortcutId::TerminalCopy)
            .unwrap()
            .default_binding()
            .chords()[0]
            .keystroke()
            .clone();
        let terminal_contexts = [
            KeyContext::parse(WORKSPACE_KEY_CONTEXT).unwrap(),
            KeyContext::parse("Terminal").unwrap(),
        ];
        let (actions, pending) = keymap.bindings_for_input(&[terminal_copy], &terminal_contexts);
        assert!(!pending);
        assert!(actions[0].action().partial_eq(&TerminalCopy));

        let open_settings = SHORTCUT_REGISTRY
            .iter()
            .find(|item| item.id == ShortcutId::OpenSettings)
            .unwrap()
            .default_binding()
            .chords()[0]
            .keystroke()
            .clone();
        let workspace = [KeyContext::parse(WORKSPACE_KEY_CONTEXT).unwrap()];
        assert!(
            keymap
                .bindings_for_input(std::slice::from_ref(&open_settings), &workspace)
                .0[0]
                .action()
                .partial_eq(&OpenSettings)
        );
        let input = [
            KeyContext::parse(WORKSPACE_KEY_CONTEXT).unwrap(),
            KeyContext::parse("Input").unwrap(),
        ];
        assert!(
            keymap.bindings_for_input(&[open_settings], &input).0[0]
                .action()
                .partial_eq(&OpenSettings)
        );

        let rename = SHORTCUT_REGISTRY
            .iter()
            .find(|item| item.id == ShortcutId::RenameFile)
            .unwrap()
            .default_binding()
            .chords()[0]
            .keystroke()
            .clone();
        let file_contexts = [
            KeyContext::parse(WORKSPACE_KEY_CONTEXT).unwrap(),
            KeyContext::parse(FILE_EXPLORER_KEY_CONTEXT).unwrap(),
        ];
        assert!(
            keymap.bindings_for_input(&[rename], &file_contexts).0[0]
                .action()
                .partial_eq(&RenameFile)
        );

        // Keep the mutable binding in this test intentional: this also proves a
        // complete replacement remains possible without appending stale entries.
        keymap.clear();
        assert_eq!(keymap.bindings().len(), 0);
    }

    #[test]
    fn invocation_resolution_uses_resolved_overrides_and_runtime_scope() {
        let mut overrides = HashMap::new();
        overrides.insert("tab.quickSwitch".to_string(), "ctrl+alt+k".to_string());
        let resolved = ResolvedKeymap::resolve(&overrides);
        let quick_switch = ShortcutBinding::parse("ctrl+alt+k").unwrap().chords()[0]
            .keystroke()
            .clone();
        let input = [KeyContext::parse("Input").unwrap()];
        assert_eq!(
            resolve_invocation(&resolved, &quick_switch, &input),
            Some(ShortcutInvocation {
                id: ShortcutId::QuickSwitch,
                tab_index: None,
            })
        );

        for blocked in ["Dialog", MODAL_KEY_CONTEXT, SCREEN_LOCKED_KEY_CONTEXT] {
            let contexts = [KeyContext::parse(blocked).unwrap()];
            assert_eq!(
                resolve_invocation(&resolved, &quick_switch, &contexts),
                None
            );
        }

        let old_default = SHORTCUT_REGISTRY
            .iter()
            .find(|item| item.id == ShortcutId::QuickSwitch)
            .unwrap()
            .default_binding()
            .chords()[0]
            .keystroke()
            .clone();
        assert_eq!(resolve_invocation(&resolved, &old_default, &input), None);
    }

    #[test]
    fn invocation_resolution_expands_tab_indices_and_keeps_local_scopes_local() {
        let resolved = ResolvedKeymap::resolve(&HashMap::new());
        let switch_template = resolved
            .shortcuts
            .iter()
            .find(|item| item.id == ShortcutId::SwitchToTab)
            .unwrap()
            .binding
            .chords()[0]
            .with_key("9")
            .0;
        assert_eq!(
            resolve_invocation(&resolved, &switch_template, &[]),
            Some(ShortcutInvocation {
                id: ShortcutId::SwitchToTab,
                tab_index: Some(9),
            })
        );

        let terminal_copy = SHORTCUT_REGISTRY
            .iter()
            .find(|item| item.id == ShortcutId::TerminalCopy)
            .unwrap()
            .default_binding()
            .chords()[0]
            .keystroke()
            .clone();
        assert_eq!(resolve_invocation(&resolved, &terminal_copy, &[]), None);
        assert_eq!(
            resolve_invocation(
                &resolved,
                &terminal_copy,
                &[KeyContext::parse("Terminal").unwrap()],
            ),
            Some(ShortcutInvocation {
                id: ShortcutId::TerminalCopy,
                tab_index: None,
            })
        );

        let rename = SHORTCUT_REGISTRY
            .iter()
            .find(|item| item.id == ShortcutId::RenameFile)
            .unwrap()
            .default_binding()
            .chords()[0]
            .keystroke()
            .clone();
        assert_eq!(resolve_invocation(&resolved, &rename, &[]), None);
        assert_eq!(
            resolve_invocation(
                &resolved,
                &rename,
                &[KeyContext::parse(FILE_EXPLORER_KEY_CONTEXT).unwrap()],
            ),
            Some(ShortcutInvocation {
                id: ShortcutId::RenameFile,
                tab_index: None,
            })
        );
    }
}
