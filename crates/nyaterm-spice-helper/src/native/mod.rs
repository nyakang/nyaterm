mod abi;
mod client;
pub mod environment;
mod session;
mod video;

use std::ffi::CStr;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::thread::ThreadId;

use serde::Serialize;
use thiserror::Error;

use crate::native::abi::{Api, Object};
pub use crate::native::client::{NativeClient, NativeEvent};
pub use crate::native::session::NativeSession;

// spice-gtk's coroutine leader is process-global, including its Windows Fiber.
// Dropping a Rust runtime does not make another OS thread a valid new owner.
static NATIVE_THREAD: Mutex<Option<ThreadId>> = Mutex::new(None);

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum NativeError {
    #[error("SPICE runtime directory could not be resolved")]
    RuntimeDirectory,
    #[error("SPICE native library is missing: {0}")]
    MissingLibrary(&'static str),
    #[error("SPICE native library or one of its dependencies cannot be loaded: {0}")]
    LoadLibrary(&'static str),
    #[error("SPICE runtime is missing a required API: {0}")]
    MissingSymbol(&'static str),
    #[error("SPICE runtime must be version 0.43 or newer")]
    UnsupportedVersion,
    #[error("SPICE native allocation failed")]
    Allocation,
    #[error("SPICE GLib context is already owned by another thread")]
    ContextBusy,
    #[error("invalid SPICE native configuration field: {0}")]
    InvalidField(&'static str),
    #[error("{0}")]
    Configuration(#[from] nyaterm_core::spice::settings::SpiceConfigError),
}

#[derive(Debug, Serialize)]
pub struct RuntimeProbe {
    pub spice_version: String,
    pub os: &'static str,
    pub architecture: &'static str,
    pub session_created: bool,
    pub device_redirection_disabled: bool,
    pub video_factories: Vec<String>,
}

/// One GLib context and its thread-affine ABI. Neither can cross a Rust thread.
pub struct NativeRuntime {
    api: Api,
    context: Object,
    version: String,
    directory: PathBuf,
}

impl NativeRuntime {
    pub fn load() -> Result<Self, NativeError> {
        let directory = runtime_directory()?;
        Self::load_from(&directory)
    }

    pub fn load_from(directory: &Path) -> Result<Self, NativeError> {
        let mut owner = NATIVE_THREAD.lock().map_err(|_| NativeError::ContextBusy)?;
        let current = std::thread::current().id();
        if owner.is_some_and(|id| id != current) {
            return Err(NativeError::ContextBusy);
        }
        let api = Api::load(directory)?;
        *owner = Some(current);
        drop(owner);
        // Native error strings may contain server names, proxy tickets, or
        // clipboard data. Only typed protocol errors cross the helper boundary.
        unsafe {
            (api.g_log_set_default_handler)(Some(redacted_native_log), std::ptr::null_mut());
            (api.spice_glib_set_debug)(0);
        }
        // SAFETY: public API returns an immutable static NUL-terminated string.
        let version = unsafe {
            let version = (api.spice_glib_get_version_string)();
            if version.is_null() {
                return Err(NativeError::UnsupportedVersion);
            }
            CStr::from_ptr(version)
                .to_str()
                .map_err(|_| NativeError::UnsupportedVersion)?
                .to_owned()
        };
        if !supported_version(&version) {
            return Err(NativeError::UnsupportedVersion);
        }
        // spice-gtk's coroutine sockets and video timers attach sources to
        // the process-default context (g_source_attach(..., NULL)). A private
        // thread-default context alone leaves those sources permanently idle.
        // Each isolated helper owns exactly one process-default context.
        // SAFETY: acquire checks ownership; Api's !Send/!Sync marker keeps it
        // on this thread. Drop balances both acquisitions and our reference.
        let context = unsafe {
            let context = (api.g_main_context_ref)((api.g_main_context_default)());
            if context.is_null() {
                return Err(NativeError::Allocation);
            }
            if (api.g_main_context_acquire)(context) == 0 {
                (api.g_main_context_unref)(context);
                return Err(NativeError::ContextBusy);
            }
            (api.g_main_context_push_thread_default)(context);
            context
        };
        let runtime = Self {
            api,
            context,
            version,
            directory: directory.to_path_buf(),
        };
        video::initialize(&runtime)?;
        Ok(runtime)
    }

    /// Probe actual native symbol resolution and object allocation, not merely
    /// the presence of a DLL. It performs no server or device access.
    pub fn probe(&self) -> Result<RuntimeProbe, NativeError> {
        let session = NativeSession::new(self)?;
        Ok(RuntimeProbe {
            spice_version: self.version.clone(),
            os: std::env::consts::OS,
            architecture: std::env::consts::ARCH,
            session_created: true,
            device_redirection_disabled: session.device_redirection_disabled(),
            video_factories: crate::native::video::probe(self)?,
        })
    }
}

unsafe extern "C" fn redacted_native_log(
    _domain: *const std::ffi::c_char,
    level: std::ffi::c_uint,
    _message: *const std::ffi::c_char,
    _data: Object,
) {
    if level & (4 | 8 | 16) != 0 {
        use std::io::Write;
        // A C callback must not panic even if stderr has already closed.
        let _ = std::io::stderr()
            .lock()
            .write_all(b"SPICE native warning (details redacted)\n");
    }
}

impl Drop for NativeRuntime {
    fn drop(&mut self) {
        // SAFETY: context was pushed on this same thread; no live object is
        // allowed to outlive the runtime. Library handles remain pinned.
        unsafe {
            (self.api.g_main_context_pop_thread_default)(self.context);
            (self.api.g_main_context_release)(self.context);
            (self.api.g_main_context_unref)(self.context);
        }
    }
}

fn supported_version(version: &str) -> bool {
    let mut parts = version.split('.');
    match (
        parts.next().and_then(|s| s.parse::<u32>().ok()),
        parts.next().and_then(|s| s.parse::<u32>().ok()),
    ) {
        (Some(0), Some(minor)) => minor >= 43,
        (Some(major), Some(_)) => major >= 1,
        _ => false,
    }
}

pub fn runtime_directory() -> Result<PathBuf, NativeError> {
    if let Some(path) = std::env::var_os("NYATERM_SPICE_RUNTIME") {
        let path = PathBuf::from(path);
        return path
            .canonicalize()
            .map_err(|_| NativeError::RuntimeDirectory);
    }
    let executable = std::env::current_exe().map_err(|_| NativeError::RuntimeDirectory)?;
    let parent = executable.parent().ok_or(NativeError::RuntimeDirectory)?;
    Ok(parent.join("spice"))
}

#[cfg(test)]
mod tests {
    use crate::native::{NativeError, NativeRuntime, supported_version};

    #[test]
    fn versions_before_the_supported_abi_are_rejected() {
        for version in ["0.35", "0.42", "garbage", "0", ""] {
            assert!(!supported_version(version));
        }
        for version in ["0.43", "0.43.1", "0.44", "1.0"] {
            assert!(supported_version(version));
        }
    }

    #[test]
    fn missing_runtime_is_reported_without_searching_the_working_directory() {
        let error = NativeRuntime::load_from(std::path::Path::new("relative-runtime"))
            .err()
            .unwrap();
        assert!(matches!(error, NativeError::MissingLibrary(_)));
    }
}
