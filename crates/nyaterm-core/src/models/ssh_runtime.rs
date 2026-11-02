use serde::{Deserialize, Serialize};

/// A per-session capability policy, independent of the saved device profile.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum SshRuntimeMode {
    #[default]
    Standard,
    Terminal,
    Sftp,
}

impl SshRuntimeMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "standard" => Some(Self::Standard),
            "terminal" => Some(Self::Terminal),
            "sftp" => Some(Self::Sftp),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Terminal => "terminal",
            Self::Sftp => "sftp",
        }
    }
}
