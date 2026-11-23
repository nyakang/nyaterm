//! Process-wide native search policy, installed before the helper starts threads.

use std::ffi::OsString;
use std::path::Path;

use crate::native::{NativeError, runtime_directory};

/// Configure only this isolated helper's native libraries.
///
/// # Safety
/// Call at process entry, before creating threads or loading any native library.
/// Environment mutation is not safe after third-party threads have started.
pub unsafe fn initialize_process_environment() -> Result<(), NativeError> {
    let directory = runtime_directory()?;
    for (key, value) in environment(&directory) {
        // SAFETY: the caller guarantees single-threaded process initialization.
        unsafe { std::env::set_var(key, value) };
    }
    Ok(())
}

fn environment(directory: &Path) -> Vec<(&'static str, OsString)> {
    vec![
        ("GST_PLUGIN_PATH", OsString::new()),
        ("GST_PLUGIN_PATH_1_0", OsString::new()),
        ("GST_PLUGIN_SYSTEM_PATH", OsString::new()),
        ("GST_PLUGIN_SYSTEM_PATH_1_0", OsString::new()),
        // Load the small packaged allowlist in-process. No external scanner or
        // shared per-user registry may inject modules from another installation.
        // UPDATE=no still writes a newly created registry on some versions.
        // Disable disk caching entirely; video::initialize loads exact files.
        ("GST_REGISTRY_DISABLE", "yes".into()),
        ("GST_REGISTRY_FORK", "no".into()),
        ("GST_REGISTRY_UPDATE", "no".into()),
        ("GST_DEBUG", "0".into()),
        ("GST_DEBUG_FILE", OsString::new()),
        ("GST_PLUGIN_FEATURE_RANK", OsString::new()),
        ("SASL_PATH", directory.join("sasl2").into()),
        ("SASL_CONF_PATH", directory.join("sasl2").into()),
        ("GIO_MODULE_DIR", directory.join("gio-modules").into()),
        ("GIO_EXTRA_MODULES", OsString::new()),
        ("OPENSSL_CONF", directory.join("openssl.cnf").into()),
        ("OPENSSL_MODULES", directory.join("ossl-modules").into()),
    ]
}

#[cfg(windows)]
pub(super) fn configure_dll_directory(directory: &Path) -> Result<(), NativeError> {
    use std::os::windows::ffi::OsStrExt;
    use std::sync::OnceLock;
    // These handles deliberately survive all NativeRuntime instances, just as
    // registered GTypes and their backing modules do.
    static DIRECTORY: OnceLock<Result<std::path::PathBuf, ()>> = OnceLock::new();
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn SetDefaultDllDirectories(flags: u32) -> i32;
        fn AddDllDirectory(path: *const u16) -> *mut std::ffi::c_void;
    }
    if !directory.is_absolute() || !directory.is_dir() {
        return Err(NativeError::RuntimeDirectory);
    }
    let configured = DIRECTORY.get_or_init(|| {
        let path: Vec<u16> = directory.as_os_str().encode_wide().chain(Some(0)).collect();
        // Includes explicit registered directories and System32, never CWD/PATH.
        // Needed for plugins opened internally by GModule and Cyrus SASL.
        let ok = unsafe {
            SetDefaultDllDirectories(0x0000_0400 | 0x0000_0800) != 0
                && !AddDllDirectory(path.as_ptr()).is_null()
        };
        ok.then(|| directory.to_path_buf()).ok_or(())
    });
    match configured {
        Ok(configured) if configured == directory => Ok(()),
        _ => Err(NativeError::RuntimeDirectory),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::native::environment::environment;

    #[test]
    fn native_plugin_paths_are_private_and_do_not_extend_inherited_search_paths() {
        let directory = Path::new("/private/runtime");
        let values = environment(directory);
        for key in [
            "GST_PLUGIN_PATH",
            "GST_PLUGIN_PATH_1_0",
            "GST_PLUGIN_SYSTEM_PATH",
            "GST_PLUGIN_SYSTEM_PATH_1_0",
            "GIO_EXTRA_MODULES",
        ] {
            assert!(
                values
                    .iter()
                    .find(|(name, _)| *name == key)
                    .unwrap()
                    .1
                    .is_empty()
            );
        }
        for key in ["SASL_PATH", "OPENSSL_MODULES"] {
            let value = &values.iter().find(|(name, _)| *name == key).unwrap().1;
            assert!(Path::new(value).starts_with(directory));
        }
    }
}
