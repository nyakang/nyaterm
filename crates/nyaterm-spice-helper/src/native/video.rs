use std::ffi::{CStr, CString, c_char, c_int};
use std::mem::ManuallyDrop;

use crate::native::abi::{Error, Object, open};
use crate::native::{NativeError, NativeRuntime};

fn library(runtime: &NativeRuntime) -> Result<ManuallyDrop<libloading::Library>, NativeError> {
    let name = match std::env::consts::OS {
        "windows" => "libgstreamer-1.0-0.dll",
        "macos" => "libgstreamer-1.0.0.dylib",
        _ => "libgstreamer-1.0.so.0",
    };
    // GStreamer also registers GTypes with module-owned class vtables.
    Ok(ManuallyDrop::new(open(&runtime.directory, name)?))
}

pub(super) fn initialize(runtime: &NativeRuntime) -> Result<(), NativeError> {
    let library = library(runtime)?;
    let plugins = runtime.directory.join("gstreamer-1.0");
    // SAFETY: public GStreamer 1.x signatures; symbols and handles stay pinned.
    unsafe {
        let initialize = library
            .get::<unsafe extern "C" fn(*mut c_int, *mut *mut *mut c_char, *mut *mut Error) -> c_int>(b"gst_init_check\0")
            .map_err(|_| NativeError::MissingSymbol("gst_init_check"))?;
        let load = library
            .get::<unsafe extern "C" fn(*const c_char, *mut *mut Error) -> Object>(
                b"gst_plugin_load_file\0",
            )
            .map_err(|_| NativeError::MissingSymbol("gst_plugin_load_file"))?;
        let unref = library
            .get::<unsafe extern "C" fn(Object)>(b"gst_object_unref\0")
            .map_err(|_| NativeError::MissingSymbol("gst_object_unref"))?;
        let mut error = std::ptr::null_mut();
        let success = initialize(std::ptr::null_mut(), std::ptr::null_mut(), &mut error);
        if !error.is_null() {
            (runtime.api.g_error_free)(error);
        }
        if success == 0 {
            return Err(NativeError::LoadLibrary("GStreamer initialization"));
        }
        // Never discover arbitrary/obsolete plugins left by an old installer.
        // Exact-file loading also works in a signed/read-only installation and
        // avoids external scanners, user registries and directory enumeration.
        let suffix = match std::env::consts::OS {
            "windows" => ".dll",
            "macos" => ".dylib",
            _ => ".so",
        };
        let mut names = vec![
            "coreelements",
            "app",
            "playback",
            "typefindfunctions",
            "jpeg",
            "vpx",
            "openh264",
            "videoparsersbad",
        ];
        if plugins
            .join(format!("libgstvideoconvertscale{suffix}"))
            .is_file()
        {
            names.push("videoconvertscale");
        } else {
            // GStreamer before 1.22 ships these as two distinct modules.
            names.extend(["videoconvert", "videoscale"]);
        }
        for name in names {
            let path = plugins.join(format!("libgst{name}{suffix}"));
            let path = CString::new(path.to_str().ok_or(NativeError::RuntimeDirectory)?)
                .map_err(|_| NativeError::RuntimeDirectory)?;
            let mut error = std::ptr::null_mut();
            let plugin = load(path.as_ptr(), &mut error);
            if !error.is_null() {
                (runtime.api.g_error_free)(error);
            }
            if plugin.is_null() {
                return Err(NativeError::LoadLibrary("GStreamer video plugin"));
            }
            unref(plugin);
        }
    }
    Ok(())
}

pub(super) fn probe(runtime: &NativeRuntime) -> Result<Vec<String>, NativeError> {
    let library = library(runtime)?;
    // SAFETY: public GStreamer signatures; NativeRuntime initialized the library.
    unsafe {
        let create = library
            .get::<unsafe extern "C" fn(*const c_char, *const c_char) -> Object>(
                b"gst_element_factory_make\0",
            )
            .map_err(|_| NativeError::MissingSymbol("gst_element_factory_make"))?;
        let unref = library
            .get::<unsafe extern "C" fn(Object)>(b"gst_object_unref\0")
            .map_err(|_| NativeError::MissingSymbol("gst_object_unref"))?;
        let names: [&CStr; 10] = [
            c"appsrc",
            c"appsink",
            c"decodebin",
            c"videoconvert",
            c"jpegdec",
            c"vp8dec",
            c"vp9dec",
            c"openh264dec",
            c"h264parse",
            c"queue",
        ];
        let mut available = Vec::new();
        for name in names {
            let element = create(name.as_ptr(), std::ptr::null());
            if !element.is_null() {
                available.push(name.to_string_lossy().into_owned());
                unref(element);
            }
        }
        Ok(available)
    }
}
