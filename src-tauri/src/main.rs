// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Windows subsystem configuration and entry.
//! Delegates to `nyaterm_lib::run()` for the actual app.

#[cfg(target_os = "linux")]
unsafe extern "C" {
    fn dlopen(filename: *const std::os::raw::c_char, flags: i32) -> *mut std::ffi::c_void;
    fn dlsym(handle: *mut std::ffi::c_void, symbol: *const std::os::raw::c_char) -> *mut std::ffi::c_void;
}

#[cfg(target_os = "linux")]
fn init_linux_workarounds() {
    // XInitThreads must be called before any X11 access (including GTK/WebKit
    // initialization) to prevent the XCB "multi-threaded client" assertion
    // failure that occurs when WebKit2GTK spawns child webview windows on
    // SSH X11 forwarding.  We dlopen libX11 to avoid adding a hard link-time
    // dependency on libX11 (which can interfere with link order).
    //
    // The dlopen handle is intentionally leaked (never dlclose'd) so that
    // libX11 stays loaded for the entire process lifetime.  If we dlclose
    // here, the dynamic loader may unload libX11 (when refcount reaches
    // zero), and GTK/WebKit would later load a fresh copy where
    // XInitThreads was never called — re-introducing the XCB crash.
    //
    // `lib` is a raw pointer (Copy), so it has no Drop; simply not calling
    // dlclose is sufficient to keep the library loaded.
    unsafe {
        let name = b"libX11.so.6\0";
        let lib = dlopen(name.as_ptr().cast(), 0x101); // RTLD_LAZY | RTLD_GLOBAL
        if !lib.is_null() {
            let sym_name = b"XInitThreads\0";
            let sym = dlsym(lib, sym_name.as_ptr().cast());
            if !sym.is_null() {
                let func: extern "C" fn() -> i32 = std::mem::transmute(sym);
                func();
            }
            // Intentionally do NOT dlclose(lib) — see comment above.
            let _ = lib;
        }
    }
    // Force synchronized X11 access in GDK as an additional safeguard.
    unsafe { std::env::set_var("GDK_SYNCHRONIZE", "1"); }
}

#[cfg(not(target_os = "linux"))]
fn init_linux_workarounds() {}

fn main() {
    init_linux_workarounds();
    nyaterm_lib::run()
}
