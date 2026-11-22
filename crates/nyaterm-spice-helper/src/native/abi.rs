//! Narrow declarations of the public spice-gtk 0.43 / GLib C ABI.
//! Reference: https://www.spice-space.org/download/gtk/spice-gtk-0.43.tar.xz
//! No GObject instance or private protocol structure is accessed by Rust.

use std::ffi::{c_char, c_int, c_uint, c_ulong, c_void};
use std::marker::PhantomData;
use std::mem::ManuallyDrop;
use std::path::Path;
use std::rc::Rc;

use libloading::Library;

use crate::native::NativeError;

pub type Object = *mut c_void;
pub type Callback = Option<unsafe extern "C" fn()>;
pub type DestroyNotify = Option<unsafe extern "C" fn(Object, Object)>;
pub type LogCallback = Option<unsafe extern "C" fn(*const c_char, c_uint, *const c_char, Object)>;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct DisplayMonitor {
    pub id: c_uint,
    pub surface_id: c_uint,
    pub x: c_uint,
    pub y: c_uint,
    pub width: c_uint,
    pub height: c_uint,
}

#[repr(C)]
pub struct Array {
    pub data: *mut u8,
    pub len: c_uint,
}

#[repr(C)]
pub struct Error {
    pub domain: u32,
    pub code: c_int,
    pub message: *mut c_char,
}

macro_rules! api {
    ($($library:ident { $($name:ident: $signature:ty),* $(,)? })*) => {
        pub struct Api {
            $( $( pub $name: $signature, )* )*
            // GType registration retains class vtables even after the last
            // object is unreffed. These modules must stay loaded until exit.
            _libraries: Vec<ManuallyDrop<Library>>,
            // All native access belongs to the creating GLib owner thread.
            _thread: PhantomData<Rc<()>>,
        }

        impl Api {
            pub fn load(directory: &Path) -> Result<Self, NativeError> {
                #[cfg(windows)]
                if directory.is_absolute() && directory.is_dir() {
                    crate::native::environment::configure_dll_directory(directory)?;
                }
                $( let $library = open(directory, library_name(stringify!($library)))?; )*
                // SAFETY: every symbol is declared from the public C headers.
                // The handles are pinned for the lifetime of this process.
                unsafe {
                    Ok(Self {
                        $( $( $name: *$library.get::<$signature>(concat!(stringify!($name), "\0").as_bytes())
                            .map_err(|_| NativeError::MissingSymbol(stringify!($name)))?, )* )*
                        _libraries: vec![$(ManuallyDrop::new($library)),*],
                        _thread: PhantomData,
                    })
                }
            }
        }
    };
}

api! {
    glib {
        g_main_context_default: unsafe extern "C" fn() -> Object,
        g_main_context_ref: unsafe extern "C" fn(Object) -> Object,
        g_main_context_acquire: unsafe extern "C" fn(Object) -> c_int,
        g_main_context_release: unsafe extern "C" fn(Object),
        g_main_context_unref: unsafe extern "C" fn(Object),
        g_main_context_push_thread_default: unsafe extern "C" fn(Object),
        g_main_context_pop_thread_default: unsafe extern "C" fn(Object),
        g_main_context_iteration: unsafe extern "C" fn(Object, c_int) -> c_int,
        g_byte_array_new: unsafe extern "C" fn() -> Object,
        g_byte_array_append: unsafe extern "C" fn(Object, *const u8, c_uint) -> Object,
        g_byte_array_unref: unsafe extern "C" fn(Object),
        g_array_unref: unsafe extern "C" fn(*mut Array),
        g_error_free: unsafe extern "C" fn(*mut Error),
        g_error_matches: unsafe extern "C" fn(*const Error, u32, c_int) -> c_int,
        g_log_set_default_handler: unsafe extern "C" fn(LogCallback, Object) -> LogCallback,
    }
    gobject {
        g_object_new: unsafe extern "C" fn(usize, *const c_char, ...) -> Object,
        g_object_unref: unsafe extern "C" fn(Object),
        g_object_ref: unsafe extern "C" fn(Object) -> Object,
        g_object_set: unsafe extern "C" fn(Object, *const c_char, ...),
        g_object_get: unsafe extern "C" fn(Object, *const c_char, ...),
        g_signal_connect_data: unsafe extern "C" fn(Object, *const c_char, Callback, Object, DestroyNotify, c_uint) -> c_ulong,
        g_signal_handler_disconnect: unsafe extern "C" fn(Object, c_ulong),
    }
    gio {
        g_io_error_quark: unsafe extern "C" fn() -> u32,
        g_file_new_for_path: unsafe extern "C" fn(*const c_char) -> Object,
        g_cancellable_new: unsafe extern "C" fn() -> Object,
        g_cancellable_cancel: unsafe extern "C" fn(Object),
    }
    spice {
        spice_glib_get_version_string: unsafe extern "C" fn() -> *const c_char,
        spice_glib_set_debug: unsafe extern "C" fn(c_int),
        spice_session_get_type: unsafe extern "C" fn() -> usize,
        spice_session_connect: unsafe extern "C" fn(Object) -> c_int,
        spice_session_disconnect: unsafe extern "C" fn(Object),
        spice_channel_connect: unsafe extern "C" fn(Object) -> c_int,
        spice_inputs_channel_motion: unsafe extern "C" fn(Object, c_int, c_int, c_int),
        spice_inputs_channel_position: unsafe extern "C" fn(Object, c_int, c_int, c_int, c_int),
        spice_inputs_channel_button_press: unsafe extern "C" fn(Object, c_int, c_int),
        spice_inputs_channel_button_release: unsafe extern "C" fn(Object, c_int, c_int),
        spice_inputs_channel_key_press: unsafe extern "C" fn(Object, c_uint),
        spice_inputs_channel_key_release: unsafe extern "C" fn(Object, c_uint),
        spice_inputs_channel_set_key_locks: unsafe extern "C" fn(Object, c_uint),
        spice_main_channel_update_display: unsafe extern "C" fn(Object, c_int, c_int, c_int, c_int, c_int, c_int),
        spice_main_channel_update_display_enabled: unsafe extern "C" fn(Object, c_int, c_int, c_int),
        spice_main_channel_send_monitor_config: unsafe extern "C" fn(Object) -> c_int,
        spice_main_channel_agent_test_capability: unsafe extern "C" fn(Object, u32) -> c_int,
        spice_main_channel_clipboard_selection_grab: unsafe extern "C" fn(Object, c_uint, *const u32, c_int),
        spice_main_channel_clipboard_selection_notify: unsafe extern "C" fn(Object, c_uint, u32, *const u8, usize),
        spice_main_channel_clipboard_selection_request: unsafe extern "C" fn(Object, c_uint, u32),
        spice_main_channel_file_copy_async: unsafe extern "C" fn(Object, *const Object, c_int, Object,
            Option<unsafe extern "C" fn(i64, i64, Object)>, Object,
            Option<unsafe extern "C" fn(Object, Object, Object)>, Object),
        spice_main_channel_file_copy_finish: unsafe extern "C" fn(Object, Object, *mut *mut Error) -> c_int,
        spice_playback_channel_set_delay: unsafe extern "C" fn(Object, u32),
    }
}

fn library_name(library: &str) -> &'static str {
    match (std::env::consts::OS, library) {
        ("windows", "glib") => "libglib-2.0-0.dll",
        ("windows", "gobject") => "libgobject-2.0-0.dll",
        ("windows", "gio") => "libgio-2.0-0.dll",
        ("windows", "spice") => "libspice-client-glib-2.0-8.dll",
        ("macos", "glib") => "libglib-2.0.0.dylib",
        ("macos", "gobject") => "libgobject-2.0.0.dylib",
        ("macos", "gio") => "libgio-2.0.0.dylib",
        ("macos", "spice") => "libspice-client-glib-2.0.8.dylib",
        (_, "glib") => "libglib-2.0.so.0",
        (_, "gobject") => "libgobject-2.0.so.0",
        (_, "gio") => "libgio-2.0.so.0",
        (_, "spice") => "libspice-client-glib-2.0.so.8",
        _ => unreachable!("only declared libraries are loaded"),
    }
}

pub(super) fn open(directory: &Path, name: &'static str) -> Result<Library, NativeError> {
    let path = directory.join(name);
    // Never resolve a DLL from the current working directory or arbitrary PATH.
    // Package validation verifies the entire dependency closure beside this DLL.
    if !directory.is_absolute() || !path.is_file() {
        return Err(NativeError::MissingLibrary(name));
    }
    #[cfg(windows)]
    // SAFETY: the path is an explicit runtime location, never server input.
    let result = unsafe {
        libloading::os::windows::Library::load_with_flags(
            path,
            // LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32
            0x0000_0100 | 0x0000_0800,
        )
        .map(Into::into)
    };
    #[cfg(not(windows))]
    // SAFETY: same as the Windows branch; dylib/ELF dependency paths are
    // rewritten and checked during packaging.
    let result = unsafe { Library::new(path) };
    result.map_err(|_| NativeError::LoadLibrary(name))
}
