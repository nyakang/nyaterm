use rust_i18n::t;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConnectionKindTab {
    Ssh,
    Local,
    Telnet,
    Serial,
    Rdp,
    Vnc,
    Spice,
    Adb,
    Ftp,
}

impl ConnectionKindTab {
    pub const ALL: [Self; 9] = [
        Self::Ssh,
        Self::Local,
        Self::Telnet,
        Self::Serial,
        Self::Rdp,
        Self::Vnc,
        Self::Spice,
        Self::Adb,
        Self::Ftp,
    ];

    pub const fn persistence_id(self) -> &'static str {
        match self {
            Self::Ssh => "ssh",
            Self::Local => "local",
            Self::Telnet => "telnet",
            Self::Serial => "serial",
            Self::Rdp => "rdp",
            Self::Vnc => "vnc",
            Self::Spice => "spice",
            Self::Adb => "adb",
            Self::Ftp => "ftp",
        }
    }

    pub fn display_label(self) -> std::borrow::Cow<'static, str> {
        match self {
            Self::Local => t!("dialog.localTerminal"),
            Self::Serial => t!("dialog.serial"),
            _ => std::borrow::Cow::Borrowed(self.label()),
        }
    }

    pub fn catalog_ids() -> Vec<&'static str> {
        Self::ALL.into_iter().map(Self::persistence_id).collect()
    }

    /// Retain the saved connection's type and the active draft even when hidden.
    pub fn editor_types(
        layout: &nyaterm_core::models::settings::connection_type_layout::ConnectionTypeLayout,
        defaults_mode: bool,
        retained: &[Self],
    ) -> Vec<Self> {
        let catalog = Self::catalog_ids();
        let visible = layout.visible(&catalog);
        layout
            .ordered(&catalog)
            .into_iter()
            .filter_map(|id| {
                Self::ALL
                    .into_iter()
                    .find(|kind| kind.persistence_id() == id)
            })
            .filter(|kind| {
                defaults_mode || visible.contains(&kind.persistence_id()) || retained.contains(kind)
            })
            .collect()
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Ssh => "SSH",
            Self::Local => "Local",
            Self::Telnet => "Telnet",
            Self::Serial => "Serial",
            Self::Rdp => "RDP",
            Self::Vnc => "VNC",
            Self::Spice => "SPICE",
            Self::Adb => "Android (ADB)",
            Self::Ftp => "FTP",
        }
    }

    pub fn position_in_all(self) -> Option<usize> {
        Self::ALL.iter().position(|tab| *tab == self)
    }

    pub fn from_connection_type(config: &nyaterm_core::ConnectionType) -> Self {
        match config {
            nyaterm_core::ConnectionType::Ssh { .. } => Self::Ssh,
            nyaterm_core::ConnectionType::LocalTerminal { .. } => Self::Local,
            nyaterm_core::ConnectionType::Telnet { .. } => Self::Telnet,
            nyaterm_core::ConnectionType::Serial { .. } => Self::Serial,
            nyaterm_core::ConnectionType::Rdp { .. } => Self::Rdp,
            nyaterm_core::ConnectionType::Vnc { .. } => Self::Vnc,
            nyaterm_core::ConnectionType::Spice { .. } => Self::Spice,
            nyaterm_core::ConnectionType::Adb { .. } => Self::Adb,
            nyaterm_core::ConnectionType::Ftp { .. } => Self::Ftp,
        }
    }

    pub fn batch_kind(self) -> nyaterm_core::models::connection_batch::ConnectionBatchKind {
        use nyaterm_core::models::connection_batch::ConnectionBatchKind;
        match self {
            Self::Ssh => ConnectionBatchKind::Ssh,
            Self::Local => ConnectionBatchKind::LocalTerminal,
            Self::Telnet => ConnectionBatchKind::Telnet,
            Self::Serial => ConnectionBatchKind::Serial,
            Self::Rdp => ConnectionBatchKind::Rdp,
            Self::Vnc => ConnectionBatchKind::Vnc,
            Self::Spice => ConnectionBatchKind::Spice,
            Self::Adb => ConnectionBatchKind::Adb,
            Self::Ftp => ConnectionBatchKind::Ftp,
        }
    }

    pub fn from_batch_kind(
        kind: nyaterm_core::models::connection_batch::ConnectionBatchKind,
    ) -> Self {
        use nyaterm_core::models::connection_batch::ConnectionBatchKind;
        match kind {
            ConnectionBatchKind::Ssh => Self::Ssh,
            ConnectionBatchKind::LocalTerminal => Self::Local,
            ConnectionBatchKind::Telnet => Self::Telnet,
            ConnectionBatchKind::Serial => Self::Serial,
            ConnectionBatchKind::Rdp => Self::Rdp,
            ConnectionBatchKind::Vnc => Self::Vnc,
            ConnectionBatchKind::Spice => Self::Spice,
            ConnectionBatchKind::Adb => Self::Adb,
            ConnectionBatchKind::Ftp => Self::Ftp,
        }
    }
}

#[cfg(test)]
mod connection_type_layout_tests {
    use super::ConnectionKindTab;
    use nyaterm_core::models::settings::connection_type_layout::ConnectionTypeLayout;

    #[test]
    fn connection_type_layout_retains_original_and_active_types_without_exposing_other_hidden_types()
     {
        let layout: ConnectionTypeLayout = serde_json::from_value(serde_json::json!({
            "order": ["ftp", "vnc", "ssh", "local"],
            "hidden": ["ssh", "telnet", "vnc", "adb", "serial"]
        }))
        .unwrap();
        assert_eq!(
            ConnectionKindTab::editor_types(&layout, false, &[]),
            [
                ConnectionKindTab::Ftp,
                ConnectionKindTab::Local,
                ConnectionKindTab::Rdp,
                ConnectionKindTab::Spice
            ]
        );
        assert_eq!(
            ConnectionKindTab::editor_types(
                &layout,
                false,
                &[ConnectionKindTab::Vnc, ConnectionKindTab::Ssh]
            ),
            [
                ConnectionKindTab::Ftp,
                ConnectionKindTab::Vnc,
                ConnectionKindTab::Ssh,
                ConnectionKindTab::Local,
                ConnectionKindTab::Rdp,
                ConnectionKindTab::Spice
            ]
        );
        let defaults = ConnectionKindTab::editor_types(&layout, true, &[]);
        assert_eq!(defaults.len(), ConnectionKindTab::ALL.len());
        assert_eq!(defaults[0], ConnectionKindTab::Ftp);
    }
}

/// Navigation state belongs only to the editor draft, never to saved connections.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConnectionEditorSection {
    Basic,
    AndroidScreen,
    Network,
    Terminal,
    Sftp,
    Forwarding,
    PostLogin,
    Algorithms,
    AutoLogin,
    Compatibility,
    Security,
    Display,
    Session,
    Serial,
    Charset,
    Advanced,
}

impl ConnectionEditorSection {
    /// Sections offered by the editor. "Default session settings" reuses the
    /// same surface without the basic-info tab, because identity fields never
    /// carry over between connections.
    pub fn for_kind(kind: ConnectionKindTab, defaults_mode: bool) -> &'static [Self] {
        let sections = Self::for_kind_all_tabs(kind);
        if defaults_mode {
            match kind {
                ConnectionKindTab::Serial => &[Self::Serial, Self::Terminal],
                _ => &sections[1..],
            }
        } else {
            sections
        }
    }

    fn for_kind_all_tabs(kind: ConnectionKindTab) -> &'static [Self] {
        match kind {
            ConnectionKindTab::Adb => &[Self::Basic, Self::AndroidScreen],
            ConnectionKindTab::Ssh => &[
                Self::Basic,
                Self::Network,
                Self::Terminal,
                Self::Sftp,
                Self::Forwarding,
                Self::PostLogin,
                Self::Algorithms,
            ],
            ConnectionKindTab::Telnet => &[
                Self::Basic,
                Self::Network,
                Self::AutoLogin,
                Self::Terminal,
                Self::Compatibility,
            ],
            ConnectionKindTab::Rdp | ConnectionKindTab::Vnc | ConnectionKindTab::Spice => &[
                Self::Basic,
                Self::Network,
                Self::Security,
                Self::Display,
                Self::Session,
            ],
            ConnectionKindTab::Serial => &[Self::Basic, Self::Terminal],
            ConnectionKindTab::Local => &[Self::Basic, Self::Terminal],
            ConnectionKindTab::Ftp => &[Self::Basic, Self::Charset, Self::Advanced],
        }
    }

    pub fn initial_for_kind(kind: ConnectionKindTab, defaults_mode: bool) -> Self {
        if defaults_mode {
            Self::for_kind(kind, true)
                .first()
                .copied()
                .unwrap_or(Self::Basic)
        } else {
            Self::Basic
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::Basic => "basic",
            Self::AndroidScreen => "androidScreen",
            Self::Network => "network",
            Self::Terminal => "terminal",
            Self::Sftp => "sftp",
            Self::Forwarding => "forwarding",
            Self::PostLogin => "postLogin",
            Self::Algorithms => "algorithms",
            Self::AutoLogin => "autoLogin",
            Self::Compatibility => "compatibility",
            Self::Security => "security",
            Self::Display => "display",
            Self::Session => "session",
            Self::Serial => "serial",
            Self::Charset => "charset",
            Self::Advanced => "advanced",
        }
    }

    pub fn label(self) -> std::borrow::Cow<'static, str> {
        match self {
            Self::Basic => t!("dialog.editorSections.basic"),
            Self::AndroidScreen => t!("android.screenSettings"),
            Self::Network => t!("dialog.editorSections.network"),
            Self::Terminal => t!("dialog.editorSections.terminal"),
            Self::Sftp => t!("dialog.editorSections.sftp"),
            Self::Forwarding => t!("dialog.editorSections.forwarding"),
            Self::PostLogin => t!("dialog.editorSections.postLogin"),
            Self::Algorithms => t!("dialog.editorSections.algorithms"),
            Self::AutoLogin => t!("dialog.editorSections.autoLogin"),
            Self::Compatibility => t!("dialog.editorSections.compatibility"),
            Self::Security => t!("dialog.editorSections.security"),
            Self::Display => t!("dialog.editorSections.display"),
            Self::Session => t!("dialog.editorSections.session"),
            Self::Serial => t!("dialog.editorSections.serial"),
            Self::Charset => t!("dialog.editorSections.charset"),
            Self::Advanced => t!("dialog.editorSections.advanced"),
        }
    }
}
