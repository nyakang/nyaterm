//! Desktop adapters for native updater installation and progress UI.

pub(crate) use nyaterm_updater::install::{
    PreparedUpdate, cleanup_update_work_dir, launch_installer, take_update_cleanup_path,
};

#[cfg(windows)]
mod progress;

pub fn run_update_helper_if_requested() -> bool {
    #[cfg(windows)]
    return nyaterm_updater::install::run_update_helper_if_requested(progress::run);
    #[cfg(not(windows))]
    nyaterm_updater::install::run_update_helper_if_requested(|_, operation| operation(&mut |_| {}))
}
