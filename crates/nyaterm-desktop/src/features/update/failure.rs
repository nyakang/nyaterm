//! Maps typed update failures to desktop copy and actions.
use nyaterm_updater::error::{UpdateError, UpdatePackageProblem};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::features) enum UpdateFailureKind {
    Check,
    Download,
    Package,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct UpdateFailure {
    pub(in crate::features) kind: UpdateFailureKind,
    problem: Option<UpdatePackageProblem>,
    detail: Option<String>,
    raw: String,
}

impl UpdateFailure {
    pub(in crate::features) fn check(message: impl Into<String>) -> Self {
        Self {
            kind: UpdateFailureKind::Check,
            problem: None,
            detail: None,
            raw: message.into(),
        }
    }
    pub(in crate::features) fn from_update(error: UpdateError) -> Self {
        match error {
            UpdateError::Download { message } => Self {
                kind: UpdateFailureKind::Download,
                problem: None,
                detail: None,
                raw: message,
            },
            UpdateError::Package {
                problem,
                message,
                detail,
            } => Self {
                kind: UpdateFailureKind::Package,
                problem: Some(problem),
                detail,
                raw: message,
            },
        }
    }

    pub(in crate::features) fn title_key(&self) -> &'static str {
        match self.kind {
            UpdateFailureKind::Check => "updater.checkFailedTitle",
            UpdateFailureKind::Download => "updater.downloadIncompleteTitle",
            UpdateFailureKind::Package => "updater.couldNotInstall",
        }
    }

    pub(in crate::features) fn summary_key(&self) -> &'static str {
        match self.problem {
            Some(UpdatePackageProblem::UnexpectedFile) => "updater.packageUnexpectedFile",
            Some(UpdatePackageProblem::UnexpectedDirectory) => "updater.packageUnexpectedDirectory",
            Some(UpdatePackageProblem::MissingFile) => "updater.packageMissingFile",
            Some(UpdatePackageProblem::UnsafePath) => "updater.packageUnsafe",
            Some(UpdatePackageProblem::Duplicate) => "updater.packageDuplicate",
            Some(UpdatePackageProblem::Invalid) => "updater.packageInvalid",
            Some(UpdatePackageProblem::Signature) => "updater.packageSignature",
            Some(UpdatePackageProblem::Rejected) => "updater.packageRejected",
            None => match self.kind {
                UpdateFailureKind::Check => "updater.checkFailed",
                UpdateFailureKind::Download => "updater.downloadFailed",
                UpdateFailureKind::Package => "updater.packageRejected",
            },
        }
    }

    /// Short fact to show under the summary. Package outcomes whose summary
    /// already says the whole thing stay empty; a rejected install method still
    /// surfaces the original reason.
    pub(in crate::features) fn visible_detail(&self) -> Option<&str> {
        if let Some(detail) = self
            .detail
            .as_deref()
            .map(str::trim)
            .filter(|detail| !detail.is_empty())
        {
            return Some(detail);
        }
        match self.problem {
            Some(UpdatePackageProblem::Rejected) | None => {
                let raw = self.raw.trim();
                (!raw.is_empty()).then_some(raw)
            }
            Some(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{UpdateFailure, UpdateFailureKind};
    use nyaterm_updater::error::{UpdateError, UpdatePackageProblem};

    #[test]
    fn unexpected_portable_file_names_the_path_and_is_not_a_download_retry() {
        let failure = UpdateFailure::from_update(
            UpdateError::package(UpdatePackageProblem::UnexpectedFile, "source diagnostic")
                .with_detail(r"conpty\LICENSE.txt"),
        );

        assert_eq!(failure.kind, UpdateFailureKind::Package);
        assert_eq!(failure.problem, Some(UpdatePackageProblem::UnexpectedFile));
        assert_eq!(failure.visible_detail(), Some(r"conpty\LICENSE.txt"));
        assert_eq!(failure.title_key(), "updater.couldNotInstall");
        assert_eq!(failure.summary_key(), "updater.packageUnexpectedFile");
    }

    #[test]
    fn signature_and_archive_rejections_hide_the_english_sentence() {
        let signature = UpdateFailure::from_update(UpdateError::package(
            UpdatePackageProblem::Signature,
            "source diagnostic",
        ));
        assert_eq!(signature.kind, UpdateFailureKind::Package);
        assert_eq!(signature.summary_key(), "updater.packageSignature");
        assert_eq!(signature.visible_detail(), None);

        let missing = UpdateFailure::from_update(
            UpdateError::package(UpdatePackageProblem::MissingFile, "source diagnostic")
                .with_detail("nyaterm.exe"),
        );
        assert_eq!(missing.problem, Some(UpdatePackageProblem::MissingFile));
        assert_eq!(missing.visible_detail(), Some("nyaterm.exe"));
    }

    #[test]
    fn install_method_rejections_keep_the_original_reason() {
        let failure = UpdateFailure::from_update(UpdateError::package(
            UpdatePackageProblem::Rejected,
            "use Homebrew to update this installation",
        ));
        assert_eq!(failure.kind, UpdateFailureKind::Package);
        assert_eq!(failure.problem, Some(UpdatePackageProblem::Rejected));
        assert_eq!(
            failure.visible_detail(),
            Some("use Homebrew to update this installation")
        );
    }

    #[test]
    fn truncated_and_transport_errors_stay_retryable() {
        let truncated =
            UpdateFailure::from_update(UpdateError::download("update download truncated"));
        assert_eq!(truncated.kind, UpdateFailureKind::Download);
        assert_eq!(truncated.title_key(), "updater.downloadIncompleteTitle");
        assert_eq!(truncated.summary_key(), "updater.downloadFailed");
        assert_eq!(
            truncated.visible_detail(),
            Some("update download truncated")
        );

        let transport = UpdateFailure::from_update(UpdateError::download(
            "error sending request for url (https://downloads.nyaterm.app)",
        ));
        assert_eq!(transport.kind, UpdateFailureKind::Download);
    }

    #[test]
    fn check_failures_stay_separate_from_package_rejection() {
        let failure = UpdateFailure::check("offline");
        assert_eq!(failure.kind, UpdateFailureKind::Check);
        assert_eq!(failure.title_key(), "updater.checkFailedTitle");
        assert_eq!(failure.summary_key(), "updater.checkFailed");
        assert_eq!(failure.visible_detail(), Some("offline"));
    }
}
