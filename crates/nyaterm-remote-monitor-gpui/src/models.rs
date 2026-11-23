#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteProcessSortKey {
    Cpu,
    Memory,
    Pid,
    User,
    Command,
}

impl RemoteProcessSortKey {
    pub fn label(self) -> &'static str {
        match self {
            Self::Cpu => "CPU",
            Self::Memory => "Memory",
            Self::Pid => "PID",
            Self::User => "User",
            Self::Command => "Command",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteProcessSortDirection {
    Ascending,
    Descending,
}

impl RemoteProcessSortDirection {
    pub fn marker(self) -> &'static str {
        match self {
            Self::Ascending => "↑",
            Self::Descending => "↓",
        }
    }

    pub fn reversed(self) -> Self {
        match self {
            Self::Ascending => Self::Descending,
            Self::Descending => Self::Ascending,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DockerConfirmAction {
    ContainerAction {
        container_id: String,
        action: &'static str,
    },
    ImageRemove {
        image_id: String,
        force: bool,
    },
    VolumeRemove {
        volume_name: String,
        force: bool,
    },
    NetworkRemove {
        network_id: String,
    },
    ComposeAction {
        project_name: String,
        config_files: Option<String>,
        action: &'static str,
    },
    Prune {
        volumes: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockerConfirmState {
    pub title: String,
    pub detail: String,
    pub action: DockerConfirmAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DockerTab {
    Containers,
    Images,
    Volumes,
    Networks,
    Compose,
}

impl DockerTab {
    pub fn label(self) -> &'static str {
        match self {
            Self::Containers => "Containers",
            Self::Images => "Images",
            Self::Volumes => "Volumes",
            Self::Networks => "Networks",
            Self::Compose => "Compose",
        }
    }
}
