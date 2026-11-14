mod appearance;
mod data_directory;
mod general;
mod interaction;
mod keybindings;
mod window_background;
#[cfg(any(target_os = "linux", test))]
mod window_decorations;

pub(in crate::features::pages::settings) mod theme_designer;
