//! Download/preparation outcomes carried unchanged across background job boundaries.
use nyaterm_core::updater::UpdaterError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdatePackageProblem {
    UnexpectedFile,
    UnexpectedDirectory,
    MissingFile,
    UnsafePath,
    Duplicate,
    Invalid,
    Signature,
    Rejected,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateError {
    Download {
        message: String,
    },
    Package {
        problem: UpdatePackageProblem,
        message: String,
        detail: Option<String>,
    },
}

impl UpdateError {
    pub fn download(message: impl ToString) -> Self {
        Self::Download {
            message: message.to_string(),
        }
    }
    pub fn package(problem: UpdatePackageProblem, message: impl Into<String>) -> Self {
        Self::Package {
            problem,
            message: message.into(),
            detail: None,
        }
    }
    pub fn with_detail(self, detail: impl Into<String>) -> Self {
        match self {
            Self::Package {
                problem, message, ..
            } => Self::Package {
                problem,
                message,
                detail: Some(detail.into()),
            },
            download => download,
        }
    }
}
impl std::fmt::Display for UpdateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Download { message } | Self::Package { message, .. } => f.write_str(message),
        }
    }
}
impl std::error::Error for UpdateError {}
impl From<UpdaterError> for UpdateError {
    fn from(error: UpdaterError) -> Self {
        let problem = match &error {
            UpdaterError::MissingSignature => UpdatePackageProblem::Signature,
            UpdaterError::UnsupportedPackageKind => UpdatePackageProblem::Rejected,
            UpdaterError::InvalidVersion { .. }
            | UpdaterError::ManifestParse(_)
            | UpdaterError::MissingPlatforms
            | UpdaterError::ManifestVersionMismatch { .. }
            | UpdaterError::UnsupportedPlatform(_)
            | UpdaterError::UnsupportedArchitecture(_)
            | UpdaterError::MissingArtifact(_)
            | UpdaterError::InvalidArtifactUrl => UpdatePackageProblem::Invalid,
        };
        Self::package(problem, error.to_string())
    }
}
