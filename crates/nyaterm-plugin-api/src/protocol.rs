//! Versioned host contracts. Session handles identify one connection generation,
//! never the currently selected tab. Long operations return task handles.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    Network,
    LocalFilesRead,
    LocalFilesWrite,
    RemoteFilesRead,
    RemoteFilesWrite,
    SessionsRead,
    TerminalRead,
    TerminalWrite,
    RemoteExecute,
    ProcessExecute,
    Storage,
    Ui,
}

impl Capability {
    pub fn parse(value: &str) -> Result<Self, &'static str> {
        serde_json::from_value(serde_json::Value::String(value.to_owned()))
            .map_err(|_| "Unknown extension capability")
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionHandle {
    pub id: String,
    pub generation: u64,
    #[serde(default)]
    pub connection_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Scope {
    Plugin,
    Origin { origin: String },
    Directory { path: String },
    Connection { id: String },
    Executable { path: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Grant {
    pub capability: Capability,
    pub scope: Scope,
}

/// Bound to immutable package provenance, not just a mutable display name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Authorization {
    pub plugin_id: String,
    pub source: String,
    pub grants: Vec<Grant>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum HostRequest {
    Http {
        url: String,
        method: String,
        body: Vec<u8>,
    },
    LocalRead {
        root: String,
        path: String,
        offset: u64,
        length: u32,
    },
    LocalWrite {
        root: String,
        path: String,
        offset: u64,
        bytes: Vec<u8>,
    },
    RemoteRead {
        session: SessionHandle,
        path: String,
        offset: u64,
        length: u32,
    },
    RemoteWrite {
        session: SessionHandle,
        path: String,
        offset: u64,
        bytes: Vec<u8>,
    },
    Sessions,
    TerminalSnapshot {
        session: SessionHandle,
        lines: u32,
    },
    TerminalWrite {
        session: SessionHandle,
        bytes: Vec<u8>,
    },
    RemoteExecute {
        session: SessionHandle,
        command: String,
    },
    ProcessExecute {
        executable: String,
        arguments: Vec<String>,
    },
    StorageRead {
        key: String,
    },
    StorageWrite {
        key: String,
        bytes: Vec<u8>,
    },
    Notify {
        #[serde(default)]
        session: Option<SessionHandle>,
        message: String,
    },
    ShowPanel {
        #[serde(default)]
        session: Option<SessionHandle>,
        id: String,
        title: String,
        elements: Vec<PanelElement>,
    },
}

impl HostRequest {
    pub fn capability(&self) -> Capability {
        match self {
            Self::Http { .. } => Capability::Network,
            Self::LocalRead { .. } => Capability::LocalFilesRead,
            Self::LocalWrite { .. } => Capability::LocalFilesWrite,
            Self::RemoteRead { .. } => Capability::RemoteFilesRead,
            Self::RemoteWrite { .. } => Capability::RemoteFilesWrite,
            Self::Sessions => Capability::SessionsRead,
            Self::TerminalSnapshot { .. } => Capability::TerminalRead,
            Self::TerminalWrite { .. } => Capability::TerminalWrite,
            Self::RemoteExecute { .. } => Capability::RemoteExecute,
            Self::ProcessExecute { .. } => Capability::ProcessExecute,
            Self::StorageRead { .. } | Self::StorageWrite { .. } => Capability::Storage,
            Self::Notify { .. } | Self::ShowPanel { .. } => Capability::Ui,
        }
    }

    pub fn session(&self) -> Option<&SessionHandle> {
        match self {
            Self::RemoteRead { session, .. }
            | Self::RemoteWrite { session, .. }
            | Self::TerminalSnapshot { session, .. }
            | Self::TerminalWrite { session, .. }
            | Self::RemoteExecute { session, .. } => Some(session),
            Self::Notify { session, .. } | Self::ShowPanel { session, .. } => session.as_ref(),
            _ => None,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum TaskEvent {
    Pending,
    Output {
        bytes: Vec<u8>,
    },
    Completed {
        bytes: Vec<u8>,
        exit_code: Option<i32>,
    },
    Failed {
        code: String,
    },
    Cancelled,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Contributions {
    #[serde(default)]
    pub terminal_menu: Vec<String>,
    #[serde(default)]
    pub file_menu: Vec<String>,
    #[serde(default)]
    pub session_connected: Vec<String>,
    #[serde(default)]
    pub session_disconnected: Vec<String>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PanelElement {
    Text {
        text: String,
    },
    Log {
        text: String,
    },
    Table {
        columns: Vec<String>,
        rows: Vec<Vec<String>>,
    },
    Input {
        id: String,
        label: String,
        value: String,
    },
    Button {
        action: String,
        label: String,
    },
    Progress {
        label: String,
        completed: u64,
        total: u64,
    },
}
