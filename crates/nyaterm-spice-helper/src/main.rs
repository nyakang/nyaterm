fn main() {
    // Panic payloads can contain guest pixels, clipboard, paths or tickets.
    std::panic::set_hook(Box::new(|_| {}));
    // SAFETY: process entry, before IPC, audio, GLib or any worker threads.
    // If directory resolution fails, the driver reports its typed missing-
    // runtime error after the IPC handshake, before loading native code.
    let _ = unsafe { nyaterm_spice_helper::native::environment::initialize_process_environment() };
    let success = nyaterm_spice_helper::driver::run_stdio();
    std::process::exit(if success { 0 } else { 1 });
}
