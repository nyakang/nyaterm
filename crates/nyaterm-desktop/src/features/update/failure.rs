//! Classifies the single string reported by an update job.
//!
//! Download, signature, and portable-archive checks all finish as
//! `DownloadState::Failed(String)`. The dialog uses this split to decide
//! whether downloading the same artifact again can succeed.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::features) enum UpdateFailureKind {
    Check,
    Download,
    Package,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::features) enum UpdatePackageProblem {
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
pub(in crate::features) struct UpdateFailure {
    pub(in crate::features) kind: UpdateFailureKind,
    problem: Option<UpdatePackageProblem>,
    detail: Option<String>,
    raw: String,
}

impl UpdateFailure {
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
        if let Some(detail) = self.detail.as_deref().filter(|detail| !detail.is_empty()) {
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

pub(in crate::features) fn classify_update_failure(download: bool, message: &str) -> UpdateFailure {
    if !download {
        return UpdateFailure {
            kind: UpdateFailureKind::Check,
            problem: None,
            detail: None,
            raw: message.to_string(),
        };
    }
    if let Some((problem, detail)) = package_problem(message) {
        return UpdateFailure {
            kind: UpdateFailureKind::Package,
            problem: Some(problem),
            detail,
            raw: message.to_string(),
        };
    }
    UpdateFailure {
        kind: UpdateFailureKind::Download,
        problem: None,
        detail: None,
        raw: message.to_string(),
    }
}

fn package_problem(message: &str) -> Option<(UpdatePackageProblem, Option<String>)> {
    const PREFIXES: &[(&str, UpdatePackageProblem)] = &[
        (
            "portable update archive contains an unexpected file: ",
            UpdatePackageProblem::UnexpectedFile,
        ),
        (
            "portable update archive contains an unexpected directory: ",
            UpdatePackageProblem::UnexpectedDirectory,
        ),
        (
            "portable update archive contains a duplicate entry: ",
            UpdatePackageProblem::Duplicate,
        ),
        (
            "portable update archive is missing ",
            UpdatePackageProblem::MissingFile,
        ),
        (
            "portable update archive entry has an invalid size: ",
            UpdatePackageProblem::Invalid,
        ),
        ("failed to stage ", UpdatePackageProblem::Invalid),
        ("failed to extract ", UpdatePackageProblem::Invalid),
    ];
    for (prefix, problem) in PREFIXES {
        if let Some(rest) = message.strip_prefix(prefix) {
            let rest = rest.trim();
            let detail = (!rest.is_empty()).then(|| rest.to_string());
            return Some((*problem, detail));
        }
    }

    const EXACT: &[(&str, UpdatePackageProblem)] = &[
        (
            "portable update archive contains an unsafe path",
            UpdatePackageProblem::UnsafePath,
        ),
        (
            "portable update archive contains a symbolic link",
            UpdatePackageProblem::UnsafePath,
        ),
        (
            "portable update archive has an unexpected root directory",
            UpdatePackageProblem::UnexpectedDirectory,
        ),
        (
            "portable update archive contains a duplicate entry",
            UpdatePackageProblem::Duplicate,
        ),
        (
            "portable update archive contains too many entries",
            UpdatePackageProblem::Invalid,
        ),
        (
            "portable update archive is too large",
            UpdatePackageProblem::Invalid,
        ),
        (
            "portable update archive has an inconsistent directory",
            UpdatePackageProblem::Invalid,
        ),
        ("missing payload parent", UpdatePackageProblem::Invalid),
        (
            "update signature verification failed",
            UpdatePackageProblem::Signature,
        ),
        (
            "invalid update signature encoding",
            UpdatePackageProblem::Signature,
        ),
        ("invalid update signature", UpdatePackageProblem::Signature),
        (
            "unsupported update signature",
            UpdatePackageProblem::Signature,
        ),
        ("invalid public key", UpdatePackageProblem::Signature),
        (
            "update artifact signature is missing",
            UpdatePackageProblem::Signature,
        ),
        ("update too large", UpdatePackageProblem::Invalid),
        ("update manifest too large", UpdatePackageProblem::Invalid),
        (
            "update belongs to a different application flavor",
            UpdatePackageProblem::Rejected,
        ),
        (
            "portable updates are only supported on Windows",
            UpdatePackageProblem::Rejected,
        ),
        (
            "this installation must be updated manually",
            UpdatePackageProblem::Rejected,
        ),
        (
            "use Homebrew to update this installation",
            UpdatePackageProblem::Rejected,
        ),
        ("not an application bundle", UpdatePackageProblem::Rejected),
        (
            "application bundle belongs to a different identity",
            UpdatePackageProblem::Rejected,
        ),
        (
            "unsupported bundle archive entry",
            UpdatePackageProblem::Rejected,
        ),
        (
            "invalid bundle archive path",
            UpdatePackageProblem::Rejected,
        ),
        (
            "portable installation marker is missing",
            UpdatePackageProblem::Rejected,
        ),
        (
            "the selected update package is not supported on this platform",
            UpdatePackageProblem::Rejected,
        ),
    ];
    if let Some((_, problem)) = EXACT.iter().find(|(text, _)| *text == message) {
        return Some((*problem, None));
    }

    const INVALID_PREFIXES: &[&str] = &[
        "update manifest is invalid",
        "update manifest does not contain",
        "update manifest version mismatch",
        "update artifact URL does not match",
        "unsupported update platform",
        "unsupported update architecture",
        "invalid application version",
    ];
    INVALID_PREFIXES
        .iter()
        .any(|prefix| message.starts_with(prefix))
        .then_some((UpdatePackageProblem::Invalid, None))
}

#[cfg(test)]
mod tests {
    use super::{UpdateFailureKind, UpdatePackageProblem, classify_update_failure};

    #[test]
    fn unexpected_portable_file_names_the_path_and_is_not_a_download_retry() {
        let failure = classify_update_failure(
            true,
            r"portable update archive contains an unexpected file: conpty\LICENSE.txt",
        );

        assert_eq!(failure.kind, UpdateFailureKind::Package);
        assert_eq!(failure.problem, Some(UpdatePackageProblem::UnexpectedFile));
        assert_eq!(failure.visible_detail(), Some(r"conpty\LICENSE.txt"));
        assert_eq!(failure.title_key(), "updater.couldNotInstall");
        assert_eq!(failure.summary_key(), "updater.packageUnexpectedFile");
    }

    #[test]
    fn signature_and_archive_rejections_hide_the_english_sentence() {
        let signature = classify_update_failure(true, "update signature verification failed");
        assert_eq!(signature.kind, UpdateFailureKind::Package);
        assert_eq!(signature.summary_key(), "updater.packageSignature");
        assert_eq!(signature.visible_detail(), None);

        let missing =
            classify_update_failure(true, "portable update archive is missing nyaterm.exe");
        assert_eq!(missing.problem, Some(UpdatePackageProblem::MissingFile));
        assert_eq!(missing.visible_detail(), Some("nyaterm.exe"));
    }

    #[test]
    fn install_method_rejections_keep_the_original_reason() {
        let failure = classify_update_failure(true, "use Homebrew to update this installation");
        assert_eq!(failure.kind, UpdateFailureKind::Package);
        assert_eq!(failure.problem, Some(UpdatePackageProblem::Rejected));
        assert_eq!(
            failure.visible_detail(),
            Some("use Homebrew to update this installation")
        );
    }

    #[test]
    fn truncated_and_transport_errors_stay_retryable() {
        let truncated = classify_update_failure(true, "update download truncated");
        assert_eq!(truncated.kind, UpdateFailureKind::Download);
        assert_eq!(truncated.title_key(), "updater.downloadIncompleteTitle");
        assert_eq!(truncated.summary_key(), "updater.downloadFailed");
        assert_eq!(
            truncated.visible_detail(),
            Some("update download truncated")
        );

        let transport = classify_update_failure(
            true,
            "error sending request for url (https://downloads.nyaterm.app)",
        );
        assert_eq!(transport.kind, UpdateFailureKind::Download);
    }

    #[test]
    fn check_failures_stay_separate_from_package_rejection() {
        let failure = classify_update_failure(false, "offline");
        assert_eq!(failure.kind, UpdateFailureKind::Check);
        assert_eq!(failure.title_key(), "updater.checkFailedTitle");
        assert_eq!(failure.summary_key(), "updater.checkFailed");
        assert_eq!(failure.visible_detail(), Some("offline"));
    }
}
