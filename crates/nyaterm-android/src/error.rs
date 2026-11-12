use std::io;

#[derive(Debug, thiserror::Error)]
pub enum AdbError {
    #[error("Android file does not exist")]
    FileMissing,
    #[error("ADB I/O: {0}")]
    Io(#[from] io::Error),
    #[error("ADB executable is missing; configure an explicit Platform-Tools path")]
    ExecutableMissing,
    #[error("ADB server is incompatible; stop or update it manually")]
    IncompatibleServer,
    #[error(
        "Android helper version does not match the application; rebuild or reinstall both together"
    )]
    HelperVersionMismatch,
    #[error("Authorize this computer on the Android device")]
    Unauthorized,
    #[error("Android device is offline or missing")]
    Offline,
    #[error("ADB server rejected the request")]
    Rejected,
    #[error("Invalid ADB protocol: {0}")]
    Protocol(&'static str),
    #[error("Android operation was cancelled")]
    Cancelled,
    #[error("Android operation timed out")]
    Timeout,
    #[error("Android shell command failed (exit {0})")]
    ShellExit(u8),
}

pub type AdbResult<T> = Result<T, AdbError>;
