//! Presentation-only localization for Android backend errors and status codes.
use nyaterm_android::adb::client::DeviceStatus;
use nyaterm_android::error::AdbError;
use nyaterm_core::models::android::AndroidScreenValidationError;
use rust_i18n::t;
use std::borrow::Cow;

pub(in crate::features) fn device_status_label(status: DeviceStatus) -> Cow<'static, str> {
    match status {
        DeviceStatus::Online => t!("android.deviceStatus.online"),
        DeviceStatus::Offline => t!("android.deviceStatus.offline"),
        DeviceStatus::Unauthorized => t!("android.deviceStatus.unauthorized"),
        DeviceStatus::Unavailable => t!("android.deviceStatus.unavailable"),
    }
}

pub(in crate::features) fn screen_validation_message(
    error: AndroidScreenValidationError,
) -> String {
    match error {
        AndroidScreenValidationError::MaxSize => t!("android.errors.maxSize"),
        AndroidScreenValidationError::MaxFps => t!("android.errors.maxFps"),
        AndroidScreenValidationError::BitRate => t!("android.errors.bitRate"),
    }
    .into_owned()
}

pub(super) enum AndroidError {
    Backend(AdbError),
    Fatal(String),
    Disconnected,
    TextureUpload,
    JobFailed,
}

impl AndroidError {
    pub(super) fn code(&self) -> &'static str {
        match self {
            Self::Backend(error) => match error {
                AdbError::Cancelled => "Cancelled",
                AdbError::Timeout => "Timeout",
                AdbError::Offline => "Offline",
                AdbError::Unauthorized => "Unauthorized",
                AdbError::ExecutableMissing => "ExecutableMissing",
                AdbError::IncompatibleServer => "IncompatibleServer",
                AdbError::HelperVersionMismatch => "HelperVersionMismatch",
                AdbError::FileMissing => "FileMissing",
                AdbError::Rejected => "Rejected",
                AdbError::Protocol(_) => "Protocol",
                AdbError::Io(_) => "Io",
                AdbError::ShellExit(_) => "ShellExit",
            },
            Self::Fatal(code) => match code.as_str() {
                "DeviceDisconnected" => "Offline",
                "HelperVersionMismatch" => "HelperVersionMismatch",
                "HelperInputFailed" => "HelperInputFailed",
                "HelperHung" | "DecoderHung" | "PresentationHung" => "HelperHung",
                "HelperExited" => "HelperExited",
                "ServerExited" => "ServerExited",
                "DecoderPanic" | "PresentationPanic" => "DecoderPanic",
                _ => "ScreenFailed",
            },
            Self::Disconnected => "Offline",
            Self::TextureUpload => "TextureUpload",
            Self::JobFailed => "JobFailed",
        }
    }
    pub(super) fn message(&self) -> String {
        match self {
            Self::Backend(error) => adb_error_message(error),
            Self::Fatal(code) => fatal_message(code).into_owned(),
            Self::Disconnected => t!("android.errors.disconnected").into_owned(),
            Self::TextureUpload => t!("android.errors.textureUpload").into_owned(),
            Self::JobFailed => t!("android.errors.operationFailed").into_owned(),
        }
    }
}

pub(in crate::features) fn adb_error_message(error: &AdbError) -> String {
    match error {
        AdbError::FileMissing => t!("android.errors.fileMissing"),
        AdbError::Io(_) => t!("android.errors.io"),
        AdbError::ExecutableMissing => t!("android.errors.executableMissing"),
        AdbError::IncompatibleServer => t!("android.errors.incompatibleServer"),
        AdbError::HelperVersionMismatch => t!("android.errors.helperVersion"),
        AdbError::Unauthorized => t!("android.errors.unauthorized"),
        AdbError::Offline => t!("android.errors.offline"),
        AdbError::Rejected => t!("android.errors.rejected"),
        AdbError::Protocol(_) => t!("android.errors.protocol"),
        AdbError::Cancelled => t!("android.errors.cancelled"),
        AdbError::Timeout => t!("android.errors.timeout"),
        AdbError::ShellExit(code) => t!("android.errors.shellExit", code = code),
    }
    .into_owned()
}

pub(super) fn input_rejection_message(code: nyaterm_android::ipc::InputRejection) -> String {
    use nyaterm_android::ipc::InputRejection;
    match code {
        InputRejection::TextTooLong => t!("android.errors.inputTextTooLong"),
        InputRejection::Nul => t!("android.errors.inputNul"),
        InputRejection::PacketTooLong => t!("android.errors.inputPacketTooLong"),
        InputRejection::InvalidInput => t!("android.errors.inputInvalid"),
    }
    .into_owned()
}

pub(in crate::features) fn fatal_message(code: &str) -> Cow<'static, str> {
    match code {
        "DeviceDisconnected" => t!("android.errors.disconnected"),
        "HelperVersionMismatch" => t!("android.errors.helperVersion"),
        "HelperInputFailed" => t!("android.errors.inputFailed"),
        "HelperHung" | "DecoderHung" | "PresentationHung" => t!("android.errors.helperHung"),
        "HelperExited" => t!("android.errors.helperExited"),
        "ServerExited" | "VideoEnded" => t!("android.errors.videoEnded"),
        "ServerProtocolFailed" | "HelperProtocolFailed" => t!("android.errors.protocol"),
        "DecoderPanic" | "VideoFailed" | "PresentationPanic" | "PresentationFailed"
        | "HelperPanic" | "HelperFailed" => {
            t!("android.errors.decoderFailed")
        }
        _ => t!("android.errors.operationFailed"),
    }
}

/// Preserve unrelated backends' diagnostics; Android errors never expose raw device output.
pub(in crate::features) fn file_error_message(error: &anyhow::Error) -> String {
    if let Some(error) = error.chain().find_map(|cause| {
        cause.downcast_ref::<nyaterm_files::file_browser::FileTransferEntryError>()
    }) {
        use nyaterm_files::file_browser::FileTransferEntryError;
        return match error {
            FileTransferEntryError::Unavailable => t!("fileExplorer.transferSourceUnavailable"),
            FileTransferEntryError::UnsupportedBackend => {
                t!("fileExplorer.transferBackendUnsupported")
            }
            FileTransferEntryError::InvalidDestination => {
                t!("fileExplorer.transferInvalidDestination")
            }
            FileTransferEntryError::PickerFailed => t!("fileExplorer.transferPickerFailed"),
            FileTransferEntryError::PickerClosed => t!("fileExplorer.transferPickerClosed"),
            FileTransferEntryError::PickerAlreadyOpen => {
                t!("fileExplorer.transferPickerAlreadyOpen")
            }
            FileTransferEntryError::DragPlatformUnsupported => {
                t!("fileExplorer.dragPlatformUnsupported")
            }
            FileTransferEntryError::NoSelection => t!("fileExplorer.transferNoSelection"),
            FileTransferEntryError::UnsupportedFileType => {
                t!("fileExplorer.transferUnsupportedFileType")
            }
            FileTransferEntryError::SourceChanged => t!("fileExplorer.transferSourceChanged"),
            FileTransferEntryError::LimitExceeded => t!("fileExplorer.transferLimitExceeded"),
            FileTransferEntryError::TargetExists => t!("fileExplorer.transferTargetExists"),
        }
        .into_owned();
    }
    if let Some(error) = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<nyaterm_files::adb::AdbFileValidationError>())
    {
        use nyaterm_files::adb::AdbFileValidationError;
        return match error {
            AdbFileValidationError::InvalidPath => t!("android.errors.invalidFilePath"),
            AdbFileValidationError::InvalidPathToken => t!("android.errors.invalidPathToken"),
            AdbFileValidationError::InvalidFilename => t!("android.errors.invalidFilename"),
        }
        .into_owned();
    }
    if let Some(error) = error.downcast_ref::<nyaterm_files::adb::AdbFilePolicyError>() {
        use nyaterm_files::adb::AdbFilePolicyError;
        return match error {
            AdbFilePolicyError::AttributesUnavailable => t!("android.errors.fileAttributes"),
            AdbFilePolicyError::PermissionsUnavailable => t!("android.errors.filePermissions"),
            AdbFilePolicyError::TimestampUnavailable => t!("android.errors.fileTimestamp"),
            AdbFilePolicyError::EditLimitExceeded => t!("android.errors.editLimit"),
            AdbFilePolicyError::RevisionRequired => t!("android.errors.revisionRequired"),
        }
        .into_owned();
    }
    if let Some(error) = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<AdbError>())
    {
        return adb_error_message(error);
    }
    if error
        .downcast_ref::<nyaterm_files::adb::AdbFileError>()
        .is_some()
    {
        return t!("android.errors.fileOperationFailed").into_owned();
    }
    if error
        .downcast_ref::<nyaterm_transport::SessionError>()
        .is_some_and(|error| matches!(error, nyaterm_transport::SessionError::Adb(_)))
    {
        return t!("android.errors.operationFailed").into_owned();
    }
    error.to_string()
}

/// Classify failures without formatting the error chain, which can contain user data.
pub(in crate::features) fn file_error_code(error: &anyhow::Error) -> &'static str {
    use nyaterm_files::adb::AdbFileValidationError;
    // anyhow contexts support typed downcasts through Error, but the standard
    // source chain only exposes the wrapper and its underlying cause.
    if let Some(error) = error.downcast_ref::<nyaterm_files::adb::AdbFilePolicyError>() {
        return error.code();
    }
    for cause in error.chain() {
        if let Some(error) =
            cause.downcast_ref::<nyaterm_files::file_browser::FileTransferEntryError>()
        {
            return error.code();
        }
        if let Some(error) = cause.downcast_ref::<AdbFileValidationError>() {
            return match error {
                AdbFileValidationError::InvalidPath => "InvalidPath",
                AdbFileValidationError::InvalidPathToken => "InvalidPathToken",
                AdbFileValidationError::InvalidFilename => "InvalidFilename",
            };
        }
        if let Some(error) = cause.downcast_ref::<AdbError>() {
            return match error {
                AdbError::FileMissing => "FileMissing",
                AdbError::Io(error) => io_error_code(error),
                AdbError::ExecutableMissing => "ExecutableMissing",
                AdbError::IncompatibleServer => "IncompatibleServer",
                AdbError::HelperVersionMismatch => "HelperVersionMismatch",
                AdbError::Unauthorized => "Unauthorized",
                AdbError::Offline => "Offline",
                AdbError::Rejected => "Rejected",
                AdbError::Protocol(_) => "Protocol",
                AdbError::Cancelled => "Cancelled",
                AdbError::Timeout => "Timeout",
                AdbError::ShellExit(_) => "ShellExit",
            };
        }
        if let Some(error) = cause.downcast_ref::<std::io::Error>() {
            return io_error_code(error);
        }
    }
    if error
        .chain()
        .any(|cause| cause.to_string() == nyaterm_files::sftp::SFTP_TRANSFER_CANCELLED)
    {
        "Cancelled"
    } else {
        "OperationFailed"
    }
}

fn io_error_code(error: &std::io::Error) -> &'static str {
    match error.kind() {
        std::io::ErrorKind::NotFound => "FileMissing",
        std::io::ErrorKind::PermissionDenied => "PermissionDenied",
        std::io::ErrorKind::TimedOut => "Timeout",
        _ => "Io",
    }
}

pub(in crate::features) fn log_file_error(
    operation: &'static str,
    backend: nyaterm_files::file_browser::FileBrowserBackendKind,
    request_id: &str,
    error: &anyhow::Error,
) {
    let code = file_error_code(error);
    if code != "Cancelled" {
        tracing::warn!(operation, backend = ?backend, request_id, error_code = code, "File browser operation failed");
    }
}

pub(in crate::features) fn session_start_error_message(
    error: nyaterm_transport::SessionError,
) -> String {
    let error = anyhow::Error::new(error);
    error
        .chain()
        .find_map(|cause| cause.downcast_ref::<AdbError>())
        .map(adb_error_message)
        .unwrap_or_else(|| t!("android.errors.operationFailed").into_owned())
}

pub(in crate::features) fn shell_event_message(code: &str) -> String {
    match code {
        "AndroidShellInputFailed" => t!("android.errors.inputFailed").into_owned(),
        "AndroidDeviceDisconnected" => t!("android.errors.disconnected").into_owned(),
        "AndroidShellTruncated" | "AndroidShellProtocolFailed" => {
            t!("android.errors.protocol").into_owned()
        }
        _ => code
            .strip_prefix("AndroidShellExit:")
            .and_then(|code| code.parse::<u8>().ok())
            .map(|code| t!("android.errors.shellExit", code = code).into_owned())
            .unwrap_or_else(|| t!("android.errors.operationFailed").into_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::{adb_error_message, fatal_message, file_error_message, shell_event_message};
    use nyaterm_android::error::AdbError;

    #[test]
    fn android_errors_translate_machine_codes_without_exposing_backend_details() {
        assert!(
            !adb_error_message(&AdbError::Protocol("sensitive backend context"))
                .contains("sensitive")
        );
        for code in [
            "DeviceDisconnected",
            "HelperVersionMismatch",
            "HelperInputFailed",
            "HelperHung",
            "DecoderHung",
            "HelperExited",
            "ServerExited",
            "VideoEnded",
            "ServerProtocolFailed",
            "DecoderPanic",
            "VideoFailed",
            "HelperPanic",
            "HelperFailed",
            "unknown-future-code",
        ] {
            let message = fatal_message(code);
            assert!(!message.is_empty());
            assert_ne!(message, code);
            assert!(!message.starts_with("android."));
        }
        for code in [
            "AndroidShellInputFailed",
            "AndroidDeviceDisconnected",
            "AndroidShellTruncated",
            "AndroidShellProtocolFailed",
            "unknown-future-code",
        ] {
            let message = shell_event_message(code);
            assert_ne!(message, code);
            assert!(!message.starts_with("android."));
        }
        assert!(shell_event_message("AndroidShellExit:42").contains("42"));
        let error =
            anyhow::Error::new(AdbError::Unauthorized).context(nyaterm_files::adb::AdbFileError);
        assert_eq!(
            file_error_message(&error),
            adb_error_message(&AdbError::Unauthorized)
        );
        let unrelated = anyhow::Error::new(nyaterm_transport::SessionError::NotFound(
            "other backend".into(),
        ));
        assert_eq!(file_error_message(&unrelated), unrelated.to_string());
        assert_eq!(
            super::session_start_error_message(nyaterm_transport::SessionError::Adb(
                anyhow::Error::new(AdbError::Unauthorized)
            )),
            adb_error_message(&AdbError::Unauthorized)
        );
        let unknown =
            anyhow::anyhow!("sensitive backend context").context(nyaterm_files::adb::AdbFileError);
        assert!(!file_error_message(&unknown).contains("sensitive"));
        assert_eq!(
            file_error_message(&anyhow::anyhow!("other backend")),
            "other backend"
        );
    }
    #[test]
    fn typed_file_validation_errors_have_translations_in_all_six_locales() {
        use nyaterm_files::adb::{AdbFileError, AdbFileValidationError};
        for (error, key) in [
            (
                AdbFileValidationError::InvalidPath,
                "android.errors.invalidFilePath",
            ),
            (
                AdbFileValidationError::InvalidPathToken,
                "android.errors.invalidPathToken",
            ),
            (
                AdbFileValidationError::InvalidFilename,
                "android.errors.invalidFilename",
            ),
        ] {
            let error = anyhow::Error::new(error).context(AdbFileError);
            assert_ne!(
                file_error_message(&error),
                rust_i18n::t!("android.errors.fileOperationFailed")
            );
            for locale in ["zh-CN", "zh-TW", "en", "ja", "ko", "fr"] {
                let message = rust_i18n::t!(key, locale = locale);
                assert!(!message.is_empty());
                assert_ne!(message, key);
                assert!(!message.contains("%{"));
            }
        }
    }

    #[test]
    fn file_policy_failures_have_specific_translations_and_stable_codes() {
        use nyaterm_files::adb::{AdbFileError, AdbFilePolicyError};
        for (policy, key) in [
            (
                AdbFilePolicyError::AttributesUnavailable,
                "android.errors.fileAttributes",
            ),
            (
                AdbFilePolicyError::PermissionsUnavailable,
                "android.errors.filePermissions",
            ),
            (
                AdbFilePolicyError::TimestampUnavailable,
                "android.errors.fileTimestamp",
            ),
            (
                AdbFilePolicyError::EditLimitExceeded,
                "android.errors.editLimit",
            ),
            (
                AdbFilePolicyError::RevisionRequired,
                "android.errors.revisionRequired",
            ),
        ] {
            let error = anyhow::anyhow!("private device output")
                .context(policy)
                .context(AdbFileError);
            assert_eq!(super::file_error_code(&error), policy.code());
            assert!(!file_error_message(&error).contains("private"));
            for locale in ["zh-CN", "zh-TW", "en", "ja", "ko", "fr"] {
                assert_ne!(rust_i18n::t!(key, locale = locale), key);
            }
        }
    }

    #[test]
    fn file_diagnostics_include_stable_failure_codes_without_sensitive_error_context_or_cancellation()
     {
        use std::io::Write;
        use std::sync::{Arc, Mutex};
        #[derive(Clone)]
        struct LogWriter(Arc<Mutex<Vec<u8>>>);
        impl Write for LogWriter {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let sink = LogWriter(bytes.clone());
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_max_level(tracing::Level::WARN)
            .with_writer(move || sink.clone())
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            let error = anyhow::Error::new(nyaterm_files::adb::AdbFileValidationError::InvalidPath)
                .context("sensitive path, device and output");
            super::log_file_error(
                "list-directory",
                nyaterm_files::file_browser::FileBrowserBackendKind::Adb,
                "request-1",
                &error,
            );
            super::log_file_error(
                "list-directory",
                nyaterm_files::file_browser::FileBrowserBackendKind::Adb,
                "request-cancelled",
                &anyhow::Error::new(AdbError::Cancelled),
            );
        });
        let logs = String::from_utf8(bytes.lock().unwrap().clone()).unwrap();
        assert!(logs.contains("list-directory"));
        assert!(logs.contains("Adb"));
        assert!(logs.contains("request-1"));
        assert!(logs.contains("InvalidPath"));
        assert!(!logs.contains("sensitive"));
        assert!(!logs.contains("request-cancelled"));
        assert_eq!(
            super::file_error_code(&anyhow::Error::new(AdbError::Unauthorized)),
            "Unauthorized"
        );
        assert_eq!(
            super::file_error_code(&anyhow::Error::new(std::io::Error::from(
                std::io::ErrorKind::PermissionDenied
            ))),
            "PermissionDenied"
        );
        assert_eq!(
            super::file_error_code(&anyhow::anyhow!("unknown sensitive detail")),
            "OperationFailed"
        );
    }
}
